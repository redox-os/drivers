use anyhow::{Context, Result};

// TODO: Do not use target architecture to distinguish these.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod x86;

/// The rtc driver runs only once, being perhaps the first of all processes that init starts (since
/// early logging benefits from knowing the time, even though this can be adjusted later once the
/// time is known). The sole job of `rtcd` is to read from the hardware real-time clock, and then
/// write the offset to the kernel.

fn main() -> Result<()> {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        common::acquire_port_io_rights().context("failed to set iopl")?;

        let time_s = self::x86::get_time();
        let time_ns = u128::from(time_s) * 1_000_000_000;

        std::fs::write("/scheme/sys/update_time_offset", &time_ns.to_ne_bytes())
            .context("failed to write to time offset")?;
    }
    // TODO: aarch64 is currently handled in the kernel

    Ok(())
}
