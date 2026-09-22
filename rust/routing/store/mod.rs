use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

mod cooldown;

pub use cooldown::CooldownStore;

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
