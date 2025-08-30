use pci_types::capability::{MultipleMessageSupport, PciCapability};
use pci_types::{ConfigRegionAccess, EndpointHeader};
use pcid_interface::PciFunction;

use crate::cfg_access::Pcie;

pub struct DriverHandler<'a> {
    func: PciFunction,
    endpoint_header: &'a mut EndpointHeader,
    capabilities: &'a mut [PciCapability],

    pcie: &'a Pcie,
}

impl<'a> DriverHandler<'a> {
    pub fn new(
        func: PciFunction,
        endpoint_header: &'a mut EndpointHeader,
        capabilities: &'a mut [PciCapability],
        pcie: &'a Pcie,
    ) -> Self {
        DriverHandler {
            func,
            endpoint_header,
            capabilities,
            pcie,
        }
    }

    pub fn respond(
        &mut self,
        request: pcid_interface::PcidClientRequest,
    ) -> pcid_interface::PcidClientResponse {
        use pcid_interface::*;

        #[forbid(non_exhaustive_omitted_patterns)]
        match request {
            PcidClientRequest::EnableDevice => {
                self.func.legacy_interrupt_line = crate::enable_function(
                    &self.pcie,
                    &mut self.endpoint_header,
                    &mut self.capabilities,
                );

                PcidClientResponse::EnabledDevice
            }
            PcidClientRequest::RequestVendorCapabilities => PcidClientResponse::VendorCapabilities(
                self.capabilities
                    .iter()
                    .filter_map(|capability| match capability {
                        PciCapability::Vendor(addr) => unsafe {
                            Some(VendorSpecificCapability::parse(*addr, self.pcie))
                        },
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            ),
            PcidClientRequest::RequestConfig => {
                PcidClientResponse::Config(SubdriverArguments { func: self.func })
            }
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
                            msix_capability.set_enabled(false, self.pcie);
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
                        capability.set_enabled(true, self.pcie);
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
                            msi_capability.set_enabled(false, self.pcie);
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
                        capability.set_enabled(true, self.pcie);
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
                                self.pcie,
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
                                & ((1 << info.multiple_message_enable(self.pcie) as u8) - 1)
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
                                self.pcie,
                            );
                        }
                        if let Some(mask_bits) = info_to_set.mask_bits {
                            info.set_message_mask(mask_bits, self.pcie);
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
                            info.set_function_mask(mask, self.pcie);
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
                let value = unsafe { self.pcie.read(self.func.addr, offset) };
                return PcidClientResponse::ReadConfig(value);
            }
            PcidClientRequest::WriteConfig(offset, value) => {
                unsafe {
                    self.pcie.write(self.func.addr, offset, value);
                }
                return PcidClientResponse::WriteConfig;
            }
            _ => unreachable!(),
        }
    }
}
