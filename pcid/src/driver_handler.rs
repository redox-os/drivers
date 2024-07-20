use std::fs::File;
use std::os::unix::io::{FromRawFd, RawFd};
use std::process::Command;
use std::sync::Arc;
use std::thread;

use log::{error, info};
use pci_types::capability::{MultipleMessageSupport, PciCapability};
use pci_types::{ConfigRegionAccess, PciAddress};

use crate::State;

pub struct DriverHandler {
    addr: PciAddress,
    capabilities: Vec<PciCapability>,

    state: Arc<State>,
}

impl DriverHandler {
    pub fn spawn(
        state: Arc<State>,
        func: pcid_interface::PciFunction,
        capabilities: Vec<PciCapability>,
        args: &[String],
    ) {
        let subdriver_args = pcid_interface::SubdriverArguments { func };

        let mut args = args.iter();
        if let Some(program) = args.next() {
            let program = if program.starts_with('/') {
                program.to_owned()
            } else {
                "/usr/lib/drivers/".to_owned() + program
            };
            let mut command = Command::new(program);
            for arg in args {
                if arg.starts_with("$") {
                    panic!("support for $VARIABLE has been removed. use pcid_interface instead");
                }
                command.arg(arg);
            }

            info!("PCID SPAWN {:?}", command);

            // TODO: libc wrapper?
            let [fds1, fds2] = unsafe {
                let mut fds1 = [0 as libc::c_int; 2];
                let mut fds2 = [0 as libc::c_int; 2];

                assert_eq!(
                    libc::pipe(fds1.as_mut_ptr()),
                    0,
                    "pcid: failed to create pcid->client pipe"
                );
                assert_eq!(
                    libc::pipe(fds2.as_mut_ptr()),
                    0,
                    "pcid: failed to create client->pcid pipe"
                );

                [fds1.map(|c| c as usize), fds2.map(|c| c as usize)]
            };

            let [pcid_to_client_read, pcid_to_client_write] = fds1;
            let [pcid_from_client_read, pcid_from_client_write] = fds2;

            let envs = vec![
                ("PCID_TO_CLIENT_FD", format!("{}", pcid_to_client_read)),
                ("PCID_FROM_CLIENT_FD", format!("{}", pcid_from_client_write)),
            ];

            match command.envs(envs).spawn() {
                Ok(mut child) => {
                    let driver_handler = DriverHandler {
                        addr: func.addr,
                        state: state.clone(),
                        capabilities,
                    };
                    let handle = thread::spawn(move || {
                        driver_handler.handle_spawn(
                            pcid_to_client_write,
                            pcid_from_client_read,
                            subdriver_args,
                        );
                    });
                    state.threads.lock().unwrap().push(handle);
                    match child.wait() {
                        Ok(_status) => (),
                        Err(err) => error!("pcid: failed to wait for {:?}: {}", command, err),
                    }
                }
                Err(err) => error!("pcid: failed to execute {:?}: {}", command, err),
            }
        }
    }

    fn respond(
        &mut self,
        request: pcid_interface::PcidClientRequest,
        args: &pcid_interface::SubdriverArguments,
    ) -> pcid_interface::PcidClientResponse {
        use pcid_interface::*;

        #[forbid(non_exhaustive_omitted_patterns)]
        match request {
            PcidClientRequest::RequestVendorCapabilities => PcidClientResponse::VendorCapabilities(
                self.capabilities
                    .iter()
                    .filter_map(|capability| match capability {
                        PciCapability::Vendor(addr) => unsafe {
                            Some(VendorSpecificCapability::parse(*addr, &self.state.pcie))
                        },
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            ),
            PcidClientRequest::RequestConfig => PcidClientResponse::Config(args.clone()),
            PcidClientRequest::RequestFeatures => PcidClientResponse::AllFeatures(
                self.capabilities
                    .iter()
                    .filter_map(|capability| match capability {
                        PciCapability::Msi(_) => Some(PciFeature::Msi),
                        PciCapability::MsiX(_) => Some(PciFeature::MsiX),
                        _ => None,
                    })
                    .collect(),
            ),
            PcidClientRequest::EnableFeature(feature) => {
                match feature {
                    PciFeature::Msi => {
                        if let Some(msix_capability) =
                            self.capabilities
                                .iter_mut()
                                .find_map(|capability| match capability {
                                    PciCapability::MsiX(cap) => Some(cap),
                                    _ => None,
                                })
                        {
                            // If MSI-X is supported disable it before enabling MSI as they can't be
                            // active at the same time.
                            msix_capability.set_enabled(false, &self.state.pcie);
                        }

                        let capability = match self.capabilities.iter_mut().find_map(|capability| {
                            match capability {
                                PciCapability::Msi(cap) => Some(cap),
                                _ => None,
                            }
                        }) {
                            Some(capability) => capability,
                            None => {
                                return PcidClientResponse::Error(
                                    PcidServerResponseError::NonexistentFeature(feature),
                                )
                            }
                        };
                        capability.set_enabled(true, &self.state.pcie);
                        PcidClientResponse::FeatureEnabled(feature)
                    }
                    PciFeature::MsiX => {
                        if let Some(msi_capability) =
                            self.capabilities
                                .iter_mut()
                                .find_map(|capability| match capability {
                                    PciCapability::Msi(cap) => Some(cap),
                                    _ => None,
                                })
                        {
                            // If MSI is supported disable it before enabling MSI-X as they can't be
                            // active at the same time.
                            msi_capability.set_enabled(false, &self.state.pcie);
                        }

                        let capability = match self.capabilities.iter_mut().find_map(|capability| {
                            match capability {
                                PciCapability::MsiX(cap) => Some(cap),
                                _ => None,
                            }
                        }) {
                            Some(capability) => capability,
                            None => {
                                return PcidClientResponse::Error(
                                    PcidServerResponseError::NonexistentFeature(feature),
                                )
                            }
                        };
                        capability.set_enabled(true, &self.state.pcie);
                        PcidClientResponse::FeatureEnabled(feature)
                    }
                }
            }
            PcidClientRequest::FeatureInfo(feature) => PcidClientResponse::FeatureInfo(
                feature,
                match feature {
                    PciFeature::Msi => {
                        if let Some(info) =
                            self.capabilities
                                .iter()
                                .find_map(|capability| match capability {
                                    PciCapability::Msi(cap) => Some(cap),
                                    _ => None,
                                })
                        {
                            PciFeatureInfo::Msi(msi::MsiInfo {
                                log2_multiple_message_capable: info.multiple_message_capable()
                                    as u8,
                                is_64bit: info.is_64bit(),
                                has_per_vector_masking: info.has_per_vector_masking(),
                            })
                        } else {
                            return PcidClientResponse::Error(
                                PcidServerResponseError::NonexistentFeature(feature),
                            );
                        }
                    }
                    PciFeature::MsiX => {
                        if let Some(info) =
                            self.capabilities
                                .iter()
                                .find_map(|capability| match capability {
                                    PciCapability::MsiX(cap) => Some(cap),
                                    _ => None,
                                })
                        {
                            PciFeatureInfo::MsiX(msi::MsixInfo {
                                table_bar: info.table_bar(),
                                table_offset: info.table_offset(),
                                table_size: info.table_size(),
                                pba_bar: info.pba_bar(),
                                pba_offset: info.pba_offset(),
                            })
                        } else {
                            return PcidClientResponse::Error(
                                PcidServerResponseError::NonexistentFeature(feature),
                            );
                        }
                    }
                },
            ),
            PcidClientRequest::SetFeatureInfo(info_to_set) => match info_to_set {
                SetFeatureInfo::Msi(info_to_set) => {
                    if let Some(info) =
                        self.capabilities
                            .iter_mut()
                            .find_map(|capability| match capability {
                                PciCapability::Msi(cap) => Some(cap),
                                _ => None,
                            })
                    {
                        if let Some(mme) = info_to_set.multi_message_enable {
                            if (info.multiple_message_capable() as u8) < mme {
                                return PcidClientResponse::Error(
                                    PcidServerResponseError::InvalidBitPattern,
                                );
                            }
                            info.set_multiple_message_enable(
                                match mme {
                                    0 => MultipleMessageSupport::Int1,
                                    1 => MultipleMessageSupport::Int2,
                                    2 => MultipleMessageSupport::Int4,
                                    3 => MultipleMessageSupport::Int8,
                                    4 => MultipleMessageSupport::Int16,
                                    5 => MultipleMessageSupport::Int32,
                                    _ => {
                                        return PcidClientResponse::Error(
                                            PcidServerResponseError::InvalidBitPattern,
                                        )
                                    }
                                },
                                &self.state.pcie,
                            );
                        }
                        if let Some(message_addr_and_data) = info_to_set.message_address_and_data {
                            let message_addr = message_addr_and_data.addr;
                            if message_addr & 0b11 != 0 {
                                return PcidClientResponse::Error(
                                    PcidServerResponseError::InvalidBitPattern,
                                );
                            }
                            if message_addr_and_data.data
                                & ((1 << info.multiple_message_enable(&self.state.pcie) as u8) - 1)
                                != 0
                            {
                                return PcidClientResponse::Error(
                                    PcidServerResponseError::InvalidBitPattern,
                                );
                            }
                            info.set_message_info(
                                message_addr,
                                message_addr_and_data
                                    .data
                                    .try_into()
                                    .expect("pcid: MSI message data too big"),
                                &self.state.pcie,
                            );
                        }
                        if let Some(mask_bits) = info_to_set.mask_bits {
                            info.set_message_mask(mask_bits, &self.state.pcie);
                        }
                        PcidClientResponse::SetFeatureInfo(PciFeature::Msi)
                    } else {
                        return PcidClientResponse::Error(
                            PcidServerResponseError::NonexistentFeature(PciFeature::Msi),
                        );
                    }
                }
                SetFeatureInfo::MsiX { function_mask } => {
                    if let Some(info) =
                        self.capabilities
                            .iter_mut()
                            .find_map(|capability| match capability {
                                PciCapability::MsiX(cap) => Some(cap),
                                _ => None,
                            })
                    {
                        if let Some(mask) = function_mask {
                            info.set_function_mask(mask, &self.state.pcie);
                        }
                        PcidClientResponse::SetFeatureInfo(PciFeature::MsiX)
                    } else {
                        return PcidClientResponse::Error(
                            PcidServerResponseError::NonexistentFeature(PciFeature::MsiX),
                        );
                    }
                }
                _ => unreachable!(),
            },
            PcidClientRequest::ReadConfig(offset) => {
                let value = unsafe { self.state.pcie.read(self.addr, offset) };
                return PcidClientResponse::ReadConfig(value);
            }
            PcidClientRequest::WriteConfig(offset, value) => {
                unsafe {
                    self.state.pcie.write(self.addr, offset, value);
                }
                return PcidClientResponse::WriteConfig;
            }
            _ => unreachable!(),
        }
    }
    fn handle_spawn(
        mut self,
        pcid_to_client_write: usize,
        pcid_from_client_read: usize,
        args: pcid_interface::SubdriverArguments,
    ) {
        use pcid_interface::*;

        let mut pcid_to_client = unsafe { File::from_raw_fd(pcid_to_client_write as RawFd) };
        let mut pcid_from_client = unsafe { File::from_raw_fd(pcid_from_client_read as RawFd) };

        while let Ok(msg) = recv(&mut pcid_from_client) {
            let response = self.respond(msg, &args);
            send(&mut pcid_to_client, &response).unwrap();
        }
    }
}
