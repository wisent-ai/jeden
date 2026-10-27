use super::control::{self, SharedControl};
use super::process::PtyProcess;
use crate::tool_runtime::runtime_ops::session::SessionSlot;
use super::{OperationContext, PtyError, PtySessionState};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

type ScopeSlot = SessionSlot<Arc<Path>, PtyProcess>;

struct ScopeEntry {
    slot: Arc<ScopeSlot>,
    active: Option<SharedControl>,
}

struct LiveSession {
    scope: Arc<Path>,
    control: SharedControl,
}

#[derive(Default)]
struct PtyRegistry {
    scopes: HashMap<Arc<Path>, ScopeEntry>,
    live: HashMap<String, LiveSession>,
    terminal: HashMap<String, PtySessionState>,
}

static SESSIONS: LazyLock<Mutex<PtyRegistry>> =
    LazyLock::new(|| Mutex::new(PtyRegistry::default()));

pub(super) fn scope(key: &Path) -> Result<Arc<ScopeSlot>, String> {
    let mut registry = SESSIONS.lock().map_err(|_| "PTY registry lock poisoned")?;
    if let Some(entry) = registry.scopes.get(key) {
        return Ok(Arc::clone(&entry.slot));
    }
    let key: Arc<Path> = Arc::from(key);
    let slot = ScopeSlot::new(Arc::clone(&key), "PTY scope");
    registry.scopes.insert(key, ScopeEntry {
        slot: Arc::clone(&slot),
        active: None,
    });
    Ok(slot)
}

fn require_current(registry: &PtyRegistry, slot: &Arc<ScopeSlot>) -> Result<(), String> {
    match registry.scopes.get(slot.key.as_ref()) {
        Some(entry) if Arc::ptr_eq(&entry.slot, slot) => Ok(()),
        _ => Err(format!(
            "PTY scope '{}' was torn down before terminal execution",
            slot.key.display()
        )),
    }
}

pub(super) fn check_current(slot: &Arc<ScopeSlot>) -> Result<(), String> {
    let registry = SESSIONS.lock().map_err(|_| "PTY registry lock poisoned")?;
    require_current(&registry, slot)
}

/// Publish control before the shell handshake can block, so teardown can kill it.
pub(super) fn register(slot: &Arc<ScopeSlot>, shared: &SharedControl) -> Result<(), String> {
    let id = {
        let control = control::lock(shared)?;
        control.require_live().map_err(|error| error.to_string())?;
        control.metadata.session_id.clone()
    };
    let mut registry = SESSIONS.lock().map_err(|_| "PTY registry lock poisoned")?;
    require_current(&registry, slot)?;
    if registry.live.contains_key(&id) || registry.terminal.contains_key(&id) {
        return Err(format!("PTY session '{id}' is already registered"));
    }
    let entry = registry.scopes.get_mut(slot.key.as_ref())
        .expect("checked PTY scope disappeared while registry was locked");
    if entry.active.is_some() {
        return Err(format!("PTY scope '{}' already has a registered shell", slot.key.display()));
    }
    entry.active = Some(Arc::clone(shared));
    registry.live.insert(id, LiveSession {
        scope: Arc::clone(&slot.key),
        control: Arc::clone(shared),
    });
    Ok(())
}

pub(super) fn session(
    context: &OperationContext<'_>,
    session_id: &str,
) -> Result<SharedControl, PtyError> {
    if context.cancellation().is_cancelled() {
        return Err(PtyError::OperationCancelled);
    }
    let registry = SESSIONS.lock()
        .map_err(|_| PtyError::System("PTY registry lock poisoned".into()))?;
    if context.cancellation().is_cancelled() {
        return Err(PtyError::OperationCancelled);
    }
    if let Some(session) = registry.live.get(session_id) {
        return Ok(Arc::clone(&session.control));
    }
    Err(match registry.terminal.get(session_id) {
        Some(PtySessionState::Ended) => PtyError::SessionEnded { session_id: session_id.into() },
        Some(PtySessionState::Cancelled) => PtyError::SessionCancelled { session_id: session_id.into() },
        _ => PtyError::SessionNotFound { session_id: session_id.into() },
    })
}

impl PtyRegistry {
    fn retire(&mut self, id: String, state: PtySessionState) {
        if let Some(session) = self.live.remove(&id) {
            if let Some(entry) = self.scopes.get_mut(session.scope.as_ref()) {
                if entry.active.as_ref().is_some_and(|active| Arc::ptr_eq(active, &session.control)) {
                    entry.active = None;
                }
            }
        }
        self.terminal.insert(id, state);
    }
}

fn cleanup_registry() -> MutexGuard<'static, PtyRegistry> {
    match SESSIONS.lock() {
        Ok(registry) => registry,
        Err(poisoned) => {
            eprintln!("jeden: PTY registry lock poisoned during terminal cleanup: {poisoned}; recovering its owned sessions to terminate them");
            poisoned.into_inner()
        }
    }
}

pub(super) fn finish(shared: &SharedControl, state: PtySessionState) {
    // No registry lock is held while waiting for control or native termination.
    let (id, state) = {
        let mut control = match shared.lock() {
            Ok(control) => control,
            Err(poisoned) => {
                eprintln!("jeden: PTY control lock poisoned during terminal cleanup: {poisoned}; recovering its owned process to terminate it");
                poisoned.into_inner()
            }
        };
        let state = control.terminate(state);
        (control.metadata.session_id.clone(), state)
    };
    cleanup_registry().retire(id, state);
}

pub(super) fn teardown(scope: &Path) {
    // Drop the scope's command/process ownership only after releasing the registry.
    let entry = cleanup_registry().scopes.remove(scope);
    if let Some(entry) = entry {
        let idle = entry.slot.close();
        if let Some(control) = entry.active {
            finish(&control, PtySessionState::Cancelled);
        }
        drop(idle);
    }
}
