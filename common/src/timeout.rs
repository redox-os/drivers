use std::{thread, time::{Duration, Instant}};

pub struct Timeout {
    instant: Instant,
    duration: Duration,
}

impl Timeout {
    #[inline]
    pub fn new(duration: Duration) -> Self {
        Self {
            instant: Instant::now(),
            duration,
        }
    }

    #[inline]
    pub fn from_micros(micros: u64) -> Self {
        Self::new(Duration::from_micros(micros))
    }

    #[inline]
    pub fn from_millis(millis: u64) -> Self {
        Self::new(Duration::from_millis(millis))
    }

    #[inline]
    pub fn from_secs(secs: u64) -> Self {
        Self::new(Duration::from_secs(secs))
    }

    #[inline]
    pub fn run(&self) -> Result<(), ()> {
        if self.instant.elapsed() < self.duration {
            // Sleeps in Redox are only evaluated on PIT ticks (a few ms), which is not
            // short enough for a reasonably responsive timeout. However, the clock is
            // highly accurate. So, we yield instead of sleep to reduce latency.
            //TODO: allow timeout that spins instead of yields?
            std::thread::yield_now();
            Ok(())
        } else {
            Err(())
        }
    }
}