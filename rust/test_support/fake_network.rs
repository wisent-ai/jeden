use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::failpoints::{Failpoint, Failpoints};

#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct FakeNetwork {
    opted_in: AtomicBool,
    requests: AtomicU64,
    failpoints: Failpoints,
}

#[allow(dead_code)]
impl FakeNetwork {
    pub fn set_opt_in(&self, value: bool) {
        self.opted_in.store(value, Ordering::Release);
    }
    pub fn requests(&self) -> u64 {
        self.requests.load(Ordering::Acquire)
    }
    pub fn failpoints(&self) -> &Failpoints {
        &self.failpoints
    }
    pub fn send_allowlisted(&self, _records: usize) -> Result<(), Failpoint> {
        if !self.opted_in.load(Ordering::Acquire) {
            return Err(Failpoint::NetworkConnect);
        }
        self.failpoints.hit(Failpoint::NetworkConnect)?;
        self.requests.fetch_add(1, Ordering::AcqRel);
        self.failpoints.hit(Failpoint::NetworkFirstByte)?;
        self.failpoints.hit(Failpoint::NetworkIdle)
    }
}
