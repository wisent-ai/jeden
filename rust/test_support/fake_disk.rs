use std::collections::BTreeMap;

use super::failpoints::{Failpoint, Failpoints};

#[allow(dead_code)]
#[derive(Debug)]
pub struct FakeDisk {
    durable: BTreeMap<String, Vec<u8>>,
    max_bytes: usize,
    failpoints: Failpoints,
}

#[allow(dead_code)]
impl FakeDisk {
    pub fn bounded(max_bytes: usize) -> Self {
        Self {
            durable: BTreeMap::new(),
            max_bytes,
            failpoints: Failpoints::default(),
        }
    }
    pub fn failpoints(&self) -> &Failpoints {
        &self.failpoints
    }
    pub fn read(&self, key: &str) -> Option<&[u8]> {
        self.durable.get(key).map(Vec::as_slice)
    }
    pub fn total_bytes(&self) -> usize {
        self.durable.values().map(Vec::len).sum()
    }

    /// Models temp -> sync -> rename. A failure exposes either the prior durable value or the
    /// complete new value, never a partial record.
    pub fn atomic_replace(&mut self, key: &str, value: &[u8]) -> Result<(), Failpoint> {
        self.failpoints.hit(Failpoint::BeforeWrite)?;
        let projected = self
            .total_bytes()
            .saturating_sub(self.durable.get(key).map_or(0, Vec::len))
            .saturating_add(value.len());
        if projected > self.max_bytes {
            return Err(Failpoint::BeforeWrite);
        }
        let staged = value.to_vec();
        self.failpoints.hit(Failpoint::AfterWriteBeforeSync)?;
        self.failpoints.hit(Failpoint::AfterSyncBeforeRename)?;
        self.durable.insert(key.to_owned(), staged);
        self.failpoints.hit(Failpoint::AfterRename)
    }
}
