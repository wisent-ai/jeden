use crate::sdk::{ApprovalRequest, ElicitationRequest, InteractionHandler};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use super::server::JsonWriter;

const INTERACTION_TIMEOUT: Duration = Duration::from_secs(300);

enum PendingInteraction {
    Elicitation(mpsc::SyncSender<Result<String, String>>),
    Approval(mpsc::SyncSender<Result<bool, String>>),
}

pub(super) struct RpcInteractionBridge {
    writer: JsonWriter,
    pending: Mutex<HashMap<String, PendingInteraction>>,
}

impl RpcInteractionBridge {
    pub(super) fn new(writer: JsonWriter) -> Arc<Self> {
        Arc::new(Self {
            writer,
            pending: Mutex::new(HashMap::new()),
        })
    }

    pub(super) fn resolve_elicitation(
        &self,
        token: &str,
        answer: Result<String, String>,
    ) -> Result<(), String> {
        let pending = self
            .pending
            .lock()
            .map_err(|_| "interaction lock poisoned")?
            .remove(token);
        match pending {
            Some(PendingInteraction::Elicitation(sender)) => sender
                .send(answer)
                .map_err(|_| "elicitation requester is gone".into()),
            Some(PendingInteraction::Approval(_)) => {
                Err("interaction token is for approval".into())
            }
            None => Err(format!("unknown interaction token: {token}")),
        }
    }

    pub(super) fn resolve_approval(
        &self,
        token: &str,
        answer: Result<bool, String>,
    ) -> Result<(), String> {
        let pending = self
            .pending
            .lock()
            .map_err(|_| "interaction lock poisoned")?
            .remove(token);
        match pending {
            Some(PendingInteraction::Approval(sender)) => sender
                .send(answer)
                .map_err(|_| "approval requester is gone".into()),
            Some(PendingInteraction::Elicitation(_)) => {
                Err("interaction token is for elicitation".into())
            }
            None => Err(format!("unknown interaction token: {token}")),
        }
    }

    pub(super) fn cancel_all(&self) -> Result<(), String> {
        let pending = self
            .pending
            .lock()
            .map_err(|_| "interaction lock poisoned")?
            .drain()
            .map(|(_, value)| value)
            .collect::<Vec<_>>();
        for interaction in pending {
            match interaction {
                PendingInteraction::Elicitation(sender) => {
                    let _ = sender.send(Err("server shutting down".into()));
                }
                PendingInteraction::Approval(sender) => {
                    let _ = sender.send(Err("server shutting down".into()));
                }
            }
        }
        Ok(())
    }

    fn remove_pending(&self, token: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(token);
        }
    }
}

impl InteractionHandler for RpcInteractionBridge {
    fn elicit(&self, request: ElicitationRequest) -> Result<String, String> {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.pending
            .lock()
            .map_err(|_| "interaction lock poisoned")?
            .insert(
                request.token.clone(),
                PendingInteraction::Elicitation(sender),
            );
        if let Err(error) = self.writer.send(&json!({"method":"session/request_input","params":{"token":request.token,"requestId":request.request_id,"question":request.question,"options":request.options}})) {
            self.remove_pending(&request.token);
            return Err(error);
        }
        let result = receiver
            .recv_timeout(INTERACTION_TIMEOUT)
            .map_err(|_| "elicitation timed out".to_string());
        self.remove_pending(&request.token);
        result?
    }

    fn approve(&self, request: ApprovalRequest) -> Result<bool, String> {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.pending
            .lock()
            .map_err(|_| "interaction lock poisoned")?
            .insert(request.token.clone(), PendingInteraction::Approval(sender));
        if let Err(error) = self.writer.send(&json!({"method":"session/request_permission","params":{"token":request.token,"requestId":request.request_id,"tool":request.tool,"detail":request.detail}})) {
            self.remove_pending(&request.token);
            return Err(error);
        }
        let result = receiver
            .recv_timeout(INTERACTION_TIMEOUT)
            .map_err(|_| "approval timed out".to_string());
        self.remove_pending(&request.token);
        result?
    }
}
