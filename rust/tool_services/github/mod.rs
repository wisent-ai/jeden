use super::process;
use super::types::{
    bounded_json, command_exists, nonempty, HealthDescriptor, ServiceError, ServiceResult,
};
use crate::tool_runtime::runtime_ops::OperationContext;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

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
        "Search GitHub repositories, code, issues, or pull requests (kind, query and limit are required; repo narrows)",
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
    /// A `git_` tool needs git; every other tool here drives the GitHub CLI.
    pub(crate) fn health_for(&self, tool: &str) -> HealthDescriptor {
        if tool.starts_with("git_") {
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
        let named = nonempty(input.get("action"), "action")?;
        let action: ResourceAction = declared(&named, "action")?;
        let mut args = vec![resource.into(), named.clone()];
        let mutating = action.writes();
        if mutating && !allow_write {
            return Err(ServiceError::PermissionDenied(format!(
                "gh {resource} {named} requires write permission"
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
        let named = nonempty(input.get("kind"), "kind")?;
        let kind: SearchKind = declared(&named, "search kind")?;
        let mut args = vec![
            "search".into(),
            named,
            nonempty(input.get("query"), "query")?,
            "--json".into(),
            kind.fields().into(),
            "--limit".into(),
            input
                .get("limit")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    ServiceError::InvalidInput(
                        "search needs limit: how many results gh search returns".into(),
                    )
                })?
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
        let named = nonempty(input.get("action"), "action")?;
        let action: RunAction = declared(&named, "actions action")?;
        if action.writes() && !allow_write {
            return Err(ServiceError::PermissionDenied(format!(
                "run {named} requires write permission"
            )));
        }
        let mut args = match action {
            RunAction::List => vec![
                "run".into(),
                named,
                "--json".into(),
                "databaseId,name,status,conclusion,url,headBranch,event".into(),
            ],
            RunAction::View => vec![
                "run".into(),
                named,
                nonempty(input.get("run"), "run")?,
                "--json".into(),
                "databaseId,name,status,conclusion,url,jobs".into(),
            ],
            RunAction::Dispatch => vec![
                "workflow".into(),
                "run".into(),
                nonempty(input.get("workflow"), "workflow")?,
            ],
            RunAction::Rerun | RunAction::Cancel => {
                vec!["run".into(), named, nonempty(input.get("run"), "run")?]
            }
        };
        if let Some(repo) = input.get("repo").and_then(Value::as_str) {
            args.extend(["--repo".into(), repo.into()]);
        }
        self.run_gh(context, args)
    }
    fn run_gh(&self, context: &OperationContext<'_>, args: Vec<String>) -> ServiceResult<Value> {
        let output = process::run("github", context, &self.cwd, "gh", &args, None)?;
        let value =
            serde_json::from_str(&output).unwrap_or_else(|_| json!({"ok":true,"output":output}));
        bounded_json(context, "github", &value)
    }
}

/// The value of a declared vocabulary spelled `named`, or a refusal that
/// names `what` and every spelling it accepts.
fn declared<T>(named: &str, what: &str) -> ServiceResult<T>
where
    T: serde::de::DeserializeOwned + Vocabulary,
{
    serde_json::from_value(Value::String(named.to_owned())).map_err(|_| {
        ServiceError::InvalidInput(format!(
            "{what} must be one of {}, not {named}",
            T::choices()
        ))
    })
}

/// A closed vocabulary whose spellings are its serde names.
trait Vocabulary: serde::Serialize + Sized + 'static {
    const ALL: &'static [Self];
    fn choices() -> String {
        Self::ALL
            .iter()
            .filter_map(|value| match serde_json::to_value(value) {
                Ok(Value::String(name)) => Some(name),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// What `gh search` searches, spelled as its subcommand
/// (https://cli.github.com/manual/gh_search), with the JSON fields asked of each.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum SearchKind {
    Repos,
    Code,
    Issues,
    Prs,
    Commits,
}

impl Vocabulary for SearchKind {
    const ALL: &'static [Self] = &[
        SearchKind::Repos,
        SearchKind::Code,
        SearchKind::Issues,
        SearchKind::Prs,
        SearchKind::Commits,
    ];
}

impl SearchKind {
    fn fields(self) -> &'static str {
        match self {
            SearchKind::Repos => "fullName,description,url,visibility,updatedAt",
            SearchKind::Code => "path,repository,url,sha",
            SearchKind::Issues | SearchKind::Prs => "number,title,state,url,repository,updatedAt",
            SearchKind::Commits => "sha,url,repository,commit",
        }
    }
}

/// What `gh issue` and `gh pr` are asked to do, spelled as their
/// subcommands; the ones that change GitHub need write permission.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum ResourceAction {
    List,
    View,
    Create,
    Edit,
    Comment,
    Close,
    Reopen,
    Merge,
    Review,
}

impl Vocabulary for ResourceAction {
    const ALL: &'static [Self] = &[
        ResourceAction::List,
        ResourceAction::View,
        ResourceAction::Create,
        ResourceAction::Edit,
        ResourceAction::Comment,
        ResourceAction::Close,
        ResourceAction::Reopen,
        ResourceAction::Merge,
        ResourceAction::Review,
    ];
}

impl ResourceAction {
    fn writes(self) -> bool {
        !matches!(self, ResourceAction::List | ResourceAction::View)
    }
}

/// What `github_actions` does to workflow runs; the ones that start, rerun
/// or cancel a run need write permission.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum RunAction {
    List,
    View,
    Dispatch,
    Rerun,
    Cancel,
}

impl Vocabulary for RunAction {
    const ALL: &'static [Self] = &[
        RunAction::List,
        RunAction::View,
        RunAction::Dispatch,
        RunAction::Rerun,
        RunAction::Cancel,
    ];
}

impl RunAction {
    fn writes(self) -> bool {
        !matches!(self, RunAction::List | RunAction::View)
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
