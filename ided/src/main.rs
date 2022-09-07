use pcid_interface::PcidServerHandle;

use log::{error, info};
use redox_log::{OutputBuilder, RedoxLogger};

fn setup_logging() -> Option<&'static RedoxLogger> {
    let mut logger = RedoxLogger::new()
        .with_output(
            OutputBuilder::stderr()
                .with_filter(log::LevelFilter::Info) // limit global output to important info
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        );

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("disk", "pcie", "ide.log") {
        Ok(b) => logger = logger.with_output(
            // TODO: Add a configuration file for this
            b.with_filter(log::LevelFilter::Info)
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("ided: failed to create ide.log: {}", error),
    }

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("disk", "pcie", "ide.ansi.log") {
        Ok(b) => logger = logger.with_output(
            b.with_filter(log::LevelFilter::Info)
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("ided: failed to create ide.ansi.log: {}", error),
    }

    match logger.enable() {
        Ok(logger_ref) => {
            eprintln!("ided: enabled logger");
            Some(logger_ref)
        }
        Err(error) => {
            eprintln!("ided: failed to set default logger: {}", error);
            None
        }
    }
}

fn main() {
    redox_daemon::Daemon::new(daemon).expect("ided: failed to daemonize");
}

fn daemon(daemon: redox_daemon::Daemon) -> ! {
    let _logger_ref = setup_logging();

    let mut pcid_handle =
        PcidServerHandle::connect_default().expect("ided: failed to setup channel to pcid");

    println!("IDE {:#x?}", pcid_handle.fetch_header());

    std::process::exit(0);
}
