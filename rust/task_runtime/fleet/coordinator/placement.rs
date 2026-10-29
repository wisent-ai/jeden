use std::cmp::Reverse;
use std::num::NonZeroU32;

use crate::task_runtime::protocol::{Job, PlacementConstraints, ProtocolError, Worker};

/// How good a worker is for a job, compared field by field: a worker already
/// holding the job's input first, then the one with more free slots, then the
/// one running less.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PlacementRank {
    pub holds_input: bool,
    pub free_slots: u32,
    pub running: Reverse<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlacementDecision {
    pub worker_id: String,
    pub rank: PlacementRank,
    pub reasons: Vec<String>,
}

pub fn select_worker<'a>(
    job: &Job,
    workers: impl IntoIterator<Item = &'a Worker>,
) -> Result<PlacementDecision, ProtocolError> {
    let mut eligible = Vec::new();
    let mut rejected = Vec::new();
    for worker in workers {
        match hard_constraints(&job.constraints, worker) {
            Ok(capacity) => eligible.push(ranked(job, worker, capacity)),
            Err(reasons) => rejected.push(format!(
                "{}: {}",
                worker.hello.worker_id,
                reasons.join(", ")
            )),
        }
    }
    eligible.sort_by(|left, right| {
        right
            .rank
            .cmp(&left.rank)
            .then_with(|| left.worker_id.cmp(&right.worker_id))
    });
    eligible.into_iter().next().ok_or_else(|| {
        ProtocolError::NoPlacement(if rejected.is_empty() {
            "no registered workers".into()
        } else {
            format!(
                "no worker satisfies hard constraints ({})",
                rejected.join("; ")
            )
        })
    })
}

/// The worker's declared capacity when it meets every hard constraint, or every
/// constraint it misses. A worker that declares no parallel capacity takes no job.
fn hard_constraints(
    required: &PlacementConstraints,
    worker: &Worker,
) -> Result<NonZeroU32, Vec<String>> {
    let available = &worker.hello.descriptor;
    let mut reasons = Vec::new();
    if let Some(os) = required.os.as_ref().filter(|os| **os != available.os) {
        reasons.push(format!("os requires {os}, has {}", available.os));
    }
    if let Some(arch) = required
        .arch
        .as_ref()
        .filter(|arch| **arch != available.arch)
    {
        reasons.push(format!("arch requires {arch}, has {}", available.arch));
    }
    for capability in required.capabilities.difference(&available.capabilities) {
        reasons.push(format!("missing capability {capability}"));
    }
    if let Some(sandbox) = required
        .sandbox_profile
        .as_ref()
        .filter(|sandbox| !available.sandbox_profiles.contains(*sandbox))
    {
        reasons.push(format!("missing sandbox {sandbox}"));
    }
    if let Some(zone) = required
        .trust_zone
        .as_ref()
        .filter(|zone| !available.trust_zones.contains(*zone))
    {
        reasons.push(format!("wrong trust zone {zone}"));
    }
    if let Some(residency) = required
        .residency
        .as_ref()
        .filter(|residency| !available.residencies.contains(*residency))
    {
        reasons.push(format!("wrong residency {residency}"));
    }
    if !available.resources.fits(&required.resources) {
        reasons.push("insufficient resources".into());
    }
    for (key, value) in &required.labels {
        if available.labels.get(key) != Some(value) {
            reasons.push(format!("label {key}={value} not present"));
        }
    }
    let capacity = NonZeroU32::new(available.max_parallel);
    match capacity {
        None => reasons.push("declares no parallel capacity (maxParallel)".into()),
        Some(capacity) if worker.running >= capacity.get() => {
            reasons.push(format!("capacity exhausted {}/{capacity}", worker.running));
        }
        Some(_) => {}
    }
    match capacity {
        Some(capacity) if reasons.is_empty() => Ok(capacity),
        _ => Err(reasons),
    }
}

fn ranked(job: &Job, worker: &Worker, capacity: NonZeroU32) -> PlacementDecision {
    let holds_input = worker
        .hello
        .descriptor
        .cas_objects
        .contains(&job.input_root);
    let free_slots = capacity.get().saturating_sub(worker.running);
    PlacementDecision {
        worker_id: worker.hello.worker_id.clone(),
        rank: PlacementRank {
            holds_input,
            free_slots,
            running: Reverse(worker.running),
        },
        reasons: vec![
            format!("holds input={holds_input}"),
            format!("free={free_slots}"),
        ],
    }
}
