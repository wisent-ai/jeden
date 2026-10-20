//! What one Jeden process holds in Brama's pool, and moving a session to an
//! account with capacity when its own reaches its limit.
//!
//! When a call on the leased account fails before any output is visible, the
//! session asks Brama again: it releases the lease and takes a new one. Brama
//! passes over a member its ledger holds a rate or quota block against, and
//! a member at the operator's limit, so a different lease is an account with
//! capacity; the same account back means the failure was not the account's,
//! and the call is handled as before. A pool with no account left is Brama's
//! refusal (`409 pool_full`, `503 no_usable_member`) with the acquisition
//! Brama started for it, and that document is the turn's error.
//!
//! Jeden keeps nothing on disk: the leases this process took are held in
//! memory only so the session's end releases them on the gateway that gave
//! them.

use super::*;
use std::sync::{Mutex, MutexGuard};

type Limits = (BTreeMap<String, u64>, BTreeMap<String, Value>);

struct Held {
    /// Each provider's stated limit and each unstated provider's refusal,
    /// as the register answered them; read once per process.
    limits: Option<Limits>,
    /// The lease of each (session, provider), with the configuration that
    /// took it.
    leases: BTreeMap<(String, String), (SessionLease, ChatConfig)>,
}

static HELD: Mutex<Held> = Mutex::new(Held {
    limits: None,
    leases: BTreeMap::new(),
});

fn held() -> Result<MutexGuard<'static, Held>, String> {
    HELD.lock().map_err(|_| {
        "the record of this process's leases was poisoned by a panic in another thread".to_string()
    })
}

/// The provider a route is leased on: `provider/model` names it.
fn route_provider(model: &str) -> Option<&str> {
    model
        .split_once('/')
        .map(|(provider, _)| provider)
        .filter(|provider| !provider.is_empty())
}

/// The lease a call on `route` runs on. A Brama session its starter holds a
/// lease for (`JEDEN_LEASE`) runs its Brama calls on that lease and asks Brama
/// for none: the starter's proxy names that account on each request and
/// relays no lease route. An explicit direct provider ignores that lease.
/// Otherwise no lease is taken when the configuration does not reach Brama,
/// names no session, or the route's provider is not pooled by Brama.
/// A provider Brama declares without a sessions limit is refused with its
/// reason: no lease can be taken and the session is not run outside the pool.
pub(crate) fn for_route(config: &ChatConfig, route: &str) -> Result<Option<SessionLease>, String> {
    if !config.via_brama {
        return Ok(None);
    }
    if let Some(inherited) = &config.inherited_lease {
        return Ok(Some(inherited.clone()));
    }
    let Some(session_id) = config.session_id.as_deref() else {
        return Ok(None);
    };
    let Some(provider) = route_provider(route) else {
        return Ok(None);
    };
    let known = held()?.limits.clone();
    let (limits, unstated) = match known {
        Some(limits) => limits,
        None => {
            let register = register(config)?;
            let limits = (register.limits, register.unstated);
            held()?.limits = Some(limits.clone());
            limits
        }
    };
    if let Some(detail) = unstated.get(provider) {
        return Err(format!(
            "Brama declares provider {provider} without a sessions-per-subscription limit, so session {session_id} cannot lease an account for route {route}: {detail}"
        ));
    }
    if !limits.contains_key(provider) {
        return Ok(None);
    }
    let key = (session_id.to_string(), provider.to_string());
    if let Some((lease, _)) = held()?.leases.get(&key) {
        return Ok(Some(lease.clone()));
    }
    let taken = take(config, provider, session_id)?;
    held()?
        .leases
        .insert(key, (taken.lease.clone(), config.clone()));
    Ok(Some(taken.lease))
}

/// After a failed call on `lease`: release it and ask Brama for the
/// session's account again. `Ok(Some)` is a different account with capacity
/// to retry on; `Ok(None)` is the same account back, so the failure was not
/// the account's. A refusal is Brama's document (a full pool, no usable
/// member) and becomes the turn's error. A lease the session's starter
/// holds is not Jeden's to release: the call's failure stands, and moving
/// the session to another account is the starter's model policy.
pub(crate) fn switch(
    config: &ChatConfig,
    lease: &SessionLease,
) -> Result<Option<SessionLease>, String> {
    if config.inherited_lease.as_ref() == Some(lease) {
        return Ok(None);
    }
    let key = (lease.session_id.clone(), lease.provider.clone());
    release(config, &lease.id).map_err(|error| {
        format!(
            "the call on account {} failed and its lease {} could not be released to ask Brama for another: {error}",
            lease.member(),
            lease.id
        )
    })?;
    held()?.leases.remove(&key);
    let taken = take(config, &lease.provider, &lease.session_id).map_err(|error| {
        format!(
            "account {} of provider {} failed and Brama gave session {} no other: {error}",
            lease.member(),
            lease.provider,
            lease.session_id
        )
    })?;
    held()?
        .leases
        .insert(key, (taken.lease.clone(), config.clone()));
    if taken.lease.subscription_id == lease.subscription_id {
        return Ok(None);
    }
    Ok(Some(taken.lease))
}

/// Release every lease this process took for `session_id`: the session
/// ended. Answers what Brama released; a session that took none releases
/// nothing and asks Brama nothing.
pub(crate) fn release_held(session_id: &str) -> Result<Vec<SessionLease>, String> {
    let taken: Vec<(SessionLease, ChatConfig)> = {
        let mut record = held()?;
        let keys: Vec<(String, String)> = record
            .leases
            .keys()
            .filter(|(session, _)| session == session_id)
            .cloned()
            .collect();
        keys.iter()
            .filter_map(|key| record.leases.remove(key))
            .collect()
    };
    let mut released = Vec::new();
    let mut failures = Vec::new();
    for (lease, config) in taken {
        match release(&config, &lease.id) {
            Ok(ended) => released.extend(ended),
            Err(error) => failures.push(format!(
                "session {session_id} ended and its lease {} on account {} could not be released: {error}",
                lease.id,
                lease.member()
            )),
        }
    }
    if failures.is_empty() {
        Ok(released)
    } else {
        Err(failures.join("; "))
    }
}
