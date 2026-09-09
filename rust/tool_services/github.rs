use super::process;
use super::types::{
    bounded_json, command_exists, nonempty, HealthDescriptor, ServiceError, ServiceResult,
};
use crate::tool_runtime::runtime_ops::OperationContext;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

mod git;

pub(crate) const TOOLS: &[(&str, &str)] = &[
    (
        "github_issue",
        "List, view, create, edit, comment on, or close GitHub issues",
    ),
    (
        "github_pr",
        "List, view, create, review, merge, comment on, or close GitHub pull requests",
    ),
    (
        "github_search",
        "Search GitHub repositories, code, issues, or pull requests",
    ),
    (
        "github_actions",
        "List, inspect, dispatch, rerun, or cancel GitHub Actions runs",
    ),
    ("git_worktree", "List, remove, or prune Git worktrees"),
    (
        "git_guarded_push",
        "Push a clean checked-out branch after explicit confirmation and safety checks",
    ),
];
pub(crate) struct GithubService {
    cwd: PathBuf,
    gh: bool,
    git: bool,
}
impl GithubService {
    pub(crate) fn discover(cwd: &Path, _value: &Value) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
            gh: command_exists("gh"),
            git: command_exists("git"),
        }
    }
    pub(crate) fn health_for(&self, tool: &str) -> HealthDescriptor {
        if matches!(tool, "git_worktree" | "git_guarded_push") {
            if self.git {
                HealthDescriptor::healthy("github", "git")
            } else {
                HealthDescriptor::unavailable("github", "git executable not found")
            }
        } else if self.gh {
            HealthDescriptor::healthy("github", "gh")
        } else {
            HealthDescriptor::unavailable("github", "GitHub CLI (gh) executable not found")
        }
    }
    pub(crate) fn execute(
        &self,
        tool: &str,
        input: &Value,
        context: &OperationContext<'_>,
        allow_write: bool,
        allow_command: bool,
    ) -> ServiceResult<Value> {
        if !allow_command {
            return Err(ServiceError::PermissionDenied(
                "GitHub and git services require command permission".into(),
            ));
        }
        let health = self.health_for(tool);
        if !health.available() {
            return Err(ServiceError::Unavailable {
                service: "github",
                detail: health.detail,
            });
        }
        match tool {
            "github_issue" => self.gh_resource("issue", input, context, allow_write),
            "github_pr" => self.gh_resource("pr", input, context, allow_write),
            "github_search" => self.search(input, context),
            "github_actions" => self.actions(input, context, allow_write),
            "git_worktree" => self.worktree(input, context, allow_write),
            "git_guarded_push" => self.guarded_push(input, context, allow_write),
            _ => Err(ServiceError::InvalidInput(format!(
                "unknown GitHub tool {tool}"
            ))),
        }
    }
    fn gh_resource(
        &self,
        resource: &str,
        input: &Value,
        context: &OperationContext<'_>,
        allow_write: bool,
    ) -> ServiceResult<Value> {
        let action = nonempty(input.get("action"), "action")?;
        let mut args = vec![resource.into(), action.clone()];
        let mutating = matches!(
            action.as_str(),
            "create" | "edit" | "comment" | "close" | "reopen" | "merge" | "review"
        );
        if mutating && !allow_write {
            return Err(ServiceError::PermissionDenied(format!(
                "gh {resource} {action} requires write permission"
            )));
        }
        append_gh_arguments(&mut args, input)?;
        if !mutating {
            args.extend([
                "--json".into(),
                if resource == "issue" {
                    "number,title,state,url,labels,assignees,body"
                } else {
                    "number,title,state,url,headRefName,baseRefName,mergeable,body"
                }
                .into(),
            ]);
        }
        self.run_gh(context, args)
    }
    fn search(&self, input: &Value, context: &OperationContext<'_>) -> ServiceResult<Value> {
        let kind = input
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("issues");
        if !matches!(kind, "repos" | "code" | "issues" | "prs" | "commits") {
            return Err(ServiceError::InvalidInput(
                "search kind must be repos, code, issues, prs, or commits".into(),
            ));
        }
        let fields = match kind {
            "repos" => "fullName,description,url,visibility,updatedAt",
            "code" => "path,repository,url,sha",
            "issues" | "prs" => "number,title,state,url,repository,updatedAt",
            "commits" => "sha,url,repository,commit",
            _ => unreachable!(),
        };
        let mut args = vec![
            "search".into(),
            kind.into(),
            nonempty(input.get("query"), "query")?,
            "--json".into(),
            fields.into(),
            "--limit".into(),
            input
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20)
                .clamp(1, 100)
                .to_string(),
        ];
        if let Some(repo) = input.get("repo").and_then(Value::as_str) {
            args.extend(["--repo".into(), repo.into()]);
        }
        self.run_gh(context, args)
    }
    fn actions(
        &self,
        input: &Value,
        context: &OperationContext<'_>,
        allow_write: bool,
    ) -> ServiceResult<Value> {
        let action = nonempty(input.get("action"), "action")?;
        let mut args = match action.as_str() {
            "list" => vec![
                "run".into(),
                "list".into(),
                "--json".into(),
                "databaseId,name,status,conclusion,url,headBranch,event".into(),
            ],
            "view" => vec![
                "run".into(),
                "view".into(),
                nonempty(input.get("run"), "run")?,
                "--json".into(),
                "databaseId,name,status,conclusion,url,jobs".into(),
            ],
            "dispatch" => {
                if !allow_write {
                    return Err(ServiceError::PermissionDenied(
                        "workflow dispatch requires write permission".into(),
                    ));
                }
                vec![
                    "workflow".into(),
                    "run".into(),
                    nonempty(input.get("workflow"), "workflow")?,
                ]
            }
            "rerun" | "cancel" => {
                if !allow_write {
                    return Err(ServiceError::PermissionDenied(format!(
                        "run {action} requires write permission"
                    )));
                }
                vec!["run".into(), action, nonempty(input.get("run"), "run")?]
            }
            _ => {
                return Err(ServiceError::InvalidInput(
                    "actions action must be list, view, dispatch, rerun, or cancel".into(),
                ))
            }
        };
        if let Some(repo) = input.get("repo").and_then(Value::as_str) {
            args.extend(["--repo".into(), repo.into()]);
        }
        self.run_gh(context, args)
    }
    fn run_gh(&self, context: &OperationContext<'_>, args: Vec<String>) -> ServiceResult<Value> {
        let output = process::run(
            "github",
            context,
            &self.cwd,
            "gh",
            &args,
            None,
            Duration::from_secs(60),
        )?;
        let value =
            serde_json::from_str(&output).unwrap_or_else(|_| json!({"ok":true,"output":output}));
        bounded_json(context, "github", &value)
    }
}
fn append_gh_arguments(args: &mut Vec<String>, input: &Value) -> ServiceResult<()> {
    if let Some(number) = input.get("number").and_then(Value::as_u64) {
        args.push(number.to_string());
    }
    for (key, flag) in [
        ("repo", "--repo"),
        ("title", "--title"),
        ("body", "--body"),
        ("label", "--label"),
        ("assignee", "--assignee"),
        ("base", "--base"),
        ("head", "--head"),
    ] {
        if let Some(value) = input.get(key).and_then(Value::as_str) {
            args.extend([flag.into(), value.into()]);
        }
    }
    Ok(())
}
