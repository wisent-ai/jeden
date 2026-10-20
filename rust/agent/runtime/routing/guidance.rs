use std::path::Path;

/// The repository's remembered context for the prompt. A memory store that
/// cannot be opened or read is said: the turn then runs without the memory
/// the session would otherwise carry, and that is not the same as none.
pub(in crate::agent) fn memory_guidance_for_prompt(cwd: &Path) -> Option<String> {
    let store = match crate::memory::MemoryStore::open() {
        Ok(store) => store,
        Err(error) => {
            eprintln!(
                "jeden: memory is not in this prompt: the memory store cannot be opened: {error}"
            );
            return None;
        }
    };
    let scope = crate::memory::MemoryScope {
        kind: "repo".into(),
        id: cwd.display().to_string(),
    };
    let context = match store.pre_compaction_context(&scope, "", None) {
        Ok(context) => context,
        Err(error) => {
            eprintln!(
                "jeden: memory is not in this prompt: the memory of {} cannot be read: {error}",
                cwd.display()
            );
            return None;
        }
    };
    (!context.is_empty()).then_some(context)
}

pub(in crate::agent) fn env_usize(name: &str) -> Option<usize> {
    let value = match std::env::var(name) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return None,
        Err(std::env::VarError::NotUnicode(_)) => {
            eprintln!("jeden: {name} cannot be read: value is not Unicode");
            return None;
        }
    };
    match value.trim().parse::<std::num::NonZeroUsize>() {
        Ok(value) => Some(value.get()),
        Err(error) => {
            eprintln!("jeden: {name} must be a positive integer: {error}");
            None
        }
    }
}
