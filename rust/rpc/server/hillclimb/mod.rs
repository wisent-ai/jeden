//! Native writer execution. The connection remains responsive to reads/cancel.
mod parameters;
mod requests;
pub(super) use requests::Requests;

use std::sync::Arc;
use std::thread::JoinHandle;

use serde_json::{json, Value};

use super::{error_response, parking::InFlight, success_response, ServerState, WireRequest};
use crate::agent::RunHooks;

type Failure = (&'static str, String);

fn failed_response(id: Value, detail: &str, cancellation_requested: bool) -> Value {
    let mut response = error_response(id, "hillclimb_failed", detail);
    response["error"]["data"] = json!({ "cancellationRequested": cancellation_requested });
    response
}

pub(super) fn start(
    state: &Arc<ServerState>,
    wire: WireRequest,
    in_flight: InFlight,
) -> Result<Option<JoinHandle<Result<(), String>>>, String> {
    let (args, climb) = match parameters::parse(wire.params) {
        Ok(parameters) => parameters,
        Err((code, detail)) => {
            state.writer.send(&error_response(wire.id, code, &detail))?;
            return Ok(None);
        }
    };
    let lease = match state
        .hillclimb_requests
        .begin(&wire.id, &args.cwd, climb.scope)
    {
        Ok(lease) => lease,
        Err((code, detail)) => {
            state.writer.send(&error_response(wire.id, code, &detail))?;
            return Ok(None);
        }
    };
    let state = Arc::clone(state);
    let failure_writer = state.writer.clone();
    let failure_id = wire.id.clone();
    let failure_cancellation = lease.cancellation.clone();
    match std::thread::Builder::new().name("jeden-hillclimb".into()).spawn(move || {
        let _in_flight = in_flight;
        let mut hooks = RunHooks::inert();
        hooks.cancel = Arc::clone(&lease.flag);
        hooks.interactive = false;
        let progress_writer = state.writer.clone();
        let progress_id = wire.id.clone();
        let cancellation = lease.cancellation.clone();
        hooks.progress = Box::new(move |message| {
            if let Err(error) = progress_writer.send(&json!({
                "type": "hillclimbProgress", "requestId": progress_id, "message": message,
            })) {
                eprintln!("hillclimb request {progress_id}: writing progress failed: {error}; requesting cancellation");
                cancellation.cancel();
            }
        });
        let result = crate::hillclimb::execution::run(&args, climb, &mut hooks)
            .and_then(|outcome| serde_json::to_value(outcome)
                .map_err(|error| format!("the hillclimb outcome cannot be encoded: {error}")));
        let cancellation_requested = lease.cancellation.is_cancelled();
        // As with context requests, release the id before publishing its end.
        drop(lease);
        let response = match result {
            Ok(outcome) => success_response(wire.id, outcome),
            Err(detail) => failed_response(wire.id, &detail, cancellation_requested),
        };
        state.writer.send(&response).map_err(|error|
            format!("writing hillclimb response {} failed: {error}", response["id"]))
    }) {
        Ok(worker) => Ok(Some(worker)),
        Err(error) => {
            let detail = format!("the hillclimb worker could not be started: {error}");
            let response = failed_response(failure_id, &detail, failure_cancellation.is_cancelled());
            failure_writer.send(&response).map_err(|write_error|
                format!("{detail}; writing hillclimb startup refusal {} failed: {write_error}", response["id"]))?;
            Ok(None)
        }
    }
}

pub(super) fn cancel(state: &ServerState, params: &Value) -> Result<Value, Failure> {
    let id = params.get("requestId").ok_or_else(|| {
        (
            "invalid_params",
            "hillclimb/cancel requires requestId".to_string(),
        )
    })?;
    state.hillclimb_requests.cancel(id)
}

pub(super) fn active(state: &ServerState) -> Value {
    state.hillclimb_requests.status()
}

pub(super) fn status(params: &Value) -> Result<Value, Failure> {
    let parameters = parameters::history(params)?;
    let repository = crate::hillclimb::status::RepositorySelection::new(
        parameters.repository,
        parameters.without_repository,
    )
    .map_err(|error| ("invalid_params", error))?;
    let report = crate::hillclimb::status::read(
        std::path::Path::new(parameters.cwd),
        parameters.ledger.map(std::path::Path::new),
        parameters.last,
        parameters.ending,
        parameters.round,
        repository,
    )
    .map_err(|error| ("hillclimb_unreadable", error))?;
    serde_json::to_value(&report).map_err(|error| {
        (
            "invalid_response",
            format!(
                "the hillclimb response for ledger {:?} cannot be encoded as JSON: {error}",
                report.ledger
            ),
        )
    })
}
