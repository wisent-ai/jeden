mod browser;
mod config;
mod debugger;
mod github;
mod media;
mod process;
mod ssh;
mod types;
mod web;

pub use types::{HealthDescriptor, HealthStatus};

use crate::capability::{
    CapabilityDescriptor, CapabilityHealth, CapabilityKind, CapabilityPolicy, FunctionTarget,
};
use crate::tool_runtime::{
    register_dynamic_tools, DynamicToolDescriptor, DynamicToolRegistration, ToolRuntime,
};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

struct ServiceHub {
    browser: browser::BrowserService,
    debugger: debugger::DebuggerService,
    github: github::GithubService,
    ssh: ssh::SshService,
    media: media::MediaService,
    web: web::WebService,
}
impl ServiceHub {
    fn discover(cwd: &Path) -> Self {
        let value = config::discover(cwd);
        Self {
            browser: browser::BrowserService::discover(cwd, &value),
            debugger: debugger::DebuggerService::discover(cwd, &value),
            github: github::GithubService::discover(cwd, &value),
            ssh: ssh::SshService::discover(cwd, &value),
            media: media::MediaService::discover(cwd, &value),
            web: web::WebService::discover(cwd, &value),
        }
    }
    fn health(&self, tool: &str) -> HealthDescriptor {
        if browser::TOOLS.iter().any(|(name, _)| *name == tool) {
            self.browser.health()
        } else if debugger::TOOLS.iter().any(|(name, _)| *name == tool) {
            self.debugger.health()
        } else if github::TOOLS.iter().any(|(name, _)| *name == tool) {
            self.github.health_for(tool)
        } else if ssh::TOOLS.iter().any(|(name, _)| *name == tool) {
            self.ssh.health()
        } else if media::TOOLS.iter().any(|(name, _)| *name == tool) {
            self.media.health_for(tool)
        } else {
            self.web.health()
        }
    }
    fn execute(
        &self,
        runtime: &ToolRuntime<'_>,
        tool: &str,
        input: &Value,
    ) -> Result<Value, String> {
        let result = if browser::TOOLS.iter().any(|(name, _)| *name == tool) {
            self.browser.execute(tool, input, &runtime.operation)
        } else if debugger::TOOLS.iter().any(|(name, _)| *name == tool) {
            self.debugger.execute(tool, input, &runtime.operation)
        } else if github::TOOLS.iter().any(|(name, _)| *name == tool) {
            self.github.execute(
                tool,
                input,
                &runtime.operation,
                runtime.allow_write,
                runtime.allow_command,
            )
        } else if ssh::TOOLS.iter().any(|(name, _)| *name == tool) {
            self.ssh.execute(
                tool,
                input,
                &runtime.operation,
                runtime.allow_write,
                runtime.allow_command,
            )
        } else if media::TOOLS.iter().any(|(name, _)| *name == tool) {
            self.media.execute(tool, input, &runtime.operation)
        } else if web::TOOLS.iter().any(|(name, _)| *name == tool) {
            self.web.execute(input, &runtime.operation)
        } else {
            return Err(format!("unknown tool service: {tool}"));
        };
        result.map_err(|error| error.to_string())
    }
}

static HUBS: LazyLock<Mutex<BTreeMap<PathBuf, (u64, Arc<ServiceHub>)>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
fn canonical(cwd: &Path) -> PathBuf {
    cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf())
}
fn hub(cwd: &Path) -> Arc<ServiceHub> {
    let cwd = canonical(cwd);
    let fingerprint = config::fingerprint(&cwd);
    let mut hubs = HUBS.lock();
    if let Some((cached, hub)) = hubs.get(&cwd) {
        if *cached == fingerprint {
            return hub.clone();
        }
    }
    if hubs.len() >= 32 && !hubs.contains_key(&cwd) {
        if let Some(oldest) = hubs.keys().next().cloned() {
            hubs.remove(&oldest);
        }
    }
    let service = Arc::new(ServiceHub::discover(&cwd));
    hubs.insert(cwd, (fingerprint, service.clone()));
    service
}

fn tool_specs() -> impl Iterator<Item = (&'static str, &'static str)> {
    browser::TOOLS
        .iter()
        .chain(debugger::TOOLS)
        .chain(web::TOOLS)
        .chain(github::TOOLS)
        .chain(ssh::TOOLS)
        .chain(media::TOOLS)
        .copied()
}
fn dynamic_descriptors() -> Vec<DynamicToolDescriptor> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let services = hub(&cwd);
    tool_specs()
        .map(|(name, description)| {
            let health = services.health(name);
            DynamicToolDescriptor {
                name: name.into(),
                description: description.into(),
                input: json!({"type":"object","additionalProperties":true}),
                healthy: health.available(),
                health: health.detail,
            }
        })
        .collect()
}
fn execute_dynamic(
    runtime: &ToolRuntime<'_>,
    tool: &str,
    input: &Value,
) -> Option<Result<Value, String>> {
    if !tool_specs().any(|(name, _)| name == tool) {
        return None;
    }
    Some(hub(runtime.cwd).execute(runtime, tool, input))
}

pub(crate) fn register_with_tool_runtime() -> Result<(), String> {
    register_dynamic_tools(DynamicToolRegistration {
        owner: "tool-services",
        descriptors: dynamic_descriptors,
        execute: execute_dynamic,
    })?;
    crate::capability::invalidate();
    Ok(())
}

pub(crate) fn capability_descriptors(cwd: &Path) -> Vec<CapabilityDescriptor> {
    let services = hub(cwd);
    tool_specs()
        .map(|(name, description)| {
            let health = services.health(name);
            let capability_health = match health.status {
                HealthStatus::Healthy => CapabilityHealth::healthy(),
                HealthStatus::Unavailable => CapabilityHealth::unavailable(health.detail.clone()),
            };
            let policy = if matches!(
                name,
                "github_issue"
                    | "github_pr"
                    | "github_actions"
                    | "git_worktree"
                    | "git_guarded_push"
                    | "ssh_write"
                    | "ssh_exec"
            ) {
                CapabilityPolicy::ApprovalRequired
            } else {
                CapabilityPolicy::ReadOnly
            };
            let mut descriptor = CapabilityDescriptor::new(
                format!("tool/{name}"),
                CapabilityKind::Tool,
                "tool-services",
                name,
                description,
                FunctionTarget::BuiltinTool { name: name.into() },
            )
            .operation(name)
            .policy(policy)
            .health(capability_health)
            .metadata(json!({"service":health.service,"backend":health.backend}));
            if health.available() {
                descriptor = descriptor.executable(name);
            }
            descriptor
        })
        .collect()
}

#[cfg(test)]
mod tests;
