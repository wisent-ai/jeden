use super::SubscriptionTargetIdentity;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

#[derive(Clone, Debug, PartialEq)]
pub struct OutcomeObservation {
    pub served_route: String,
    pub succeeded: bool,
    pub quality: f64,
    pub cost_microunits: u64,
    pub latency_ms: u64,
    pub retries: u32,
    pub failovers: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServedRouteEvidence {
    pub decision_id: String,
    pub selected_route: String,
    pub served_route: String,
    pub attempt: u32,
    pub retry: bool,
    pub fallback: bool,
}

impl ServedRouteEvidence {
    pub fn initial(decision_id: impl Into<String>, selected_route: impl Into<String>) -> Self {
        let selected_route = selected_route.into();
        Self {
            decision_id: decision_id.into(),
            served_route: selected_route.clone(),
            selected_route,
            attempt: 1,
            retry: false,
            fallback: false,
        }
    }

    pub fn retry(&self, attempt: u32) -> Self {
        Self {
            attempt,
            retry: true,
            ..self.clone()
        }
    }

    pub fn fallback(&self, served_route: impl Into<String>, attempt: u32) -> Self {
        Self {
            served_route: served_route.into(),
            attempt,
            retry: attempt > 1,
            fallback: true,
            ..self.clone()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RouteQuality {
    pub samples: u64,
    pub successes: u64,
    pub mean_quality: f64,
    pub mean_cost_microunits: f64,
    pub mean_latency_ms: f64,
    pub retries: u64,
    pub failovers: u64,
}

impl RouteQuality {
    fn observe(&mut self, outcome: &OutcomeObservation) {
        self.samples = self.samples.saturating_add(1);
        self.successes = self.successes.saturating_add(u64::from(outcome.succeeded));
        let n = self.samples as f64;
        self.mean_quality += (outcome.quality.clamp(0.0, 1.0) - self.mean_quality) / n;
        self.mean_cost_microunits +=
            (outcome.cost_microunits as f64 - self.mean_cost_microunits) / n;
        self.mean_latency_ms += (outcome.latency_ms as f64 - self.mean_latency_ms) / n;
        self.retries = self.retries.saturating_add(outcome.retries as u64);
        self.failovers = self.failovers.saturating_add(outcome.failovers as u64);
    }

    pub fn success_rate(&self) -> f64 {
        if self.samples == 0 {
            0.0
        } else {
            self.successes as f64 / self.samples as f64
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct QualityMetrics {
    pub observations: u64,
    pub mean_regret: f64,
    pub calibration_error: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct QualitySnapshot {
    pub routes: BTreeMap<String, RouteQuality>,
    pub metrics: QualityMetrics,
}

#[derive(Clone, Default)]
pub struct QualityStore {
    inner: Arc<RwLock<QualitySnapshot>>,
}

impl QualityStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Updates the actually served route, never the merely selected route.
    pub fn update(&self, outcome: OutcomeObservation) {
        let mut state = self.inner.write().expect("quality store poisoned");
        let predicted = state
            .routes
            .get(&outcome.served_route)
            .map(|r| r.mean_quality)
            .unwrap_or(0.5);
        let best = state
            .routes
            .values()
            .filter(|r| r.samples > 0)
            .map(|r| r.mean_quality)
            .fold(outcome.quality, f64::max);
        let regret = (best - outcome.quality).max(0.0);
        let calibration = (predicted - outcome.quality).abs();
        let n = state.metrics.observations.saturating_add(1);
        state.metrics.mean_regret += (regret - state.metrics.mean_regret) / n as f64;
        state.metrics.calibration_error +=
            (calibration - state.metrics.calibration_error) / n as f64;
        state.metrics.observations = n;
        state
            .routes
            .entry(outcome.served_route.clone())
            .or_default()
            .observe(&outcome);
    }

    pub fn route(&self, route: &str) -> Option<RouteQuality> {
        self.inner
            .read()
            .expect("quality store poisoned")
            .routes
            .get(route)
            .cloned()
    }

    pub fn snapshot(&self) -> QualitySnapshot {
        self.inner.read().expect("quality store poisoned").clone()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CooldownEntry {
    target: SubscriptionTargetIdentity,
    until_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct CooldownDocument {
    version: u32,
    entries: Vec<CooldownEntry>,
}

/// Durable quota cooldowns. Expired entries are ignored; recording uses the
/// latest on-disk document and a max deadline so concurrent clones cannot
/// shorten a cooldown.
#[derive(Clone)]
pub struct CooldownStore {
    path: PathBuf,
    operation: Arc<Mutex<()>>,
}

impl CooldownStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let store = Self {
            path,
            operation: Arc::new(Mutex::new(())),
        };
        store.load()?;
        Ok(store)
    }

    pub fn is_cooling_down(
        &self,
        target: &SubscriptionTargetIdentity,
        now_ms: u64,
    ) -> Result<bool, String> {
        let _guard = self
            .operation
            .lock()
            .map_err(|_| "cooldown store poisoned")?;
        Ok(self
            .load()?
            .entries
            .iter()
            .any(|entry| &entry.target == target && entry.until_ms > now_ms))
    }

    pub fn cooldown_until(
        &self,
        target: &SubscriptionTargetIdentity,
    ) -> Result<Option<u64>, String> {
        let _guard = self
            .operation
            .lock()
            .map_err(|_| "cooldown store poisoned")?;
        Ok(self
            .load()?
            .entries
            .iter()
            .find(|entry| &entry.target == target)
            .map(|entry| entry.until_ms))
    }

    pub fn record(
        &self,
        target: SubscriptionTargetIdentity,
        until_ms: u64,
        now_ms: u64,
    ) -> Result<(), String> {
        if until_ms <= now_ms {
            return Err("cooldown deadline must be in the future".into());
        }
        let _guard = self
            .operation
            .lock()
            .map_err(|_| "cooldown store poisoned")?;
        let mut document = self.load()?;
        document.entries.retain(|entry| entry.until_ms > now_ms);
        if let Some(entry) = document
            .entries
            .iter_mut()
            .find(|entry| entry.target == target)
        {
            entry.until_ms = entry.until_ms.max(until_ms);
        } else {
            document.entries.push(CooldownEntry { target, until_ms });
            document
                .entries
                .sort_by(|left, right| left.target.cmp(&right.target));
        }
        self.persist(&document)
    }

    fn load(&self) -> Result<CooldownDocument, String> {
        if !self.path.exists() {
            return Ok(CooldownDocument {
                version: 1,
                entries: Vec::new(),
            });
        }
        let document: CooldownDocument =
            serde_json::from_slice(&fs::read(&self.path).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        if document.version != 1 {
            return Err(format!(
                "unsupported cooldown store version {}",
                document.version
            ));
        }
        Ok(document)
    }

    fn persist(&self, document: &CooldownDocument) -> Result<(), String> {
        let parent = self.path.parent().ok_or("cooldown path has no parent")?;
        let temporary = self
            .path
            .with_extension(format!("tmp-{}", std::process::id()));
        let _ = fs::remove_file(&temporary);
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        let result = (|| {
            serde_json::to_writer(&mut file, document).map_err(|error| error.to_string())?;
            file.write_all(b"\n").map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
            fs::rename(&temporary, &self.path).map_err(|error| error.to_string())?;
            OpenOptions::new()
                .read(true)
                .open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| error.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}
