use super::protocol::{Job, PlacementConstraints, ProtocolError, Worker};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlacementDecision {
    pub worker_id: String,
    pub score: i64,
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
            Ok(()) => eligible.push(score(job, worker)),
            Err(reasons) => rejected.push(format!(
                "{}: {}",
                worker.hello.worker_id,
                reasons.join(", ")
            )),
        }
    }
    eligible.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
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

fn hard_constraints(required: &PlacementConstraints, worker: &Worker) -> Result<(), Vec<String>> {
    let available = &worker.hello.descriptor;
    let mut reasons = Vec::new();
    if required
        .os
        .as_ref()
        .map(|v| v != &available.os)
        .unwrap_or(false)
    {
        reasons.push(format!(
            "os requires {:?}, has {}",
            required.os, available.os
        ));
    }
    if required
        .arch
        .as_ref()
        .map(|v| v != &available.arch)
        .unwrap_or(false)
    {
        reasons.push(format!(
            "arch requires {:?}, has {}",
            required.arch, available.arch
        ));
    }
    for capability in required.capabilities.difference(&available.capabilities) {
        reasons.push(format!("missing capability {capability}"));
    }
    if required
        .sandbox_profile
        .as_ref()
        .map(|v| !available.sandbox_profiles.contains(v))
        .unwrap_or(false)
    {
        reasons.push(format!(
            "missing sandbox {}",
            required.sandbox_profile.as_deref().unwrap_or_default()
        ));
    }
    if required
        .trust_zone
        .as_ref()
        .map(|v| !available.trust_zones.contains(v))
        .unwrap_or(false)
    {
        reasons.push(format!(
            "wrong trust zone {}",
            required.trust_zone.as_deref().unwrap_or_default()
        ));
    }
    if required
        .residency
        .as_ref()
        .map(|v| !available.residencies.contains(v))
        .unwrap_or(false)
    {
        reasons.push(format!(
            "wrong residency {}",
            required.residency.as_deref().unwrap_or_default()
        ));
    }
    if !available.resources.fits(&required.resources) {
        reasons.push("insufficient resources".into());
    }
    for (key, value) in &required.labels {
        if available.labels.get(key) != Some(value) {
            reasons.push(format!("label {key}={value} not present"));
        }
    }
    let capacity = available.max_parallel.max(1);
    if worker.running >= capacity {
        reasons.push(format!("capacity exhausted {}/{capacity}", worker.running));
    }
    if reasons.is_empty() {
        Ok(())
    } else {
        Err(reasons)
    }
}

fn score(job: &Job, worker: &Worker) -> PlacementDecision {
    let locality = usize::from(
        worker
            .hello
            .descriptor
            .cas_objects
            .contains(&job.input_root),
    ) as i64;
    let capacity = worker.hello.descriptor.max_parallel.max(1) as i64;
    let free = capacity.saturating_sub(worker.running as i64);
    PlacementDecision {
        worker_id: worker.hello.worker_id.clone(),
        score: locality * 1_000_000 + free * 1_000 - worker.running as i64,
        reasons: vec![format!("locality={locality}"), format!("free={free}")],
    }
}
