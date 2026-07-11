use std::collections::BTreeMap;
use std::sync::Mutex;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Failpoint {
    BeforeWrite,
    AfterWriteBeforeSync,
    AfterSyncBeforeRename,
    AfterRename,
    NetworkConnect,
    NetworkFirstByte,
    NetworkIdle,
    ChildSpawn,
    ChildCancel,
    OutboxClaim,
    OutboxAcknowledge,
}

#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct Failpoints {
    armed: Mutex<BTreeMap<Failpoint, u64>>,
}

#[allow(dead_code)]
impl Failpoints {
    pub fn arm(&self, point: Failpoint, hits: u64) {
        self.armed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(point, hits);
    }

    pub fn hit(&self, point: Failpoint) -> Result<(), Failpoint> {
        let mut armed = self.armed.lock().unwrap_or_else(|p| p.into_inner());
        let Some(remaining) = armed.get_mut(&point) else {
            return Ok(());
        };
        if *remaining == 0 {
            return Ok(());
        }
        *remaining -= 1;
        Err(point)
    }
}
