//! The standing rules an operator states, kept so they hold in every later
//! turn of every session, whatever the session forgot. The model a turn runs
//! on reads each operator message, in the background, for instructions or
//! corrections about how the agent must work from now on; each one is kept,
//! verbatim, as a `rule` memory of the operator's scope, and every turn's
//! system prompt carries the operator's kept rules (`section`). What a reading
//! kept, or why it kept nothing, is a `memory_mutation` ledger entry with
//! `source: operator_rules`, and the turn proceeds either way.
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::memory::{MemoryScope, MemorySource, MemoryStore};
use crate::model_router::{chat_completion, ChatConfig};

const SYSTEM_PROMPT: &str = include_str!("prompt.txt");
const KIND: &str = "rule";

/// The scope every operator rule is kept under: the operator of this home,
/// across repositories and sessions.
fn scope() -> MemoryScope {
    MemoryScope {
        kind: "operator".into(),
        id: "operator".into(),
    }
}

/// Read `prompt` for standing rules and keep each one, without blocking the
/// turn. What was kept, or why nothing could be read, goes to the ledger.
pub(crate) fn spawn_reading(
    router: ChatConfig,
    prompt: String,
    session_dir: PathBuf,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let session_id = session_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        match read(&router, &prompt).and_then(|rules| keep(&rules, session_id).map(|()| rules)) {
            Ok(rules) if rules.is_empty() => {}
            Ok(rules) => recorded(
                &session_dir,
                json!({ "source": "operator_rules", "kept": true, "rules": rules }),
            ),
            Err(reason) => recorded(
                &session_dir,
                json!({ "source": "operator_rules", "kept": false, "model": router.model, "reason": reason }),
            ),
        }
    })
}

/// The standing rules `prompt` states, as the turn's own model reads them.
fn read(router: &ChatConfig, prompt: &str) -> Result<Vec<String>, String> {
    let mut router = router.clone();
    router.fallbacks.clear();
    let messages = vec![
        json!({ "role": "system", "content": SYSTEM_PROMPT }),
        json!({ "role": "user", "content": prompt }),
    ];
    let completion = chat_completion(&router, messages, None, &[]).map_err(|error| {
        format!(
            "{} did not read the operator's rules: {error}",
            router.model
        )
    })?;
    let value: Value = serde_json::from_str(completion.content.trim()).map_err(|error| {
        format!(
            "{} answered no rule list ({error}): {}",
            router.model, completion.content
        )
    })?;
    let rules = value
        .get("rules")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!(
                "{} answered no rule list: {}",
                router.model, completion.content
            )
        })?;
    rules
        .iter()
        .map(|rule| {
            rule.as_str()
                .map(str::trim)
                .filter(|rule| !rule.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("{} answered a rule that is not text: {rule}", router.model))
        })
        .collect()
}

/// Keep each rule as a `rule` memory of the operator's scope; a rule kept
/// before is kept once, by its text.
fn keep(rules: &[String], session_id: Option<String>) -> Result<(), String> {
    if rules.is_empty() {
        return Ok(());
    }
    let store = MemoryStore::open()?;
    let source = MemorySource {
        origin: "operator_message".into(),
        session_id,
        entry_id: None,
    };
    for rule in rules {
        store.remember(
            KIND,
            &scope(),
            rule,
            &["operator-rule".into()],
            &source,
            1.0,
        )?;
    }
    Ok(())
}

/// The operator's kept rules, for the system prompt of every turn. A store
/// that cannot be read is said there and on standard error: a turn that runs
/// without the operator's rules must not look like one that has none.
pub(crate) fn section() -> String {
    let kept =
        MemoryStore::open().and_then(|store| store.pre_compaction_context(&scope(), "", None));
    match kept {
        Ok(rules) if rules.trim().is_empty() => String::new(),
        Ok(rules) => format!(
            "\n\nOperator rules (stated by the operator in earlier messages; binding in every turn, \
             as the operator contracts are):\n{rules}"
        ),
        Err(cause) => {
            eprintln!("jeden: the operator's rules are not in this prompt: {cause}");
            format!("\n\nOperator rules could not be read for this turn: {cause}")
        }
    }
}

/// Append the reading's `memory_mutation` entry to the session ledger, saying
/// an entry that cannot be dated or written.
fn recorded(session_dir: &std::path::Path, data: Value) {
    let written = crate::agent::now_stamp()
        .map_err(|error| format!("it cannot be dated: {error}"))
        .and_then(|at| {
            crate::cli::sessions::append_ledger_entry(session_dir, at, "memory_mutation", data)
        });
    if let Err(cause) = written {
        eprintln!(
            "jeden: the operator rule reading is not in {}: {cause}",
            session_dir.display()
        );
    }
}
