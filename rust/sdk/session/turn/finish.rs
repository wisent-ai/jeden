use super::*;

impl AgentSession {
    pub(super) fn finish_prompt(
        &self,
        request_id: String,
        result: Result<PromptResult, String>,
    ) -> Result<PromptResult, String> {
        let result = match self.inner.active.lock() {
            Ok(mut active) => {
                active.remove(&request_id);
                result
            }
            Err(error) => {
                let cause = format!(
                    "cannot remove completed request {request_id} from active requests: {error}"
                );
                Err(match result {
                    Ok(_) => cause,
                    Err(initiating) => format!("{initiating}; {cause}"),
                })
            }
        };
        if let Err(message) = &result {
            if let Err(error) = self.inner.emit(SessionEvent {
                request_id,
                event: SessionEventKind::Error {
                    message: message.clone(),
                },
            }) {
                return Err(format!(
                    "{message}; cannot publish request failure event: {error}"
                ));
            }
        }
        result
    }
}
