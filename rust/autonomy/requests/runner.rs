use super::super::JedenStageRunner;
use super::store::Store;
use pursuit::{Stage, StageResponse, StageRunner};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

const INITIAL_STAGE_INDEX: usize = 0;

#[derive(Serialize, Deserialize)]
struct Record {
    stage: String,
    prompt_sha256: String,
    session: PathBuf,
    text: Option<String>,
    error: Option<String>,
}

pub(super) struct DurableRunner<'a> {
    store: &'a Store,
    runner: JedenStageRunner,
    position: usize,
    pub indeterminate: bool,
}
impl<'a> DurableRunner<'a> {
    pub fn new(store: &'a Store, args: &crate::Args) -> Result<Self, String> {
        Ok(Self {
            store,
            runner: JedenStageRunner::new(args)?,
            position: INITIAL_STAGE_INDEX,
            indeterminate: false,
        })
    }
}

fn unresolved_effect(session: &std::path::Path) -> Result<bool, String> {
    let ledger = crate::session::store::read_events(session)?;
    let mut pending = 0usize;
    for event in ledger.events {
        match event.payload {
            crate::session::SessionPayloadV2::ToolCall(_) => pending += 1,
            crate::session::SessionPayloadV2::ToolResult(_) => {
                pending = pending
                    .checked_sub(1)
                    .ok_or("session has a tool result without its dispatch receipt")?;
            }
            _ => (),
        }
    }
    Ok(pending != 0 || ledger.recovered_truncated_tail)
}

impl StageRunner for DurableRunner<'_> {
    fn run(&mut self, stage: &Stage, prompt: &str) -> Result<StageResponse, String> {
        let position = self.position;
        self.position += 1;
        let digest = hex::encode(Sha256::digest(prompt.as_bytes()));
        let mut record = if let Some(record) = self.store.stage::<Record>(position)? {
            if record.stage != stage.label() || record.prompt_sha256 != digest {
                return Err("resume_contract_changed: the recorded stage input differs; old evidence was not rewritten".into());
            }
            if let Some(text) = &record.text {
                self.runner.restore(stage, &record.session)?;
                return Ok(StageResponse::new(text.clone(), Some(record.session)));
            }
            if !stage.read_only() && unresolved_effect(&record.session)? {
                self.indeterminate = true;
                return Err(format!("indeterminate_external_effect: stage {} in {} contains a dispatched tool with no complete result; no tool was replayed",record.stage,record.session.display()));
            }
            self.runner.restore(stage, &record.session)?;
            record
        } else {
            Record {
                stage: stage.label(),
                prompt_sha256: digest,
                session: self.runner.prepare(stage)?,
                text: None,
                error: None,
            }
        };
        self.store.record_stage(position, &record)?;
        let review_source = if matches!(stage, Stage::AcceptanceReview { .. }) {
            let request = self
                .store
                .get::<super::Request>("request")?
                .ok_or("request payload is missing")?;
            let revisions = super::evidence::snapshot(&request, true)?;
            Some((request, revisions))
        } else {
            None
        };
        match self.runner.run(stage, prompt) {
            Ok(response) => {
                if let Some((request, revisions)) = review_source {
                    if super::evidence::snapshot(&request, true)? != revisions {
                        let error =
                            "source changed during independent review; acceptance was not recorded"
                                .to_string();
                        record.error = Some(error.clone());
                        self.store.record_stage(position, &record)?;
                        return Err(error);
                    }
                    if serde_json::from_str::<pursuit::TaskVerdict>(&response.text)
                        .is_ok_and(|verdict| pursuit::verdict_accepted(&verdict))
                    {
                        self.store.set("reviewed_revisions", &revisions)?;
                    }
                }
                record.text = Some(response.text.clone());
                record.error = None;
                if let Some(session) = &response.session {
                    record.session = session.clone();
                }
                self.store.record_stage(position, &record)?;
                Ok(response)
            }
            Err(error) => {
                record.error = Some(error.clone());
                self.store.record_stage(position, &record)?;
                Err(error)
            }
        }
    }
}
