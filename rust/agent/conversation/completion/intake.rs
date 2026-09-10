//! Recording acceptance requirements for a retained request before any of it
//! is executed, and correcting one refused answer instead of stranding the
//! request behind a durable blocker.

use super::super::*;
use crate::completion::{self, IntakePlan};

const INTAKE: &str = include_str!("../../../completion/prompts/intake.txt");

impl Conversation {
    /// Plan every retained request that has no acceptance tasks yet.
    ///
    /// A refused intake — an answer that does not parse, or a plan the native
    /// controller rejects — is corrected once against the same request, with
    /// the exact refusal quoted back to a fresh read-only inspection. Before
    /// that, one malformed answer recorded `task_intake` as a durable blocker
    /// and ended the turn with the user's request retained but unplanned, so a
    /// single missing field stopped work that the next answer could plan.
    /// A second refusal is recorded and stops the turn: the correction is
    /// bounded, never a retry loop.
    pub(super) fn plan_pending_requests(
        &mut self,
        args: &Args,
        hooks: &RunHooks<'_>,
    ) -> Result<(), String> {
        loop {
            let state = completion::read_state(&self.recorder.path())?;
            let Some(request) = state
                .requests
                .iter()
                .find(|request| !request.planned && !request.paused)
            else {
                return Ok(());
            };
            let input = json!({
                "request": request,
                "retainedTasks": state.tasks,
                "workspace": args.cwd,
                "executionGrants": {"write": args.allow_write, "command": args.allow_command},
            });
            hooks.note("recording acceptance requirements before execution");
            let revision = state.revision;
            let request_id = request.id.clone();
            let session = self.recorder.path();
            let (planned, inspector) = self.corrected_inspection(
                args,
                "task_intake",
                INTAKE,
                &input,
                hooks,
                &mut |text| {
                    crate::protocol::extract_json_object(text)
                        .and_then(|object| {
                            serde_json::from_str::<IntakePlan>(object)
                                .map_err(|error| format!("invalid task intake: {error}"))
                        })
                        .and_then(|plan| {
                            completion::plan_request(&session, revision, &request_id, plan)
                        })
                },
            )?;
            self.recorder.record(
                "completion_review",
                json!({
                    "stage": "intake", "reviewerSession": inspector,
                    "revision": planned.revision,
                }),
            )?;
            self.publish_completion(&planned, hooks)?;
        }
    }
}
