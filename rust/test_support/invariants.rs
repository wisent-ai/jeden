use std::collections::{BTreeMap, BTreeSet};

#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct ReliabilityState {
    pub live_processes: BTreeSet<u64>,
    pub live_leases: BTreeSet<String>,
    pub pending_outbox: BTreeSet<String>,
    pub ledger_parents: BTreeMap<String, Option<String>>,
    pub terminal_counts: BTreeMap<String, u64>,
    pub durable_bytes: Vec<u8>,
    pub maximum_queue_depth: usize,
    pub queue_limit: usize,
    pub cancellation_millis: Vec<u64>,
}

#[allow(dead_code)]
impl ReliabilityState {
    pub fn observe_queue(&mut self, depth: usize) {
        self.maximum_queue_depth = self.maximum_queue_depth.max(depth);
    }
    pub fn terminal(&mut self, operation: impl Into<String>) {
        *self.terminal_counts.entry(operation.into()).or_default() += 1;
    }

    pub fn assert_clean(
        &self,
        canaries: &[&str],
        cancellation_limit_ms: u64,
    ) -> Result<(), String> {
        if !self.live_processes.is_empty() {
            return Err(format!("orphan processes: {}", self.live_processes.len()));
        }
        if !self.live_leases.is_empty() {
            return Err(format!("orphan leases: {}", self.live_leases.len()));
        }
        if !self.pending_outbox.is_empty() {
            return Err(format!("pending outbox: {}", self.pending_outbox.len()));
        }
        if self.maximum_queue_depth > self.queue_limit {
            return Err(format!(
                "queue depth {} exceeds {}",
                self.maximum_queue_depth, self.queue_limit
            ));
        }
        if self.terminal_counts.values().any(|count| *count != 1) {
            return Err("missing or duplicate terminal outcome".into());
        }
        for id in self.ledger_parents.keys() {
            self.assert_lineage(id)?;
        }
        for canary in canaries.iter().filter(|value| !value.is_empty()) {
            if self
                .durable_bytes
                .windows(canary.len())
                .any(|bytes| bytes == canary.as_bytes())
            {
                return Err("plaintext secret canary reached durable state".into());
            }
        }
        if percentile_99(&self.cancellation_millis) > cancellation_limit_ms {
            return Err(format!(
                "cancellation p99 exceeded {cancellation_limit_ms}ms"
            ));
        }
        Ok(())
    }

    fn assert_lineage(&self, start: &str) -> Result<(), String> {
        let mut seen = BTreeSet::new();
        let mut current = Some(start);
        while let Some(id) = current {
            if !seen.insert(id) {
                return Err(format!("ledger lineage cycle at {id}"));
            }
            current = self
                .ledger_parents
                .get(id)
                .and_then(|parent| parent.as_deref());
        }
        Ok(())
    }
}

fn percentile_99(values: &[u64]) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted[((sorted.len() - 1) * 99).div_ceil(100)]
}
