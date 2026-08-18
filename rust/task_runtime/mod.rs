pub use crate::cas;
pub mod coordinator;
mod discovery;
mod mailbox;
pub mod placement;
pub mod protocol;
pub(crate) mod sandbox;
mod scheduler;
pub mod store;
mod types;
pub mod worker;
mod workspace;

pub use coordinator::Coordinator;
pub use discovery::discover_agents;
pub use mailbox::Mailbox;
pub use placement::{select_worker, PlacementDecision};
pub use protocol::{
    Attempt, AttemptPhase, CommitRequest, Job, JobOutcome, JobPhase, Lease, NegotiatedHello,
    PlacementConstraints, ProtocolError, ProtocolVersion, Resources, VersionRange, WorkOffer,
    Worker, WorkerDescriptor, WorkerEvent, WorkerHello, WORKER_PROTOCOL_MAJOR,
    WORKER_PROTOCOL_MINOR,
};
pub(crate) use scheduler::workspace_root_for;
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static ATOMIC_SEQUENCE: AtomicU64 = AtomicU64::new(1);
pub const TASK_STORE_SCHEMA_VERSION: u32 = 2;

pub(crate) fn ensure_store_schema(store: &Path) -> Result<(), TaskError> {
    let manifest = store.join("store-schema.json");
    if manifest.exists() {
        let plan = crate::cli::config::migrations::builtin_document_plan("task-store")
            .map_err(TaskError::Invalid)?;
        crate::cli::config::migrations::migrate_json(&manifest, &plan)
            .map_err(TaskError::Invalid)?;
    } else {
        crate::cli::config::migrations::write_json_atomic(
            &manifest,
            &json!({"schemaVersion":TASK_STORE_SCHEMA_VERSION,"store":"task-store"}),
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
            description: "Poll, list, cancel, deliver, merge, or inspect task scheduler health",
            input: json!({"type":"object","properties":{"op":{"type":"string"},"id":{"type":"string"},"waitMs":{"type":"number"}},"required":["op"]}),
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
    let health_store =
        std::env::temp_dir().join(format!("jeden-task-health-{}", std::process::id()));
    let health = std::env::current_dir()
        .map_err(|error| error.to_string())
        .and_then(|cwd| {
            TaskScheduler::open(&cwd, &health_store, TaskLimits::default())
                .map_err(|error| error.to_string())
        })
        .map(|scheduler| scheduler.health());
    let _ = fs::remove_dir_all(&health_store);
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
    if !matches!(tool, "task" | "job" | "irc") {
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

pub fn limits_from_config(cwd: &Path) -> TaskLimits {
    let config = crate::cli::config::merged_config_value(cwd);
    config
        .get("taskScheduler")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

pub fn default_store(cwd: &Path, artifact_dir: Option<&Path>) -> PathBuf {
    artifact_dir
        .and_then(Path::parent)
        .map(|p| p.join("task-runtime"))
        .unwrap_or_else(|| cwd.join(".jeden/task-runtime"))
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
        limits_from_config(cwd),
    )?;
    match tool {
        "task" => execute_task(&scheduler, input),
        "job" => execute_job(&scheduler, input),
        "irc" => execute_irc(&scheduler, input),
        other => Err(TaskError::NotFound(format!(
            "unregistered task tool: {other}"
        ))),
    }
}

pub fn execute_delegate(
    cwd: &Path,
    artifact_dir: Option<&Path>,
    input: &Value,
) -> Result<Value, TaskError> {
    let scheduler = TaskScheduler::open(
        cwd,
        &default_store(cwd, artifact_dir),
        limits_from_config(cwd),
    )?;
    let mut request: SpawnRequest =
        serde_json::from_value(input.clone()).map_err(|e| TaskError::Invalid(e.to_string()))?;
    if request.agent.is_empty() {
        request.agent = "default".into();
    }
    if request.parent_job.is_none() {
        request.parent_job = std::env::var("JEDEN_TASK_JOB")
            .ok()
            .filter(|value| !value.is_empty());
    }
    let job = scheduler.spawn(request)?;
    let job = scheduler.poll(
        &job.id,
        Duration::from_millis(scheduler.limits.wait_timeout_ms),
    )?;
    if !job.status.terminal() {
        let _ = scheduler.cancel(&job.id);
        return Err(TaskError::Timeout(format!(
            "delegated job timed out: {}",
            job.id
        )));
    }
    let stdout = bounded_text(&job.stdout, scheduler.limits.max_output_bytes)?;
    let stderr = bounded_text(&job.stderr, scheduler.limits.max_output_bytes)?;
    let delegated =
        serde_json::from_str::<Value>(stdout.trim()).unwrap_or(Value::String(stdout.clone()));
    Ok(json!({"job":job,"stdout":stdout,"stderr":stderr,"delegated":delegated}))
}

fn execute_task(scheduler: &TaskScheduler, input: &Value) -> Result<Value, TaskError> {
    if let Some(tasks) = input.get("tasks") {
        let batch: Vec<BatchTask> = serde_json::from_value(tasks.clone())?;
        return Ok(json!({"jobs":scheduler.batch(batch)?}));
    }
    let mut request: SpawnRequest = serde_json::from_value(input.clone())?;
    if request.parent_job.is_none() {
        request.parent_job = std::env::var("JEDEN_TASK_JOB")
            .ok()
            .filter(|value| !value.is_empty());
    }
    let wait = input.get("wait").and_then(Value::as_bool).unwrap_or(false);
    let job = scheduler.spawn(request)?;
    if wait {
        Ok(json!(scheduler.poll(
            &job.id,
            Duration::from_millis(scheduler.limits.wait_timeout_ms)
        )?))
    } else {
        Ok(json!(job))
    }
}

fn execute_job(scheduler: &TaskScheduler, input: &Value) -> Result<Value, TaskError> {
    let op = input
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| TaskError::Invalid("job op is required".into()))?;
    let id = || {
        input
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| TaskError::Invalid(format!("job {op} requires id")))
    };
    match op {
        "list" => Ok(json!(scheduler.list()?)),
        "poll" => Ok(json!(scheduler.poll(
            id()?,
            Duration::from_millis(
                input
                    .get("waitMs")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .min(scheduler.limits.wait_timeout_ms)
            )
        )?)),
        "cancel" => Ok(json!({"cancelled":scheduler.cancel(id()?)?})),
        "deliver" => Ok(json!(scheduler.deliver(id()?)?)),
        "merge" => Ok(json!(scheduler.merge(id()?)?)),
        "health" => Ok(json!(scheduler.health())),
        _ => Err(TaskError::Invalid(format!("unknown job op: {op}"))),
    }
}

fn execute_irc(scheduler: &TaskScheduler, input: &Value) -> Result<Value, TaskError> {
    let mailbox = scheduler.mailbox()?;
    let op = input
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| TaskError::Invalid("irc op is required".into()))?;
    let text = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| TaskError::Invalid(format!("irc {op} requires {key}")))
    };
    match op {
        "send" => Ok(json!(mailbox.send(
            text("from")?,
            text("to")?,
            text("message")?,
            input
                .get("correlationId")
                .and_then(Value::as_str)
                .map(str::to_string),
            input
                .get("replyTo")
                .and_then(Value::as_str)
                .map(str::to_string)
        )?)),
        "inbox" => Ok(json!(mailbox.inbox(text("agent")?, true)?)),
        "wait" => Ok(json!(mailbox.wait(
            text("agent")?,
            input.get("correlationId").and_then(Value::as_str),
            Duration::from_millis(
                input
                    .get("timeoutMs")
                    .and_then(Value::as_u64)
                    .unwrap_or(scheduler.limits.wait_timeout_ms)
                    .min(scheduler.limits.wait_timeout_ms)
            )
        )?)),
        "wake" => Ok(json!({"pending":mailbox.wake_pending(text("agent")?)?})),
        _ => Err(TaskError::Invalid(format!("unknown irc op: {op}"))),
    }
}

fn bounded_text(path: &Path, max: u64) -> Result<String, TaskError> {
    let file = fs::File::open(path)?;
    use std::io::Read;
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        bytes.truncate(max as usize);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub(crate) fn next_sequence() -> u64 {
    ATOMIC_SEQUENCE.fetch_add(1, Ordering::Relaxed)
}
pub(crate) fn atomic_json(path: &Path, value: &impl Serialize) -> Result<(), TaskError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let sequence = next_sequence();
    let temporary = path.with_extension(format!("tmp-{}-{sequence}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    use std::io::Write;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}
