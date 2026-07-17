use super::discovery::discover_agents;
use super::mailbox::Mailbox;
use super::types::{
    AgentDefinition, CapabilityHealth, JobRecord, JobStatus, TaskError, TaskLimits,
};
use super::workspace::{isolate, IsolatedWorkspace};
use super::{atomic_json, ensure_store_schema, now_millis};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static JOB_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

#[derive(Clone, Debug)]
pub struct TaskScheduler {
    pub store: PathBuf,
    pub cwd: PathBuf,
    pub limits: TaskLimits,
    exe: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnRequest {
    pub task: String,
    #[serde(default = "default_agent")]
    pub agent: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "default_steps")]
    pub max_steps: u64,
    #[serde(default)]
    pub parent_job: Option<String>,
    #[serde(default)]
    pub isolate: Option<bool>,
}
fn default_agent() -> String {
    "default".into()
}
fn default_steps() -> u64 {
    6
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchTask {
    pub id: String,
    pub task: String,
    #[serde(default = "default_agent")]
    pub agent: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

impl TaskScheduler {
    pub fn open(cwd: &Path, store: &Path, mut limits: TaskLimits) -> Result<Self, TaskError> {
        limits.max_parallel = limits.max_parallel.clamp(1, 32);
        limits.max_batch = limits.max_batch.clamp(1, 256);
        limits.max_depth = limits.max_depth.min(16);
        limits.max_children = limits.max_children.clamp(1, 256);
        limits.max_output_bytes = limits.max_output_bytes.clamp(1_024, 64 * 1024 * 1024);
        limits.wait_timeout_ms = limits.wait_timeout_ms.clamp(100, 3_600_000);
        limits.kill_grace_ms = limits.kill_grace_ms.clamp(10, 30_000);
        for path in [
            store.to_path_buf(),
            store.join("jobs"),
            store.join("workspaces"),
            store.join("slots"),
            store.join("sessions"),
        ] {
            fs::create_dir_all(path)?;
        }
        ensure_store_schema(store)?;
        let exe = std::env::var_os("JEDEN_TASK_EXECUTABLE")
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(|| {
                std::env::current_exe().map_err(|error| TaskError::Process(error.to_string()))
            })?;
        let this = Self {
            store: store.into(),
            cwd: cwd.into(),
            limits,
            exe,
        };
        this.recover()?;
        Ok(this)
    }
    pub fn mailbox(&self) -> Result<Mailbox, TaskError> {
        Mailbox::new(&self.store, self.limits.max_children.saturating_mul(64))
    }
    fn workspace_root(&self) -> PathBuf {
        let store = fs::canonicalize(&self.store).unwrap_or_else(|_| self.store.clone());
        let cwd = fs::canonicalize(&self.cwd).unwrap_or_else(|_| self.cwd.clone());
        if store.starts_with(&cwd) {
            let mut hash = DefaultHasher::new();
            cwd.hash(&mut hash);
            std::env::temp_dir()
                .join("jeden-task-workspaces")
                .join(format!("{:016x}", hash.finish()))
        } else {
            self.store.join("workspaces")
        }
    }
    fn job_path(&self, id: &str) -> PathBuf {
        self.store.join("jobs").join(format!("{id}.json"))
    }
    fn write_job(&self, job: &JobRecord) -> Result<(), TaskError> {
        atomic_json(&self.job_path(&job.id), job)
    }
    pub fn get(&self, id: &str) -> Result<JobRecord, TaskError> {
        let path = self.job_path(id);
        if !path.exists() {
            return Err(TaskError::NotFound(format!("job not found: {id}")));
        }
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }
    pub fn list(&self) -> Result<Vec<JobRecord>, TaskError> {
        let mut jobs = Vec::new();
        for entry in fs::read_dir(self.store.join("jobs"))?.flatten() {
            if entry.path().extension().and_then(|v| v.to_str()) == Some("json") {
                if let Ok(job) = serde_json::from_slice::<JobRecord>(&fs::read(entry.path())?) {
                    jobs.push(job);
                }
            }
        }
        jobs.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(jobs)
    }
    fn agents(&self) -> Result<BTreeMap<String, AgentDefinition>, TaskError> {
        Ok(discover_agents(&self.cwd)?
            .into_iter()
            .map(|a| (a.id.clone(), a))
            .collect())
    }
    fn depth_and_parent(
        &self,
        request: &SpawnRequest,
    ) -> Result<(u32, Option<JobRecord>), TaskError> {
        if let Some(id) = &request.parent_job {
            let parent = self.get(id)?;
            Ok((parent.depth.saturating_add(1), Some(parent)))
        } else {
            Ok((
                std::env::var("JEDEN_TASK_DEPTH")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
                None,
            ))
        }
    }
    fn validate_policy(
        &self,
        request: &SpawnRequest,
        definition: &AgentDefinition,
        depth: u32,
        parent: Option<&JobRecord>,
    ) -> Result<(), TaskError> {
        if depth > self.limits.max_depth {
            return Err(TaskError::RecursionDenied {
                agent: request.agent.clone(),
                depth,
            });
        }
        if definition
            .spawn
            .deny_agents
            .iter()
            .any(|v| v == &request.agent)
        {
            return Err(TaskError::RecursionDenied {
                agent: request.agent.clone(),
                depth,
            });
        }
        if let Some(parent) = parent {
            let agents = self.agents()?;
            let parent_def = agents.get(&parent.agent);
            if parent.agent == request.agent
                && !parent_def.map(|d| d.spawn.allow_recursive).unwrap_or(false)
            {
                return Err(TaskError::RecursionDenied {
                    agent: request.agent.clone(),
                    depth,
                });
            }
            if let Some(parent_def) = parent_def {
                if parent_def
                    .spawn
                    .deny_agents
                    .iter()
                    .any(|v| v == &request.agent)
                    || (!parent_def.spawn.allow_agents.is_empty()
                        && !parent_def
                            .spawn
                            .allow_agents
                            .iter()
                            .any(|v| v == &request.agent))
                {
                    return Err(TaskError::RecursionDenied {
                        agent: request.agent.clone(),
                        depth,
                    });
                }
            }
            let children = self
                .list()?
                .into_iter()
                .filter(|j| j.parent_job.as_deref() == Some(&parent.id))
                .count();
            if children >= self.limits.max_children {
                return Err(TaskError::Capacity {
                    running: children,
                    limit: self.limits.max_children,
                });
            }
        }
        Ok(())
    }
    fn reserve_slot(&self, job_id: &str) -> Result<PathBuf, TaskError> {
        self.clean_slots()?;
        for index in 0..self.limits.max_parallel.max(1) {
            let path = self.store.join("slots").join(index.to_string());
            match OpenOptions::new().create_new(true).write(true).open(&path) {
                Ok(mut file) => {
                    use std::io::Write;
                    write!(file, "{}\n{}\n", std::process::id(), job_id)?;
                    return Ok(path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        let running = self
            .list()?
            .into_iter()
            .filter(|j| j.status == JobStatus::Running)
            .count();
        Err(TaskError::Capacity {
            running,
            limit: self.limits.max_parallel,
        })
    }
    fn clean_slots(&self) -> Result<(), TaskError> {
        for entry in fs::read_dir(self.store.join("slots"))?.flatten() {
            let path = entry.path();
            let pid = fs::read_to_string(&path)
                .ok()
                .and_then(|v| v.lines().next()?.parse::<u32>().ok());
            if !pid.map(process_alive).unwrap_or(false) {
                let _ = fs::remove_file(path);
            }
        }
        Ok(())
    }
    pub fn spawn(&self, request: SpawnRequest) -> Result<JobRecord, TaskError> {
        if request.task.trim().is_empty() {
            return Err(TaskError::Invalid("task text is required".into()));
        }
        let agents = self.agents()?;
        let definition = agents
            .get(&request.agent)
            .ok_or_else(|| TaskError::NotFound(format!("agent not found: {}", request.agent)))?;
        let (depth, parent) = self.depth_and_parent(&request)?;
        self.validate_policy(&request, definition, depth, parent.as_ref())?;
        let child_task = agent_task_context(&self.cwd, definition, &request.task)?;
        let created = now_millis();
        let sequence = JOB_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let id = format!("job-{created}-{}-{sequence}", std::process::id());
        let slot = self.reserve_slot(&id)?;
        let isolated = match isolate(&self.cwd, &self.workspace_root(), &id) {
            Ok(v) => v,
            Err(error) => {
                let _ = fs::remove_file(&slot);
                return Err(error);
            }
        };
        let stdout = self.store.join("jobs").join(format!("{id}.stdout.log"));
        let stderr = self.store.join("jobs").join(format!("{id}.stderr.log"));
        let capture = self.store.join("jobs").join(format!("{id}.patch"));
        let session_path = self.store.join("sessions").join(&id);
        fs::create_dir_all(&session_path)?;
        let mut command = super::sandbox::command(
            &self.exe,
            &[
                self.cwd.clone(),
                isolated.path.clone(),
                session_path.clone(),
                self.store.clone(),
            ],
            &[
                isolated.path.clone(),
                session_path.clone(),
                self.store.clone(),
            ],
        )
        .map_err(TaskError::Process)?;
        command
            .arg("run")
            .arg(&child_task)
            .arg("--cwd")
            .arg(&isolated.path)
            .arg("--max-steps")
            .arg(request.max_steps.clamp(1, 64).to_string())
            .arg("--json")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(model) = request.model.as_ref().or(definition.model.as_ref()) {
            command.arg("--model").arg(model);
        }
        command
            .env("JEDEN_TASK_JOB", &id)
            .env(
                "JEDEN_TASK_PARENT",
                request.parent_job.as_deref().unwrap_or(""),
            )
            .env("JEDEN_TASK_DEPTH", depth.to_string())
            .env("JEDEN_TASK_SESSION", &session_path)
            .env("JEDEN_SESSION_ROOT", &session_path);
        if !definition.tools.is_empty() {
            command.env("JEDEN_AGENT_TOOLS", definition.tools.join(","));
        }
        configure_group(&mut command);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = fs::remove_file(&slot);
                return Err(TaskError::Process(error.to_string()));
            }
        };
        let child_stdout = child
            .stdout
            .take()
            .ok_or_else(|| TaskError::Process("child stdout pipe unavailable".into()))?;
        let child_stderr = child
            .stderr
            .take()
            .ok_or_else(|| TaskError::Process("child stderr pipe unavailable".into()))?;
        fs::write(&slot, format!("{}\n{}\n", child.id(), id))?;
        let job = JobRecord {
            id: id.clone(),
            task: request.task,
            agent: request.agent,
            status: JobStatus::Running,
            cwd: self.cwd.clone(),
            workspace: isolated.path.clone(),
            isolation: isolated.strategy.clone(),
            session_path,
            stdout,
            stderr,
            capture,
            pid: Some(child.id()),
            parent_job: request.parent_job,
            depth,
            created_at: created,
            updated_at: created,
            exit_code: None,
            error: None,
            delivered: false,
            metadata: BTreeMap::from([
                ("definition".into(), json!(definition.source)),
                ("tools".into(), json!(definition.tools)),
                ("skills".into(), json!(definition.skills)),
                ("outputSchema".into(), definition.output.clone()),
            ]),
        };
        self.write_job(&job)?;
        let scheduler = self.clone();
        let mut reaper_job = job.clone();
        let stdout_path = job.stdout.clone();
        let stderr_path = job.stderr.clone();
        let max_output = self.limits.max_output_bytes;
        thread::Builder::new()
            .name(format!("task-reaper-{id}"))
            .spawn(move || {
                let (status, stdout_result, stderr_result) = thread::scope(|scope| {
                    let stdout_reader =
                        scope.spawn(|| capture_pipe(child_stdout, &stdout_path, max_output));
                    let stderr_reader =
                        scope.spawn(|| capture_pipe(child_stderr, &stderr_path, max_output));
                    let status = child.wait();
                    let stdout_result = stdout_reader.join().unwrap_or_else(|_| {
                        Err(TaskError::Process("stdout capture panicked".into()))
                    });
                    let stderr_result = stderr_reader.join().unwrap_or_else(|_| {
                        Err(TaskError::Process("stderr capture panicked".into()))
                    });
                    (status, stdout_result, stderr_result)
                });
                reaper_job.updated_at = now_millis();
                reaper_job.pid = None;
                if let Err(error) = stdout_result.and(stderr_result) {
                    reaper_job.error = Some(error.to_string());
                }
                let cancelled = scheduler
                    .get(&reaper_job.id)
                    .map(|current| current.status == JobStatus::Cancelled)
                    .unwrap_or(false);
                if cancelled {
                    reaper_job.status = JobStatus::Cancelled;
                } else {
                    match status {
                        Ok(status) => {
                            reaper_job.exit_code = status.code();
                            reaper_job.status = if status.success() {
                                JobStatus::Succeeded
                            } else {
                                JobStatus::Failed
                            };
                        }
                        Err(error) => {
                            reaper_job.status = JobStatus::Interrupted;
                            reaper_job.error = Some(error.to_string());
                        }
                    }
                }
                if reaper_job.status == JobStatus::Succeeded
                    && !definition_output_is_unconstrained(&reaper_job.metadata["outputSchema"])
                {
                    match fs::read_to_string(&reaper_job.stdout)
                        .ok()
                        .and_then(|text| {
                            serde_json::from_str::<serde_json::Value>(text.trim()).ok()
                        }) {
                        Some(value) => {
                            if let Err(error) =
                                validate_output(&value, &reaper_job.metadata["outputSchema"], "$")
                            {
                                reaper_job.status = JobStatus::Failed;
                                reaper_job.error =
                                    Some(format!("agent output contract failed: {error}"));
                            }
                        }
                        None => {
                            reaper_job.status = JobStatus::Failed;
                            reaper_job.error = Some(
                                "agent output contract failed: child stdout is not one JSON value"
                                    .into(),
                            );
                        }
                    }
                }
                let workspace = IsolatedWorkspace {
                    path: reaper_job.workspace.clone(),
                    strategy: reaper_job.isolation.clone(),
                    parent: reaper_job.cwd.clone(),
                };
                if let Err(error) = workspace.capture(&reaper_job.capture, max_output) {
                    reaper_job.error = Some(error.to_string());
                    if reaper_job.status == JobStatus::Succeeded {
                        reaper_job.status = JobStatus::Failed;
                    }
                }
                let _ = scheduler.write_job(&reaper_job);
                let _ = fs::remove_file(slot);
            })
            .map_err(|e| TaskError::Process(e.to_string()))?;
        Ok(job)
    }
    pub fn poll(&self, id: &str, wait: Duration) -> Result<JobRecord, TaskError> {
        let deadline = Instant::now() + wait;
        loop {
            let job = self.get(id)?;
            if job.status.terminal() || Instant::now() >= deadline {
                return Ok(job);
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    pub fn deliver(&self, id: &str) -> Result<JobRecord, TaskError> {
        let mut job = self.get(id)?;
        if !job.status.terminal() {
            return Err(TaskError::Conflict(format!("job {id} is not terminal")));
        }
        job.delivered = true;
        job.updated_at = now_millis();
        self.write_job(&job)?;
        Ok(job)
    }
    pub fn cancel(&self, id: &str) -> Result<Vec<String>, TaskError> {
        let jobs = self.list()?;
        let mut targets = BTreeSet::from([id.to_string()]);
        loop {
            let before = targets.len();
            for job in &jobs {
                if job
                    .parent_job
                    .as_ref()
                    .map(|v| targets.contains(v))
                    .unwrap_or(false)
                {
                    targets.insert(job.id.clone());
                }
            }
            if targets.len() == before {
                break;
            }
        }
        let mut cancelled = Vec::new();
        for target in targets.iter().rev() {
            let mut job = self.get(target)?;
            if !job.status.terminal() {
                if let Some(pid) = job.pid {
                    terminate_group(pid, self.limits.kill_grace_ms);
                }
                job.status = JobStatus::Cancelled;
                job.pid = None;
                job.updated_at = now_millis();
                self.write_job(&job)?;
                cancelled.push(target.clone());
            }
        }
        Ok(cancelled)
    }
    pub fn merge(&self, id: &str) -> Result<JobRecord, TaskError> {
        let mut job = self.get(id)?;
        if !job.status.terminal() {
            return Err(TaskError::Conflict("cannot merge a running job".into()));
        }
        IsolatedWorkspace {
            path: job.workspace.clone(),
            strategy: job.isolation.clone(),
            parent: job.cwd.clone(),
        }
        .merge(&job.capture)?;
        job.metadata.insert("mergedAt".into(), json!(now_millis()));
        job.updated_at = now_millis();
        self.write_job(&job)?;
        Ok(job)
    }
    pub fn batch(&self, tasks: Vec<BatchTask>) -> Result<Vec<JobRecord>, TaskError> {
        if tasks.is_empty() || tasks.len() > self.limits.max_batch {
            return Err(TaskError::Invalid(format!(
                "batch size must be 1..{}",
                self.limits.max_batch
            )));
        }
        let ids = tasks.iter().map(|t| t.id.clone()).collect::<BTreeSet<_>>();
        if ids.len() != tasks.len() {
            return Err(TaskError::Invalid("batch task ids must be unique".into()));
        }
        if tasks
            .iter()
            .flat_map(|t| &t.depends_on)
            .any(|d| !ids.contains(d))
        {
            return Err(TaskError::Invalid("batch dependency is missing".into()));
        }
        let mut completed = BTreeSet::new();
        let mut results = Vec::new();
        while completed.len() < tasks.len() {
            let ready = tasks
                .iter()
                .filter(|t| {
                    !completed.contains(&t.id) && t.depends_on.iter().all(|d| completed.contains(d))
                })
                .take(self.limits.max_parallel)
                .cloned()
                .collect::<Vec<_>>();
            if ready.is_empty() {
                return Err(TaskError::Invalid("batch DAG contains a cycle".into()));
            }
            let mut wave = Vec::new();
            for task in ready {
                let job = self.spawn(SpawnRequest {
                    task: task.task,
                    agent: task.agent,
                    model: None,
                    max_steps: default_steps(),
                    parent_job: None,
                    isolate: Some(true),
                })?;
                wave.push((task.id, job.id));
            }
            for (task_id, job_id) in wave {
                let job = self.poll(&job_id, Duration::from_millis(self.limits.wait_timeout_ms))?;
                if !job.status.terminal() {
                    self.cancel(&job_id)?;
                    return Err(TaskError::Timeout(format!(
                        "batch task timed out: {task_id}"
                    )));
                }
                completed.insert(task_id);
                results.push(job);
            }
        }
        Ok(results)
    }
    pub fn health(&self) -> CapabilityHealth {
        let agents = discover_agents(&self.cwd);
        let jobs = self.list();
        let mut errors = Vec::new();
        if let Err(e) = &agents {
            errors.push(e.to_string());
        }
        if let Err(e) = &jobs {
            errors.push(e.to_string());
        }
        CapabilityHealth {
            id: "task-scheduler",
            healthy: errors.is_empty(),
            store: self.store.clone(),
            discovered_agents: agents.as_ref().map(|v| v.len()).unwrap_or(0),
            running: jobs
                .as_ref()
                .map(|v| v.iter().filter(|j| j.status == JobStatus::Running).count())
                .unwrap_or(0),
            limits: self.limits.clone(),
            isolation_strategies: vec!["apfs-clone", "git-worktree", "copy"],
            errors,
        }
    }
    fn recover(&self) -> Result<(), TaskError> {
        for mut job in self.list()? {
            if job.status == JobStatus::Running && !job.pid.map(process_alive).unwrap_or(false) {
                job.status = JobStatus::Interrupted;
                job.pid = None;
                job.updated_at = now_millis();
                job.error = Some(
                    "scheduler recovered job after its process exited without a terminal record"
                        .into(),
                );
                self.write_job(&job)?;
            }
        }
        self.clean_slots()
    }
}

fn agent_task_context(
    cwd: &Path,
    definition: &AgentDefinition,
    task: &str,
) -> Result<String, TaskError> {
    let contributions = if definition.skills.is_empty() {
        Vec::new()
    } else {
        crate::hooks::extension_skill_context(cwd, &definition.skills)
            .map_err(TaskError::Invalid)?
    };
    let mut context = String::new();
    if !definition.description.trim().is_empty() {
        context.push_str("Agent role: ");
        context.push_str(definition.description.trim());
        context.push_str("\n\n");
    }
    if !definition.prompt.trim().is_empty() {
        context.push_str("Agent instructions:\n");
        context.push_str(definition.prompt.trim());
        context.push_str("\n\n");
    }
    for skill in contributions {
        context.push_str("Declared skill ");
        context.push_str(&skill.id);
        context.push_str(":\n");
        context.push_str(&skill.content);
        if !skill.assets.is_empty() {
            context.push_str("\nValidated skill assets:\n");
            for asset in skill.assets {
                context.push_str("- ");
                context.push_str(&asset.display().to_string());
                context.push('\n');
            }
        }
        context.push('\n');
    }
    if !definition.tools.is_empty() {
        context.push_str("Hard tool allowlist: ");
        context.push_str(&definition.tools.join(", "));
        context.push_str(". Do not request any other tool.\n\n");
    }
    if !definition_output_is_unconstrained(&definition.output) {
        context.push_str("Required final JSON output schema:\n");
        context.push_str(&definition.output.to_string());
        context.push_str("\n\n");
    }
    context.push_str("Assigned task:\n");
    context.push_str(task);
    if context.len() > 1024 * 1024 {
        return Err(TaskError::Capacity {
            running: context.len(),
            limit: 1024 * 1024,
        });
    }
    Ok(context)
}

fn definition_output_is_unconstrained(schema: &serde_json::Value) -> bool {
    schema.is_null()
        || schema
            .as_object()
            .map(|object| object.is_empty())
            .unwrap_or(false)
}

fn validate_output(
    value: &serde_json::Value,
    schema: &serde_json::Value,
    path: &str,
) -> Result<(), String> {
    if let Some(allowed) = schema.get("enum").and_then(serde_json::Value::as_array) {
        if !allowed.contains(value) {
            return Err(format!("{path} is not one of the allowed enum values"));
        }
    }
    let expected = schema
        .get("type")
        .and_then(serde_json::Value::as_str)
        .or_else(|| schema.as_str());
    if let Some(expected) = expected {
        let matches = match expected {
            "null" => value.is_null(),
            "boolean" => value.is_boolean(),
            "number" => value.is_number(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "string" => value.is_string(),
            "array" => value.is_array(),
            "object" | "json" => value.is_object(),
            _ => return Err(format!("{path} uses unsupported schema type {expected}")),
        };
        if !matches {
            return Err(format!("{path} must be {expected}"));
        }
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(serde_json::Value::as_array) {
            for key in required.iter().filter_map(serde_json::Value::as_str) {
                if !object.contains_key(key) {
                    return Err(format!("{path}.{key} is required"));
                }
            }
        }
        if let Some(properties) = schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
        {
            for (key, property_schema) in properties {
                if let Some(property) = object.get(key) {
                    validate_output(property, property_schema, &format!("{path}.{key}"))?;
                }
            }
        }
    }
    if let (Some(items), Some(item_schema)) = (value.as_array(), schema.get("items")) {
        for (index, item) in items.iter().enumerate() {
            validate_output(item, item_schema, &format!("{path}[{index}]"))?;
        }
    }
    Ok(())
}

fn capture_pipe(mut pipe: impl Read, path: &Path, max_bytes: u64) -> Result<(), TaskError> {
    let mut file = fs::File::create(path)?;
    let mut buffer = [0u8; 8192];
    let mut written = 0u64;
    loop {
        let count = pipe.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = max_bytes.saturating_sub(written) as usize;
        let keep = remaining.min(count);
        if keep > 0 {
            file.write_all(&buffer[..keep])?;
            written += keep as u64;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    unsafe { kill(pid as i32, 0) == 0 }
}
#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    false
}
#[cfg(unix)]
fn terminate_group(pid: u32, grace_ms: u64) {
    unsafe {
        kill(-(pid as i32), 15);
    }
    thread::sleep(Duration::from_millis(grace_ms));
    if process_alive(pid) {
        unsafe {
            kill(-(pid as i32), 9);
        }
    }
}
#[cfg(not(unix))]
fn terminate_group(_pid: u32, _grace_ms: u64) {}
#[cfg(unix)]
fn configure_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}
#[cfg(not(unix))]
fn configure_group(_command: &mut Command) {}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn spawned_task_is_confined_to_its_write_roots() {
        let root = std::env::temp_dir().join(format!(
            "jeden-task-sandbox-test-{}-{}",
            std::process::id(),
            now_millis()
        ));
        let cwd = root.join("workspace");
        let store = root.join("store");
        fs::create_dir_all(&cwd).unwrap();
        let denied = root.join("outside-write");
        let allowed = store.join("inside-write");
        let executable = cwd.join("fake-jeden");
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nif printf escaped > '{}'; then exit 91; fi\nprintf confined > '{}'\nprintf TASK_OK\n",
                denied.display(),
                allowed.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        let mut scheduler = TaskScheduler::open(&cwd, &store, TaskLimits::default()).unwrap();
        scheduler.exe = executable;
        let spawned = scheduler
            .spawn(SpawnRequest {
                task: "sandbox smoke".into(),
                agent: "default".into(),
                model: None,
                max_steps: 1,
                parent_job: None,
                isolate: None,
            })
            .unwrap();
        let completed = scheduler
            .poll(&spawned.id, Duration::from_secs(10))
            .unwrap();

        assert_eq!(completed.status, JobStatus::Succeeded, "{completed:?}");
        assert!(!denied.exists(), "sandbox allowed an out-of-root write");
        assert_eq!(fs::read_to_string(allowed).unwrap(), "confined");
        fs::remove_dir_all(root).unwrap();
    }
}
