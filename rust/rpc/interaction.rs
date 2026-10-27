use crate::panicked::held;
use crate::sdk::{ApprovalRequest, ElicitationRequest, InteractionHandler};
use serde_json::json;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::{mpsc, Arc, Mutex};

use super::server::JsonWriter;

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
        // The pending map changes by one insert or remove at a time, so a
        // holder that panicked left it whole; refused, one panic would strand
        // every later question and approval of the server.
        let pending = held(&self.pending).remove(token);
        match pending {
            Some(PendingInteraction::Elicitation(sender)) => sender
                .send(answer)
                .map_err(|_answer| "elicitation requester is gone".into()),
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
        let pending = held(&self.pending).remove(token);
        match pending {
            Some(PendingInteraction::Approval(sender)) => sender
                .send(answer)
                .map_err(|_answer| "approval requester is gone".into()),
            Some(PendingInteraction::Elicitation(_)) => {
                Err("interaction token is for elicitation".into())
            }
            None => Err(format!("unknown interaction token: {token}")),
        }
    }

    pub(super) fn cancel_all(&self) -> Result<(), String> {
        let pending = held(&self.pending)
            .drain()
            .map(|(_, value)| value)
            .collect::<Vec<_>>();
        for interaction in pending {
            match interaction {
                PendingInteraction::Elicitation(sender) => {
                    let _unreceived = sender.send(Err("server shutting down".into()));
                }
                PendingInteraction::Approval(sender) => {
                    let _unreceived = sender.send(Err("server shutting down".into()));
                }
            }
        }
        Ok(())
    }

    fn remove_pending(&self, token: &str) {
        held(&self.pending).remove(token);
    }
}

impl InteractionHandler for RpcInteractionBridge {
    fn elicit(&self, request: ElicitationRequest) -> Result<String, String> {
        // One answer, so the asker never blocks on handing it over.
        let (sender, receiver) = mpsc::sync_channel(NonZeroUsize::MIN.get());
        held(&self.pending).insert(
            request.token.clone(),
            PendingInteraction::Elicitation(sender),
        );
        if let Err(error) = self.writer.send(&json!({"method":"session/request_input","params":{"token":request.token,"requestId":request.request_id,"question":request.question,"options":request.options}})) {
            self.remove_pending(&request.token);
            return Err(error);
        }
        let result = receiver
            .recv()
            .map_err(|error| format!("elicitation channel closed: {error}"));
        self.remove_pending(&request.token);
        result?
    }

    fn approve(&self, request: ApprovalRequest) -> Result<bool, String> {
        let (sender, receiver) = mpsc::sync_channel(NonZeroUsize::MIN.get());
        held(&self.pending).insert(request.token.clone(), PendingInteraction::Approval(sender));
        if let Err(error) = self.writer.send(&json!({"method":"session/request_permission","params":{"token":request.token,"requestId":request.request_id,"tool":request.tool,"detail":request.detail}})) {
            self.remove_pending(&request.token);
            return Err(error);
        }
        let result = receiver
            .recv()
            .map_err(|error| format!("approval channel closed: {error}"));
        self.remove_pending(&request.token);
        result?
    }
}
