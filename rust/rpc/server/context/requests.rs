//! Context request lifetimes belong to their RPC connection. Registration
//! happens before spawning, so a subsequent cancel cannot race worker startup.
use std::collections::{hash_map::Entry, HashMap};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::Value;

use crate::tool_runtime::runtime_ops::CancellationToken;

type Failure = (&'static str, String);

#[derive(Default)]
pub(in crate::rpc::server) struct Requests {
    active: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

pub(in crate::rpc::server) struct Lease {
    active: Arc<Mutex<HashMap<String, CancellationToken>>>,
    key: String,
}

fn key(id: &Value) -> Result<String, Failure> {
    serde_json::to_string(id).map_err(|error| ("invalid_params", error.to_string()))
}

impl Requests {
    pub(in crate::rpc::server) fn begin(&self, id: &Value) -> Result<Lease, Failure> {
        let key = key(id)?;
        match self.active.lock().entry(key.clone()) {
            Entry::Occupied(_) => Err((
                "request_already_running",
                format!("context request {id} is already running on this connection"),
            )),
            Entry::Vacant(entry) => {
                entry.insert(CancellationToken::new());
                Ok(Lease {
                    active: Arc::clone(&self.active),
                    key,
                })
            }
        }
    }

    pub(in crate::rpc::server) fn token(&self, id: &Value) -> Result<CancellationToken, Failure> {
        self.active.lock().get(&key(id)?).cloned().ok_or_else(|| {
            (
                "request_not_running",
                format!("context request {id} is not running on this connection"),
            )
        })
    }

    pub(in crate::rpc::server) fn cancel_all(&self) {
        // Wake callbacks can finish a lease, so release the registry first.
        let tokens: Vec<_> = self.active.lock().values().cloned().collect();
        for token in tokens {
            token.cancel();
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.active.lock().remove(&self.key);
    }
}
