mod client;

pub mod protocol;

mod session;
mod types;

pub use client::{ClientError, EventStream, SessionClient, SessionTransport, TransportError};
pub use session::{AgentSession, EventSubscription};
pub use types::{
    ApprovalRequest, Capabilities, ElicitationRequest, InteractionHandler, PromptRequest,
    PromptResult, RpcErrorData, SessionEvent, SessionEventKind, SessionOptions,
};
