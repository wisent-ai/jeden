pub use crate::cas;
mod dispatch;
mod fleet;
mod limits;
mod local;
mod scheduler;
mod types;
pub(crate) mod watch;

pub use fleet::{coordinator, placement, protocol, store, worker};
pub(crate) use local::sandbox;
pub use local::{discovery, mailbox, workspace};

pub use coordinator::Coordinator;
pub use discovery::discover_agents;
pub use dispatch::execute_delegate;
pub use limits::{check_declaration, limits_from_config, limits_settings, set_limits};
pub use mailbox::Mailbox;
pub use placement::{select_worker, PlacementDecision};
pub use protocol::{
    Attempt, AttemptPhase, CommitRequest, Job, JobOutcome, JobPhase, Lease, NegotiatedHello,
    PlacementConstraints, ProtocolError, ProtocolVersion, Resources, VersionRange, WorkOffer,
    Worker, WorkerDescriptor, WorkerEvent, WorkerHello, WORKER_PROTOCOL_MAJOR,
    WORKER_PROTOCOL_MINOR,
};
pub use scheduler::{BatchTask, SpawnRequest, TaskScheduler};
pub use store::{CoordinatorStore, JobState};
pub use types::{
    AgentDefinition, CapabilityHealth, JobRecord, JobStatus, MailMessage, SpawnPolicy, TaskError,
    TaskLimits,
};
pub use worker::{LoopbackTransport, WorkerExecutor, WorkerRun, WorkerRuntime, WorkerTransport};

use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Make sure the store's schema manifest stands at the version the built-in
/// `task-store` migration plan migrates to: migrate one that exists, write
/// one that does not. The version is the plan's, never a second copy here.
pub(crate) fn ensure_store_schema(store: &Path) -> Result<(), TaskError> {
    let manifest = store.join("store-schema.json");
    let plan = crate::cli::config::migrations::builtin_document_plan("task-store")
        .map_err(TaskError::Invalid)?;
    // A manifest that cannot be examined is refused: read as absent, a fresh
    // manifest was written over the store's recorded schema version.
    if crate::tool_runtime::runtime_ops::platform::presence::present(&manifest)
        .map_err(TaskError::Invalid)?
    {
        crate::cli::config::migrations::migrate_json(&manifest, &plan)
            .map_err(TaskError::Invalid)?;
    } else {
        crate::cli::config::migrations::write_json_atomic(
            &manifest,
            &json!({"schemaVersion": plan.to, "store": plan.store}),
        )
        .map_err(TaskError::Invalid)?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct TaskToolDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    pub input: Value,
}

/// Typed registration surface for the central tool registry. Dispatch may invoke
/// `execute_registered_tool`; it need not know scheduler internals.
pub fn registered_task_tools() -> Vec<TaskToolDescriptor> {
    vec![
        TaskToolDescriptor {
            name: "task",
            description:
                "Spawn one bounded durable child-agent task or execute a bounded DAG batch",
            input: json!({"type":"object","properties":{"task":{"type":"string"},"agent":{"type":"string"},"maxSteps":{"type":"number"},"wait":{"type":"boolean"},"tasks":{"type":"array"}}}),
        },
        TaskToolDescriptor {
            name: "job",
            description:
                "Poll one job until it finishes, or list, cancel, deliver, merge, or inspect task scheduler health",
            input: json!({"type":"object","properties":{"op":{"type":"string"},"id":{"type":"string"}},"required":["op"]}),
        },
        TaskToolDescriptor {
            name: "irc",
            description: "Durable agent mailbox send, inbox, wait, and wake operations",
            input: json!({"type":"object","properties":{"op":{"type":"string"},"from":{"type":"string"},"to":{"type":"string"},"agent":{"type":"string"},"message":{"type":"string"},"correlationId":{"type":"string"},"replyTo":{"type":"string"}},"required":["op"]}),
        },
    ]
}

fn dynamic_descriptors() -> Vec<crate::tool_runtime::DynamicToolDescriptor> {
    let sandbox = sandbox::health();
    if !sandbox.enforced {
        let detail = format!(
            "enforced sandbox unavailable: {}: {}",
            sandbox.backend, sandbox.detail
        );
        return registered_task_tools()
            .into_iter()
            .map(|descriptor| {
                let executable = descriptor.name != "task";
                crate::tool_runtime::DynamicToolDescriptor {
                    name: descriptor.name.into(),
                    description: descriptor.description.into(),
                    input: descriptor.input,
                    healthy: executable,
                    health: if executable {
                        "healthy".into()
                    } else {
                        detail.clone()
                    },
                }
            })
            .collect();
    }
    // The probe answers for the store delegation actually opens (the same
    // one the conformance probe reads), so nothing is written to the
    // system's temporary directory and no scratch store is left to remove.
    let health = std::env::current_dir()
        .map_err(|error| error.to_string())
        .and_then(|cwd| {
            limits_from_config(&cwd)
                .and_then(|limits| TaskScheduler::open(&cwd, &default_store(&cwd, None), limits))
                .map_err(|error| error.to_string())
        })
        .map(|scheduler| scheduler.health());
    let (healthy, detail) = match health {
        Ok(health) if health.healthy => (
            true,
            format!(
                "healthy; {} agents discovered; limits={:?}",
                health.discovered_agents, health.limits
            ),
        ),
        Ok(health) => (false, health.errors.join("; ")),
        Err(error) => (false, error),
    };
    registered_task_tools()
        .into_iter()
        .map(|descriptor| crate::tool_runtime::DynamicToolDescriptor {
            name: descriptor.name.into(),
            description: descriptor.description.into(),
            input: descriptor.input,
            healthy,
            health: detail.clone(),
        })
        .collect()
}

fn dynamic_execute(
    runtime: &crate::tool_runtime::ToolRuntime<'_>,
    tool: &str,
    input: &Value,
) -> Option<Result<Value, String>> {
    // The registry is the one list of task tools; dispatch reads it rather
    // than repeating its names.
    if !registered_task_tools()
        .iter()
        .any(|descriptor| descriptor.name == tool)
    {
        return None;
    }
    if runtime.operation.cancellation().is_cancelled() {
        return Some(Err(format!("{tool} cancelled before scheduling")));
    }
    if tool == "task" && !runtime.allow_command {
        return Some(Err("task requires --allow-command".into()));
    }
    if tool == "task" {
        let sandbox = sandbox::health();
        if !sandbox.enforced {
            return Some(Err(format!(
                "enforced sandbox unavailable: {}: {}",
                sandbox.backend, sandbox.detail
            )));
        }
    }
    Some(
        execute_registered_tool(runtime.cwd, runtime.artifact_dir, tool, input)
            .map_err(|error| error.to_string()),
    )
}

pub fn register_with_tool_runtime() -> Result<(), String> {
    crate::tool_runtime::register_dynamic_tools(crate::tool_runtime::DynamicToolRegistration {
        owner: "task-runtime",
        descriptors: dynamic_descriptors,
        execute: dynamic_execute,
    })
}

/// Where the task store lives: beside the session's artifact directory when
/// the run has one, so its jobs and mail stay with the session's files, and
/// under the working directory's `.jeden` for a run that has none.
pub fn default_store(cwd: &Path, artifact_dir: Option<&Path>) -> PathBuf {
    match artifact_dir {
        Some(artifacts) => artifacts.with_file_name("task-runtime"),
        None => cwd.join(".jeden/task-runtime"),
    }
}

pub fn execute_registered_tool(
    cwd: &Path,
    artifact_dir: Option<&Path>,
    tool: &str,
    input: &Value,
) -> Result<Value, TaskError> {
    let scheduler = TaskScheduler::open(
        cwd,
        &default_store(cwd, artifact_dir),
        limits_from_config(cwd)?,
    )?;
    match tool {
        "task" => dispatch::execute_task(&scheduler, input),
        "job" => dispatch::execute_job(&scheduler, input),
        "irc" => dispatch::execute_irc(&scheduler, input),
        other => Err(TaskError::NotFound(format!(
            "unregistered task tool: {other}"
        ))),
    }
}

/// Write `value` to `path` whole through the shared durable replace: staged
/// in a file of its own beside it, then renamed over it. A step that fails
/// removes the staged file, and one that cannot be removed is named in the
/// error, so no orphan staging file is left beside the record.
pub(crate) fn atomic_json(path: &Path, value: &impl Serialize) -> Result<(), TaskError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    crate::tool_runtime::runtime_ops::durable::replace(path, &bytes).map_err(TaskError::Io)
}
