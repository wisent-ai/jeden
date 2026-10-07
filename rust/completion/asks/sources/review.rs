//! How a review's ask finds its place in the register: the ask it names by
//! id, else the ask asked in the same words, else a new one.

use super::super::{same_words, update, AskOccurrence, AskRegister, AskSource, OperatorAsk};

/// Where one blocked task's ask goes in the register.
#[derive(Debug, Clone)]
pub(crate) struct Link {
    pub ask_id: String,
    /// The words the task records: the register's own for an ask it already
    /// holds, so every task waiting on it shows the same sentence.
    pub ask: String,
    pub wording: String,
    pub task_id: String,
    pub workspace: String,
    /// Absent when the ask is already in the register.
    pub new: Option<NewAsk>,
}

#[derive(Debug, Clone)]
pub(crate) struct NewAsk {
    pub follows: Option<String>,
}

/// Resolves the asks of one review against the register as it was read
/// before the review, plus the asks this review is adding.
pub(crate) struct Linker {
    register: AskRegister,
    pub links: Vec<Link>,
}

impl Linker {
    pub fn new(register: AskRegister) -> Self {
        Self {
            register,
            links: Vec::new(),
        }
    }

    /// `failed_after(stamp)` says whether the task's cited evidence holds an
    /// operation that failed after `stamp`.
    pub fn resolve(
        &mut self,
        task_id: &str,
        workspace: &str,
        ask_id: Option<&str>,
        wording: &str,
        failed_after: &mut dyn FnMut(&str) -> Result<bool, String>,
    ) -> Result<(), String> {
        let pending = self.links.iter().find(|link| {
            link.new.is_some()
                && (ask_id == Some(link.ask_id.as_str()) || same_words(&link.ask, wording))
        });
        if let Some(link) = pending {
            let link = Link {
                task_id: task_id.into(),
                wording: wording.into(),
                new: None,
                ..link.clone()
            };
            self.links.push(link);
            return Ok(());
        }
        let existing = match ask_id {
            Some(id) => Some(self.register.find(id).ok_or_else(|| {
                format!(
                    "task {task_id} names ask {id}, which is not in the operator ask register; name an askId from operatorAsks or leave it out to record a new ask"
                )
            })?),
            None => self
                .register
                .asks
                .iter()
                .filter(|ask| {
                    same_words(&ask.ask, wording)
                        || ask
                            .occurrences
                            .iter()
                            .any(|occurrence| same_words(&occurrence.wording, wording))
                })
                .max_by_key(|ask| (ask.answer.is_none(), ask.asked_at.parse::<u64>().unwrap_or_default())),
        };
        let link = match existing {
            Some(ask) => match &ask.answer {
                Some(answer) if !failed_after(&answer.answered_at)? => {
                    return Err(format!(
                        "task {task_id} asks the operator again for what ask {} already holds an answer to: {} The operator answered at {}: {} No operation failed since, so the work continues with that answer",
                        ask.id, ask.ask, answer.answered_at, answer.text
                    ));
                }
                Some(_) => Link {
                    ask_id: uuid::Uuid::new_v4().to_string(),
                    ask: wording.into(),
                    wording: wording.into(),
                    task_id: task_id.into(),
                    workspace: workspace.into(),
                    new: Some(NewAsk {
                        follows: Some(ask.id.clone()),
                    }),
                },
                None => Link {
                    ask_id: ask.id.clone(),
                    ask: ask.ask.clone(),
                    wording: wording.into(),
                    task_id: task_id.into(),
                    workspace: workspace.into(),
                    new: None,
                },
            },
            None => Link {
                ask_id: uuid::Uuid::new_v4().to_string(),
                ask: wording.into(),
                wording: wording.into(),
                task_id: task_id.into(),
                workspace: workspace.into(),
                new: Some(NewAsk { follows: None }),
            },
        };
        self.links.push(link);
        Ok(())
    }

    pub fn link_for(&self, task_id: &str) -> Option<&Link> {
        self.links.iter().find(|link| link.task_id == task_id)
    }

    /// Records the links once the session accepted the review. A task
    /// already recorded on the same ask is not counted twice: the register
    /// counts the places an ask was put, not the reviews that repeated it.
    pub fn commit(self, session: &str) -> Result<(), String> {
        if self.links.is_empty() {
            return Ok(());
        }
        let stamp = crate::agent::now_stamp();
        update(|register| {
            for link in &self.links {
                if let Some(new) = &link.new {
                    if register.find(&link.ask_id).is_none() {
                        register.asks.push(OperatorAsk {
                            id: link.ask_id.clone(),
                            ask: link.ask.clone(),
                            workspace: link.workspace.clone(),
                            asked_at: stamp.clone(),
                            follows: new.follows.clone(),
                            occurrences: Vec::new(),
                            answer: None,
                        });
                    }
                }
                if register
                    .for_task(session, &link.task_id)
                    .is_some_and(|ask| ask.id == link.ask_id)
                {
                    continue;
                }
                let ask = register
                    .asks
                    .iter_mut()
                    .find(|ask| ask.id == link.ask_id)
                    .ok_or_else(|| {
                        format!("ask {} left the register during the review", link.ask_id)
                    })?;
                ask.occurrences.push(AskOccurrence {
                    asked_at: stamp.clone(),
                    source: AskSource::Review,
                    session_path: session.into(),
                    task_id: Some(link.task_id.clone()),
                    wording: link.wording.clone(),
                    answered_from_register: false,
                });
            }
            Ok(())
        })
        .map(|_| ())
    }
}
