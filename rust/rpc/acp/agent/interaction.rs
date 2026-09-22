//! Asking the editor on the other end a question on behalf of a running
//! turn, and refusing when it cannot be asked.
//!
//! Split out of `rpc/acp/agent.rs`, which had grown past the module line cap.

use crate::sdk::{ApprovalRequest as JedenApprovalRequest, ElicitationRequest, InteractionHandler};
use crate::tool_runtime::runtime_ops::CancellationToken;
use agent_client_protocol::schema::v1::*;
use agent_client_protocol::{Client, ConnectionTo};
use futures::executor::block_on;
use serde_json::json;

pub(super) struct AcpInteraction {
    pub(super) session_id: String,
    pub(super) client: ConnectionTo<Client>,
    pub(super) cancellation: CancellationToken,
    pub(super) input_supported: bool,
}

impl AcpInteraction {
    fn ready(&self) -> Result<(), String> {
        if self.cancellation.is_cancelled() {
            return Err("ACP interaction cancelled".into());
        }
        Ok(())
    }
}

impl InteractionHandler for AcpInteraction {
    fn elicit(&self, request: ElicitationRequest) -> Result<String, String> {
        self.ready()?;
        if !self.input_supported {
            return Err("unsupported ACP client capability: elicitation.form".into());
        }
        let mut property = StringPropertySchema::new().title(request.question.clone());
        if !request.options.is_empty() {
            property = property.enum_values(request.options.clone());
        }
        let schema = ElicitationSchema::new().property("answer", property, true);
        let mode = ElicitationFormMode::new(
            ElicitationSessionScope::new(self.session_id.clone()),
            schema,
        );
        let response = block_on(
            self.client
                .send_request(CreateElicitationRequest::new(mode, request.question))
                .block_task(),
        )
        .map_err(|error| error.to_string())?;
        self.ready()?;
        match response.action {
            ElicitationAction::Accept(accepted) => {
                let content = accepted
                    .content
                    .ok_or_else(|| "ACP elicitation accepted without content".to_string())?;
                let answer = content
                    .get("answer")
                    .ok_or_else(|| "ACP elicitation response omitted answer".to_string())?;
                serde_json::to_value(answer)
                    .map_err(|error| error.to_string())?
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "ACP elicitation answer must be a string".to_string())
            }
            ElicitationAction::Decline => Err("ACP elicitation declined".into()),
            ElicitationAction::Cancel => Err("ACP elicitation cancelled".into()),
            _ => Err("unsupported ACP elicitation response action".into()),
        }
    }

    fn approve(&self, request: JedenApprovalRequest) -> Result<bool, String> {
        self.ready()?;
        let tool_call = ToolCallUpdate::new(
            request.token,
            ToolCallUpdateFields::new()
                .title(request.tool)
                .status(ToolCallStatus::Pending)
                .raw_input(json!({"detail": request.detail})),
        );
        let options = vec![
            PermissionOption::new("allow-once", "Allow once", PermissionOptionKind::AllowOnce),
            PermissionOption::new("reject-once", "Reject", PermissionOptionKind::RejectOnce),
        ];
        let response = block_on(
            self.client
                .send_request(RequestPermissionRequest::new(
                    self.session_id.clone(),
                    tool_call,
                    options,
                ))
                .block_task(),
        )
        .map_err(|error| error.to_string())?;
        self.ready()?;
        Ok(matches!(
            response.outcome,
            RequestPermissionOutcome::Selected(selected) if selected.option_id.0.as_ref() == "allow-once"
        ))
    }
}
