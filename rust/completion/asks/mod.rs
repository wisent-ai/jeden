//! The register of everything Jeden has asked the operator, across every
//! session of this machine, and what the operator answered.
//!
//! A task's own `operatorRequest` lives in one session. Without a register
//! the next session, or the next task of the same session, asks the operator
//! for the same value again, worded a little differently each time, and the
//! operator cannot see that he was already asked or what he said. The
//! register is that memory: each ask is recorded once with every place it was
//! asked, an answer is recorded once and delivered to every task waiting on
//! it, and a review that asks again for something already answered is
//! refused.

mod answer;
pub(crate) mod cli;
mod oko;
mod sources;
mod store;

pub(crate) use answer::{answer, deliver_answers};
pub(crate) use oko::put_waiting as put_waiting_on_oko;
pub(crate) use sources::ask_user;
pub(crate) use sources::review::{Link, Linker};
pub(crate) use store::{path as register_path, read, update};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

use super::model::{CompletionState, OperatorAnswer};

pub(crate) const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AskRegister {
    pub schema_version: u32,
    pub revision: u64,
    pub asks: Vec<OperatorAsk>,
}

/// One thing the operator was asked for, in the words it was first asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OperatorAsk {
    pub id: String,
    pub ask: String,
    /// The workspace of the session that asked first.
    pub workspace: String,
    pub asked_at: String,
    /// The earlier ask this one repeats because an operation failed after
    /// the operator answered it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follows: Option<String>,
    pub occurrences: Vec<AskOccurrence>,
    #[serde(default)]
    pub answer: Option<OperatorAnswer>,
    /// The same ask put where the operator looks, in Oko; absent for an ask
    /// answered on the spot (`ask_user`) or recorded before Oko asks existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oko: Option<oko::OkoAsk>,
}

/// One place the ask was put to the operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AskOccurrence {
    pub asked_at: String,
    pub source: AskSource,
    pub session_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// The words used this time, which may differ from the first ones.
    pub wording: String,
    /// An `ask_user` question the register already held an answer to: the
    /// recorded answer was returned and the operator was not asked again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub answered_from_register: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AskSource {
    /// An independent review blocked a task on something only the operator
    /// holds.
    Review,
    /// The execution agent asked a question in an interactive session.
    AskUser,
}

/// Two asks are the same ask when their words are the same apart from case,
/// spacing and closing punctuation.
pub(crate) fn same_words(left: &str, right: &str) -> bool {
    normalized(left) == normalized(right)
}

fn normalized(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(['.', '!', '?', ':', ';', ' '])
        .to_lowercase()
}

pub(crate) fn session_key(session: &Path) -> String {
    std::fs::canonicalize(session)
        .unwrap_or_else(|_| session.to_path_buf())
        .display()
        .to_string()
}

impl AskRegister {
    pub fn find(&self, id: &str) -> Option<&OperatorAsk> {
        self.asks.iter().find(|ask| ask.id == id)
    }

    /// The ask a task of a session waits on: the one whose latest
    /// occurrence names that task.
    pub fn for_task(&self, session: &str, task_id: &str) -> Option<&OperatorAsk> {
        self.asks
            .iter()
            .filter_map(|ask| {
                ask.occurrences
                    .iter()
                    .filter(|occurrence| {
                        occurrence.session_path == session
                            && occurrence.task_id.as_deref() == Some(task_id)
                    })
                    .map(|occurrence| occurrence.asked_at.parse::<u64>().unwrap_or_default())
                    .max()
                    .map(|stamp| (stamp, ask))
            })
            .max_by_key(|(stamp, _)| *stamp)
            .map(|(_, ask)| ask)
    }

    /// What the models read: every ask with its answer, so neither the
    /// execution agent nor a review asks again for something already given.
    pub fn context(&self) -> Vec<Value> {
        self.asks
            .iter()
            .map(|ask| {
                let mut value = json!({
                    "askId": ask.id,
                    "ask": ask.ask,
                    "askedAt": ask.asked_at,
                    "timesAsked": ask.occurrences.len(),
                    "workspace": ask.workspace,
                });
                if let Some(answer) = &ask.answer {
                    value["answer"] =
                        json!({"answeredAt": answer.answered_at, "text": answer.text});
                }
                value
            })
            .collect()
    }
}

impl OperatorAsk {
    /// One line a person reads in a stop message or a task list.
    pub fn waiting_line(&self) -> String {
        format!(
            "Waiting on you: {} (ask {}; asked {} since {}) Answer with: jeden asks answer {} --text <answer>",
            self.ask,
            self.id,
            times(self.occurrences.len()),
            self.asked_at,
            self.id
        )
    }
}

pub(crate) fn times(count: usize) -> String {
    if count == 1 {
        "once".into()
    } else {
        format!("{count} times")
    }
}

/// Every ask a session's tasks wait on, once each however many tasks wait on
/// it, with the command that answers it.
pub(crate) fn open_asks(state: &CompletionState, session: &Path) -> Result<Vec<String>, String> {
    let register = read()?;
    let session = session_key(session);
    let mut seen = Vec::new();
    let mut lines = Vec::new();
    for task in state.tasks.iter().filter(|task| task.waits_for_operator()) {
        match register.for_task(&session, &task.id) {
            Some(ask) if !seen.contains(&ask.id) => {
                seen.push(ask.id.clone());
                lines.push(ask.waiting_line());
            }
            Some(_) => {}
            None => {
                if let Some(request) = &task.operator_request {
                    lines.push(format!(
                        "Waiting on you: {} Answer with: jeden todo answer {} --text <answer> --revision {}",
                        request.ask, task.id, state.revision
                    ));
                }
            }
        }
    }
    Ok(lines)
}
