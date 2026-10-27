use super::control::{self, SharedControl};
use super::process::KernelProcess;
use super::KernelKey;
use crate::tool_runtime::runtime_ops::session::SessionSlot;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

pub(super) type KernelSlot = SessionSlot<Arc<KernelKey>, KernelProcess>;
struct Entry {
    slot: Arc<KernelSlot>,
    active: Option<SharedControl>,
}
static KERNELS: LazyLock<Mutex<HashMap<Arc<KernelKey>, Entry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) fn slot(key: KernelKey) -> Result<Arc<KernelSlot>, String> {
    let mut kernels = KERNELS.lock().map_err(|cause| format!("kernel registry lock poisoned: {cause}"))?;
    if let Some(entry) = kernels.get(&key) { return Ok(Arc::clone(&entry.slot)); }
    let key = Arc::new(key);
    let slot = KernelSlot::new(Arc::clone(&key), "interpreter scope");
    kernels.insert(key, Entry { slot: Arc::clone(&slot), active: None });
    Ok(slot)
}

fn require_current(kernels: &HashMap<Arc<KernelKey>, Entry>, slot: &Arc<KernelSlot>) -> Result<(), String> {
    match kernels.get(slot.key.as_ref()) {
        Some(entry) if Arc::ptr_eq(&entry.slot, slot) => Ok(()),
        _ => Err(format!("interpreter scope {:?} was torn down before evaluation", slot.key)),
    }
}
pub(super) fn check_current(slot: &Arc<KernelSlot>) -> Result<(), String> {
    let kernels = KERNELS.lock().map_err(|cause| format!("kernel registry lock poisoned: {cause}"))?;
    require_current(&kernels, slot)
}
pub(super) fn register(slot: &Arc<KernelSlot>, shared: &SharedControl) -> Result<(), String> {
    control::lock(shared)?.require_live()?;
    let mut kernels = KERNELS.lock().map_err(|cause| format!("kernel registry lock poisoned: {cause}"))?;
    require_current(&kernels, slot)?;
    let entry = kernels.get_mut(slot.key.as_ref()).expect("checked kernel scope disappeared while registry was locked");
    if entry.active.is_some() { return Err(format!("interpreter scope {:?} already has a registered process", slot.key)); }
    entry.active = Some(Arc::clone(shared));
    Ok(())
}

fn cleanup_registry() -> MutexGuard<'static, HashMap<Arc<KernelKey>, Entry>> {
    match KERNELS.lock() {
        Ok(kernels) => kernels,
        Err(poisoned) => {
            eprintln!("jeden: kernel registry lock poisoned during cleanup: {poisoned}; recovering its owned interpreters");
            poisoned.into_inner()
        }
    }
}
pub(super) fn finish(key: &KernelKey, shared: &SharedControl, cancelled: bool) {
    // Neither process signaling nor waiting holds the registry or a session lease lock.
    control::retire(shared, cancelled);
    let mut kernels = cleanup_registry();
    if let Some(entry) = kernels.get_mut(key) {
        if entry.active.as_ref().is_some_and(|active| Arc::ptr_eq(active, shared)) {
            entry.active = None;
        }
    }
}
pub(super) fn teardown(scope: &Path) {
    let removed: Vec<Entry> = {
        let mut kernels = cleanup_registry();
        let keys: Vec<_> = kernels.keys().filter(|key| key.scope == scope).cloned().collect();
        keys.iter().filter_map(|key| kernels.remove(key)).collect()
    };
    for entry in removed {
        let idle = entry.slot.close();
        if let Some(control) = entry.active { control::retire(&control, true); }
        drop(idle);
    }
}
