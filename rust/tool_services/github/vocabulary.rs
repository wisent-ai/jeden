//! The closed vocabularies the GitHub tools accept, each spelled as the `gh`
//! subcommand it becomes, and the one reader that refuses any other spelling.

use super::super::types::{ServiceError, ServiceResult};
use serde_json::Value;

/// The value of a declared vocabulary spelled `named`, or a refusal that
/// names `what` and every spelling it accepts.
pub(super) fn declared<T>(named: &str, what: &str) -> ServiceResult<T>
where
    T: serde::de::DeserializeOwned + Vocabulary,
{
    serde_json::from_value(Value::String(named.to_owned())).map_err(|_not_a_spelling| {
        ServiceError::InvalidInput(format!(
            "{what} must be one of {}, not {named}",
            T::choices()
        ))
    })
}

/// A closed vocabulary whose spellings are its serde names.
pub(super) trait Vocabulary: serde::Serialize + Sized + 'static {
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
pub(super) enum SearchKind {
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
    pub(super) fn fields(self) -> &'static str {
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
pub(super) enum ResourceAction {
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
    pub(super) fn writes(self) -> bool {
        !matches!(self, ResourceAction::List | ResourceAction::View)
    }
}

/// What `github_actions` does to workflow runs; the ones that start, rerun
/// or cancel a run need write permission.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum RunAction {
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
    pub(super) fn writes(self) -> bool {
        !matches!(self, RunAction::List | RunAction::View)
    }
}
