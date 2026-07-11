use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct FaultClock {
    millis: AtomicU64,
}

#[allow(dead_code)]
impl FaultClock {
    pub fn at(millis: u64) -> Self {
        Self {
            millis: AtomicU64::new(millis),
        }
    }
    pub fn now_millis(&self) -> u64 {
        self.millis.load(Ordering::Acquire)
    }
    pub fn advance(&self, duration: Duration) -> u64 {
        self.millis.fetch_add(
            duration.as_millis().min(u64::MAX as u128) as u64,
            Ordering::AcqRel,
        ) + duration.as_millis().min(u64::MAX as u128) as u64
    }
}
