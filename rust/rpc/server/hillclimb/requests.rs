//! Writer requests belong to their RPC connection, including shutdown cleanup.
use std::collections::{hash_map::Entry, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{atomic::AtomicBool, Arc};

use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::hillclimb::Scope;
use crate::tool_runtime::runtime_ops::CancellationToken;

type Failure = (&'static str, String);

struct Active {
    id: Value,
    cwd: PathBuf,
    scope: Scope,
    cancellation: CancellationToken,
}

#[derive(Default)]
pub(in crate::rpc::server) struct Requests {
    active: Arc<Mutex<HashMap<String, Active>>>,
}

pub(super) struct Lease {
    active: Arc<Mutex<HashMap<String, Active>>>,
    key: String,
    pub(super) flag: Arc<AtomicBool>,
    pub(super) cancellation: CancellationToken,
}

fn key(id: &Value) -> Result<String, Failure> {
    serde_json::to_string(id).map_err(|error| ("invalid_params", error.to_string()))
}

impl Requests {
    pub(super) fn begin(&self, id: &Value, cwd: &Path, scope: Scope) -> Result<Lease, Failure> {
        let key = key(id)?;
        match self.active.lock().entry(key.clone()) {
            Entry::Occupied(_) => Err((
                "request_already_running",
                format!("hillclimb request {id} is already running on this connection"),
            )),
            Entry::Vacant(entry) => {
                let flag = Arc::new(AtomicBool::new(false));
                let cancellation = CancellationToken::from_flag(Arc::clone(&flag));
                entry.insert(Active { id: id.clone(), cwd: cwd.to_path_buf(), scope, cancellation: cancellation.clone() });
                Ok(Lease { active: Arc::clone(&self.active), key, flag, cancellation })
            }
        }
    }

    pub(super) fn cancel(&self, id: &Value) -> Result<Value, Failure> {
        // A wake can end a lease; never announce cancellation while holding it.
        let cancellation = self.active.lock().get(&key(id)?).map(|active| active.cancellation.clone())
            .ok_or_else(|| ("request_not_running", format!("hillclimb request {id} is not running on this connection")))?;
        cancellation.cancel();
        Ok(json!({ "requestId": id, "cancellationRequested": true }))
    }

    pub(super) fn status(&self) -> Value {
        let active = self.active.lock();
        let requests: Vec<_> = active.values().map(|request| json!({
            "requestId": request.id,
            "cwd": request.cwd,
            "scope": request.scope,
            "cancellationRequested": request.cancellation.is_cancelled(),
        })).collect();
        json!({ "requests": requests })
    }

    pub(in crate::rpc::server) fn cancel_all(&self) {
        let tokens: Vec<_> = self.active.lock().values().map(|active| active.cancellation.clone()).collect();
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
