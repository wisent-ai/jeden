//! The registry read once, by [`Registry::resolve`], and walked once, by
//! [`Registry::declared`], for both the hooks a turn runs and the `/hooks`
//! summary: a registry or a registration either of them cannot use is
//! refused the same way by both, never run as a refusal by one and left out
//! of the other.

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

use super::super::command::{entry_command, unrunnable_reason, EntryCommand};
use super::super::{map_event, registry_path, TamaHook};
use crate::hooks::{read_config, Hook};

/// The resolved registry: its file, who manages it, the catalog naming each
/// hook's source, and its events table.
pub(super) struct Registry {
    pub(super) path: PathBuf,
    pub(super) managed_by: Option<String>,
    catalog: Value,
    events: Map<String, Value>,
}

/// One registry event Jeden fires, with every registration it declares.
pub(super) struct Declared<'a> {
    pub(super) tama_event: &'a str,
    pub(super) event: &'static str,
    pub(super) matcher: String,
    pub(super) blocking: bool,
    pub(super) hooks: Vec<TamaHook>,
}

/// A registration that cannot run: it blocks, and its reason is the answer.
pub(super) fn refused(id: String, matcher: String, reason: String) -> TamaHook {
    TamaHook {
        id,
        hook: Hook {
            matcher,
            command: String::new(),
        },
        blocking: true,
        unrunnable: Some(reason),
    }
}

/// A `blocking` flag: unstated is `inherited`; anything but a boolean is
/// neither blocking nor recording, and is refused naming `what` stated it.
fn blocking_flag(stated: Option<&Value>, inherited: bool, what: &str) -> Result<bool, String> {
    match stated {
        None => Ok(inherited),
        Some(Value::Bool(blocking)) => Ok(*blocking),
        Some(other) => Err(format!(
            "{what} states blocking {other}, which is not true or false"
        )),
    }
}

/// The ids an event's hooks declare, for naming hooks that do not run here.
fn hook_ids(spec: &Value) -> Vec<&str> {
    spec.get("hooks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .collect()
}

impl Registry {
    /// `Ok(None)` when no registry applies. A registry that is named but
    /// absent, cannot be read, is not JSON or holds no events table is
    /// refused naming the file.
    pub(super) fn resolve(cwd: &Path) -> Result<Option<Self>, String> {
        let Some(path) = registry_path(cwd)? else {
            return Ok(None);
        };
        let mut document = read_config(&path)?;
        let Some(Value::Object(events)) = document.get_mut("events").map(Value::take) else {
            return Err(format!(
                "the Tama registry {} has no events table, so no Tama hook can run",
                path.display()
            ));
        };
        // Without a catalog, each entry's `command` alone names its program.
        let catalog = match document.get_mut("catalog") {
            Some(catalog) => catalog.take(),
            None => Value::Null,
        };
        let managed_by = document
            .get("managedBy")
            .and_then(Value::as_str)
            .map(str::to_string);
        Ok(Some(Self {
            path,
            managed_by,
            catalog,
            events,
        }))
    }

    /// Every event Jeden fires with its registrations, and the registry
    /// events Jeden has no event for with the ids of their hooks.
    pub(super) fn declared(&self) -> (Vec<Declared<'_>>, Vec<(&str, Vec<&str>)>) {
        let mut declared = Vec::new();
        let mut unmapped = Vec::new();
        for (tama_event, spec) in &self.events {
            let Some((event, matcher)) = map_event(tama_event) else {
                unmapped.push((tama_event.as_str(), hook_ids(spec)));
                continue;
            };
            let named = format!("{}#{tama_event}", self.path.display());
            let what = format!(
                "the Tama registry {} event {tama_event}",
                self.path.display()
            );
            let mut hooks = Vec::new();
            let blocking = match blocking_flag(spec.get("blocking"), false, &what) {
                Ok(blocking) => blocking,
                Err(reason) => {
                    hooks.push(refused(named.clone(), matcher.clone(), reason));
                    true
                }
            };
            match spec.get("hooks") {
                None => {}
                Some(Value::Array(entries)) => {
                    for entry in entries {
                        if let Some(hook) = self.registration(entry, &named, &matcher, blocking) {
                            hooks.push(hook);
                        }
                    }
                }
                Some(other) => hooks.push(refused(
                    named.clone(),
                    matcher.clone(),
                    format!("{what} declares hooks that are not a list: {other}"),
                )),
            }
            declared.push(Declared {
                tama_event,
                event,
                matcher,
                blocking,
                hooks,
            });
        }
        (declared, unmapped)
    }

    /// One entry as the hook it registers; `None` for an entry kind that does
    /// not run natively here.
    fn registration(
        &self,
        entry: &Value,
        named: &str,
        matcher: &str,
        event_blocking: bool,
    ) -> Option<TamaHook> {
        let resolved = entry_command(entry, &self.path, &self.catalog)?;
        let Some(id) = entry
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            return Some(refused(
                named.to_string(),
                matcher.to_string(),
                format!("{named} registers a hook without an id: {entry}"),
            ));
        };
        let what = format!("the Tama registry {} hook {id}", self.path.display());
        let blocking = match blocking_flag(entry.get("blocking"), event_blocking, &what) {
            Ok(blocking) => blocking,
            Err(reason) => return Some(refused(id.to_string(), matcher.to_string(), reason)),
        };
        let (command, unrunnable) = match resolved {
            EntryCommand::Runnable(command) => (command, None),
            EntryCommand::Missing { written, tried } => {
                let reason = unrunnable_reason(id, &written, &tried, &self.path);
                (written, Some(reason))
            }
        };
        Some(TamaHook {
            id: id.to_string(),
            hook: Hook {
                matcher: matcher.to_string(),
                command,
            },
            blocking,
            unrunnable,
        })
    }
}
