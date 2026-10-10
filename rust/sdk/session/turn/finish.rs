use super::*;

impl AgentSession {
    pub(super) fn finish_prompt(
        &self,
        request_id: String,
        result: Result<PromptResult, String>,
    ) -> Result<PromptResult, String> {
        let result = match result {
            Err(message) => {
                match self.inner.emit(SessionEvent {
                    request_id: request_id.clone(),
                    event: SessionEventKind::Error {
                        message: message.clone(),
                    },
                }) {
                    Ok(()) => Err(message),
                    Err(error) => Err(format!(
                        "{message}; cannot publish request failure event: {error}"
                    )),
                }
            }
            result => result,
        };
        match self.inner.active.lock() {
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
        }
    }
}
