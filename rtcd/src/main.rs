#[allow(unused_imports)]
use anyhow::{anyhow, Context};

// TODO: Do not use target architecture to distinguish these.
#[cfg(target_arch = "x86_64")]
mod x86_64;

/// The rtc driver runs only once, being perhaps the first of all processes that init starts
/// (because it's nice to know what time it is when logging, even though this can be adjusted
/// dynamically once the time is known). The sole job of `rtcd` is to read from the hardware
/// real-time clock, and then write the offset to the kernel time scheme.

fn main() -> anyhow::Result<()> {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        // TODO: I/O permission bitmap?
        syscall::iopl(3).map_err(|error| anyhow!("failed to set iopl: {}", error))?;

        let time = self::x86_64::get_time();

        std::fs::write("sys:update_time_offset", &u64::to_ne_bytes(time)).context("failed to write to time offset")?;

        Ok(())
    }

    // TODO: Move aarch64 rtc code too.
    #[cfg(not(target_arch = "x86_64"))]
    return Err(anyhow!("rtcd not available for this architecture"));
}
