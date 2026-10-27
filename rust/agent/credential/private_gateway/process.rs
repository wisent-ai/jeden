//! The gateway child and its renewal lease have the same lifetime, including
//! a failed startup whose termination could not be confirmed.
use super::{Gateway, PROGRAM};
use std::fs::File;
use std::process::{Child, ExitStatus};
use std::sync::{Mutex, MutexGuard};

pub(super) struct GatewayProcess {
    _lease: File,
    pub(super) child: Child,
    /// Only a bound listener has a route; a failed startup retains its cause.
    pub(super) route: Result<Gateway, String>,
}

impl GatewayProcess {
    pub(super) fn serving(lease: File, child: Child, gateway: Gateway) -> Self {
        Self { _lease: lease, child, route: Ok(gateway) }
    }

    fn context(&self) -> &str {
        match &self.route {
            Ok(gateway) => &gateway.endpoint,
            Err(cause) => cause,
        }
    }
}

static RUNNING: Mutex<Option<GatewayProcess>> = Mutex::new(None);

pub(super) fn held() -> MutexGuard<'static, Option<GatewayProcess>> {
    match RUNNING.lock() {
        Ok(running) => running,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Reap an observed exit before releasing its lease; an inspection refusal
/// retains ownership and never publishes an unverified endpoint.
pub(super) fn live(running: &mut Option<GatewayProcess>) -> Result<Option<Gateway>, String> {
    let Some(owned) = running.as_mut() else { return Ok(None); };
    match owned.child.try_wait() {
        Ok(Some(status)) => {
            let process = owned.child.id();
            eprintln!("jeden: its own gateway process {process} ended ({status}); recorded state: {}", owned.context());
            running.take();
            Ok(None)
        }
        Ok(None) => match &owned.route {
            Ok(gateway) => Ok(Some(gateway.clone())),
            Err(cause) => Err(format!(
                "{cause}; failed-start process {} has not exited; its child and ownership lease remain held and no gateway route is returned",
                owned.child.id()
            )),
        },
        Err(error) => Err(format!(
            "cannot inspect Jeden's own gateway process {} with try_wait: {error}; recorded state: {}; child state is unverified, ownership remains held and no gateway route is returned",
            owned.child.id(), owned.context()
        )),
    }
}

pub(crate) fn running() -> Result<Option<Gateway>, String> {
    live(&mut held())
}

/// Status describes a recorded cleanup refusal, not an asserted healthy process.
pub(super) fn refusal() -> Option<String> {
    let running = held();
    let owned = running.as_ref()?;
    owned.route.as_ref().err().map(|cause| format!(
        "refused: {cause}; failed-start child {} and its ownership lease remain retained until termination is confirmed",
        owned.child.id()
    ))
}

/// Observe, terminate if necessary, and reap the actual owned child. Each
/// refused system operation is named separately, including its observed error.
pub(super) fn end(child: &mut Child) -> Result<ExitStatus, String> {
    let process = child.id();
    if let Some(status) = child.try_wait().map_err(|error| format!(
        "try_wait for Jeden's own gateway process {process} before termination failed: {error}"
    ))? {
        return Ok(status);
    }
    child.kill().map_err(|error| format!(
        "kill of Jeden's own gateway process {process} failed: {error}"
    ))?;
    stado_wait::blocking(
        stado_wait::Kind::Process,
        format!("Jeden's own gateway process {process} to end"),
        PROGRAM,
        || child.wait(),
    ).map_err(|error| format!(
        "wait to reap Jeden's own gateway process {process} failed: {error}"
    ))
}

/// A failed cleanup stores the real child and lease without inventing a route.
pub(super) fn ended(
    mut child: Child, lease: File, cause: String, said: &[String],
    running: &mut Option<GatewayProcess>,
) -> String {
    match end(&mut child) {
        Ok(status) => format!("{cause}; it was stopped ({status}); it said: {}", said.join(" | ")),
        Err(error) => {
            let cause = format!(
                "{cause}; it could not be stopped: {error}; it said: {}", said.join(" | ")
            );
            *running = Some(GatewayProcess { _lease: lease, child, route: Err(cause.clone()) });
            cause
        }
    }
}

/// Both serving and failed-start children release their lease only after an
/// observed termination; a refusal retains the initiating and cleanup causes.
pub(crate) fn stop() -> Result<(), String> {
    let mut running = held();
    let Some(owned) = running.as_mut() else { return Ok(()); };
    end(&mut owned.child).map_err(|error| format!(
        "Jeden's own gateway process {} could not be stopped: {error}; recorded state: {}; its child and ownership lease remain held",
        owned.child.id(), owned.context()
    ))?;
    running.take();
    Ok(())
}
