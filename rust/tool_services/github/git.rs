//! The git-side operations of the GitHub service: worktrees and the guarded
//! push.
//!
//! Split out of `github.rs` on the seam between "talks to GitHub through `gh`"
//! and "runs `git` in the checkout", because the operator's rule keeps every
//! source file at three hundred lines or fewer and the worktree change below
//! could not otherwise be made at all.
//!
//! `add` is gone on purpose. The operator's instruction was
//! "MA BYC NIEMOZLIWE UZYCIE WORKTREES. ZERO SUBAGENTOW NA OSOBNYCH
//! WORKTREES", and this tool was the one worktree producer a model could
//! actually invoke here: it runs `git worktree <action>` through the process
//! helper, so it never passes the shell and no `pre_tool_use:bash` guard sees
//! it. Removing the action removes the capability; guarding it would have
//! left a refusal in front of a door that should not exist.
//!
//! `list`, `remove` and `prune` stay, because that is how a machine that
//! already holds worktrees is cleaned up.

use super::super::process;
use super::super::types::{bounded_json, nonempty, ServiceError, ServiceResult};
use super::GithubService;
use crate::tool_runtime::runtime_ops::OperationContext;
use serde_json::{json, Value};
use std::time::Duration;

/// Refusal for the retired creation action, naming what to do instead.
const ADD_REFUSED: &str = "worktree add is not available: this machine holds one checkout per \
                           repository. Work in the existing checkout, and use list, remove or \
                           prune to clean up worktrees that already exist.";

impl GithubService {
    pub(super) fn worktree(
        &self,
        input: &Value,
        context: &OperationContext<'_>,
        allow_write: bool,
    ) -> ServiceResult<Value> {
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("list");
        let mut args = vec!["worktree".into(), action.into()];
        match action {
            "list" => args.push("--porcelain".into()),
            "add" => return Err(ServiceError::PermissionDenied(ADD_REFUSED.into())),
            "remove" => {
                if !allow_write {
                    return Err(ServiceError::PermissionDenied(
                        "worktree remove requires write permission".into(),
                    ));
                }
                args.push(nonempty(input.get("path"), "path")?);
            }
            "prune" => {
                if !allow_write {
                    return Err(ServiceError::PermissionDenied(
                        "worktree prune requires write permission".into(),
                    ));
                }
            }
            _ => {
                return Err(ServiceError::InvalidInput(
                    "worktree action must be list, remove, or prune".into(),
                ))
            }
        }
        let text = process::run(
            "github",
            context,
            &self.cwd,
            "git",
            &args,
            None,
            Duration::from_secs(30),
        )?;
        bounded_json(context, "github", &json!({"ok":true,"output":text}))
    }

    pub(super) fn guarded_push(
        &self,
        input: &Value,
        context: &OperationContext<'_>,
        allow_write: bool,
    ) -> ServiceResult<Value> {
        if !allow_write {
            return Err(ServiceError::PermissionDenied(
                "push requires write permission".into(),
            ));
        }
        if input.get("confirm").and_then(Value::as_bool) != Some(true) {
            return Err(ServiceError::PermissionDenied(
                "push requires confirm=true".into(),
            ));
        }
        if input.get("force").and_then(Value::as_bool) == Some(true) {
            return Err(ServiceError::PermissionDenied(
                "force push is not supported".into(),
            ));
        }
        let status = process::run(
            "github",
            context,
            &self.cwd,
            "git",
            &["status".into(), "--porcelain".into()],
            None,
            Duration::from_secs(10),
        )?;
        if !status.trim().is_empty() {
            return Err(ServiceError::PermissionDenied(
                "refusing to push a dirty worktree".into(),
            ));
        }
        let branch = process::run(
            "github",
            context,
            &self.cwd,
            "git",
            &["branch".into(), "--show-current".into()],
            None,
            Duration::from_secs(10),
        )?
        .trim()
        .to_string();
        if branch.is_empty() {
            return Err(ServiceError::PermissionDenied(
                "refusing to push detached HEAD".into(),
            ));
        }
        if !safe_git_name(&branch) {
            return Err(ServiceError::PermissionDenied(
                "checked-out branch contains unsafe characters".into(),
            ));
        }
        if let Some(expected) = input.get("branch").and_then(Value::as_str) {
            if expected != branch {
                return Err(ServiceError::PermissionDenied(format!(
                    "checked-out branch is {branch}, not {expected}"
                )));
            }
        }
        let remote = input
            .get("remote")
            .and_then(Value::as_str)
            .unwrap_or("origin");
        if !safe_git_name(remote) {
            return Err(ServiceError::InvalidInput(
                "remote contains unsafe characters".into(),
            ));
        }
        let output = process::run(
            "github",
            context,
            &self.cwd,
            "git",
            &[
                "push".into(),
                remote.into(),
                format!("HEAD:refs/heads/{branch}"),
                "--porcelain".into(),
            ],
            None,
            Duration::from_secs(120),
        )?;
        bounded_json(
            context,
            "github",
            &json!({"ok":true,"remote":remote,"branch":branch,"output":output}),
        )
    }
}

fn safe_git_name(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
}
