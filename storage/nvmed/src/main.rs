use std::cell::RefCell;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::rc::Rc;
use std::sync::Arc;
use std::usize;

use driver_block::{Disk, DiskScheme};
use pcid_interface::{irq_helpers, PciFunctionHandle};

use crate::nvme::NvmeNamespace;

use self::nvme::Nvme;

mod nvme;

struct NvmeDisk {
    nvme: Arc<Nvme>,
    ns: NvmeNamespace,
}

impl Disk for NvmeDisk {
    fn block_size(&self) -> u32 {
        self.ns.block_size.try_into().unwrap()
    }

    fn size(&self) -> u64 {
        self.ns.blocks * self.ns.block_size
    }

    async fn read(&mut self, block: u64, buffer: &mut [u8]) -> syscall::Result<usize> {
        self.nvme.namespace_read(&self.ns, block, buffer).await
    }

    async fn write(&mut self, block: u64, buffer: &[u8]) -> syscall::Result<usize> {
        self.nvme.namespace_write(&self.ns, block, buffer).await
    }
}

fn time_arm(time_handle: &mut File, secs: i64) -> io::Result<()> {
    let mut time_buf = [0_u8; core::mem::size_of::<libredox::data::TimeSpec>()];
    if time_handle.read(&mut time_buf)? < time_buf.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "time read too small",
        ));
    }

    match libredox::data::timespec_from_mut_bytes(&mut time_buf) {
        time => {
            time.tv_sec += secs;
        }
    }
    time_handle.write(&time_buf)?;
    Ok(())
}

fn main() {
    redox_daemon::Daemon::new(daemon).expect("nvmed: failed to daemonize");
}
fn daemon(daemon: redox_daemon::Daemon) -> ! {
    let mut pcid_handle = PciFunctionHandle::connect_default();
    let pci_config = pcid_handle.config();

    let scheme_name = format!("disk.{}-nvme", pci_config.func.name());

    common::setup_logging(
        "disk",
        "pci",
        &scheme_name,
        common::output_level(),
        common::file_level(),
    );

    log::debug!("NVME PCI CONFIG: {:?}", pci_config);

    let address = unsafe { pcid_handle.map_bar(0).ptr };

    let interrupt_vector = irq_helpers::pci_allocate_interrupt_vector(&mut pcid_handle, "nvmed");
    let iv = interrupt_vector.vector();
    let irq_handle = interrupt_vector.irq_handle().try_clone().unwrap();

    let mut nvme = Nvme::new(address.as_ptr() as usize, interrupt_vector, pcid_handle)
        .expect("nvmed: failed to allocate driver data");

    unsafe { nvme.init().expect("nvmed: failed to init") }
    log::debug!("Finished base initialization");
    let nvme = Arc::new(nvme);

    let executor = nvme::executor::init(Arc::clone(&nvme), iv, false /* FIXME */, irq_handle);

    let mut time_handle = File::open(&format!("/scheme/time/{}", libredox::flag::CLOCK_MONOTONIC))
        .expect("failed to open time handle");

    let mut time_events = Box::pin(
        executor.register_external_event(time_handle.as_raw_fd() as usize, event::EventFlags::READ),
    );

    // Try to init namespaces for 5 seconds
    time_arm(&mut time_handle, 5).expect("failed to arm timer");
    let namespaces = executor.block_on(async {
        let namespaces_future = nvme.init_with_queues();
        let time_future = time_events.as_mut().next();
        futures::pin_mut!(namespaces_future);
        futures::pin_mut!(time_future);
        match futures::future::select(namespaces_future, time_future).await {
            futures::future::Either::Left((namespaces, _)) => namespaces,
            futures::future::Either::Right(_) => panic!("timeout on init"),
        }
    });
    log::debug!("Initialized!");

    let scheme = Rc::new(RefCell::new(DiskScheme::new(
        Some(daemon),
        scheme_name,
        namespaces
            .into_iter()
            .map(|(k, ns)| {
                (
                    k,
                    NvmeDisk {
                        nvme: nvme.clone(),
                        ns,
                    },
                )
            })
            .collect(),
        &*executor,
    )));

    let mut scheme_events = Box::pin(executor.register_external_event(
        scheme.borrow().event_handle().raw(),
        event::EventFlags::READ,
    ));

    libredox::call::setrens(0, 0).expect("nvmed: failed to enter null namespace");

    log::debug!("Starting to listen for scheme events");

    executor.block_on(async {
        loop {
            log::trace!("new event iteration");
            if let Err(err) = scheme.borrow_mut().tick().await {
                log::error!("scheme error: {err}");
            }
            let _ = scheme_events.as_mut().next().await;
        }
    });

    //TODO: destroy NVMe stuff

    std::process::exit(0);
}
