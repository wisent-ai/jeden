use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

use crate::hillclimb::{Climb, Ending, Scope};
use crate::{Args, Command};

type Failure = (&'static str, String);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HistoryParameters<'a> {
    #[serde(borrow)]
    pub(super) cwd: &'a str,
    #[serde(borrow)]
    pub(super) ledger: Option<&'a str>,
    pub(super) last: Option<NonZeroUsize>,
    #[serde(default, deserialize_with = "history_ending")]
    pub(super) ending: Option<Ending>,
    #[serde(default, deserialize_with = "history_round")]
    pub(super) round: Option<u64>,
    #[serde(borrow, default, deserialize_with = "history_repository")]
    pub(super) repository: Option<&'a str>,
    #[serde(rename = "withoutRepository", default, deserialize_with = "history_without_repository")]
    pub(super) without_repository: bool,
}

pub(super) fn history(value: &Value) -> Result<HistoryParameters<'_>, Failure> {
    let parameters = HistoryParameters::deserialize(value)
        .map_err(|error| ("invalid_params", format!("hillclimb/status parameters cannot be read: {error}")))?;
    if parameters.cwd.trim().is_empty() {
        return Err(("invalid_params", format!(
            "hillclimb/status requires nonempty cwd, the workspace whose climb to read; got {:?}", parameters.cwd
        )));
    }
    if parameters.ledger.is_some_and(|path| path.trim().is_empty()) {
        return Err(("invalid_params", format!(
            "hillclimb/status ledger must be a nonempty path string when supplied; got {:?}", parameters.ledger
        )));
    }
    Ok(parameters)
}

fn history_ending<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<Option<Ending>, D::Error> {
    let refused = |error: D::Error| <D::Error as serde::de::Error>::custom(format!("ending: {error}"));
    let value = Option::<&str>::deserialize(decoder).map_err(refused)?;
    value.map(|value| {
        Ending::deserialize(serde::de::value::StrDeserializer::<D::Error>::new(value))
    }).transpose().map_err(refused)
}

fn history_round<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<Option<u64>, D::Error> {
    Option::<u64>::deserialize(decoder).map_err(|error|
        <D::Error as serde::de::Error>::custom(format!("round: {error}")))
}

fn history_repository<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<Option<&'de str>, D::Error> {
    Option::<&str>::deserialize(decoder).map_err(|error|
        <D::Error as serde::de::Error>::custom(format!("repository: {error}")))
}

fn history_without_repository<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<bool, D::Error> {
    bool::deserialize(decoder).map_err(|error|
        <D::Error as serde::de::Error>::custom(format!("withoutRepository: {error}")))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Parameters {
    cwd: String,
    #[serde(default)]
    scope: Scope,
    #[serde(default)]
    focus: String,
    #[serde(default)]
    only: Vec<String>,
    rounds: Option<NonZeroU64>,
    ledger: Option<PathBuf>,
    model: Option<String>,
    max_steps: Option<NonZeroU32>,
    max_tokens: Option<NonZeroU32>,
    #[serde(default)]
    allow_write: bool,
    #[serde(default)]
    allow_command: bool,
}

/// Adapt the protocol to native values, never to a shell or CLI argument string.
pub(super) fn parse(value: Value) -> Result<(Args, Climb), Failure> {
    let parameters: Parameters = serde_json::from_value(value)
        .map_err(|error| ("invalid_params", format!("hillclimb/run parameters cannot be read: {error}")))?;
    if parameters.cwd.trim().is_empty() {
        return Err(("invalid_params", "hillclimb/run requires nonempty cwd, the workspace to improve".into()));
    }
    if !(parameters.allow_write && parameters.allow_command) {
        return Err(("permission_denied", "hillclimb/run changes code and runs its checks: it requires allowWrite=true and allowCommand=true".into()));
    }
    let args = Args {
        command: Command::Hillclimb,
        cwd: PathBuf::from(parameters.cwd),
        cwd_explicit: true,
        model: parameters.model,
        max_steps: parameters.max_steps.map(NonZeroU32::get),
        max_tokens: parameters.max_tokens.map(NonZeroU32::get),
        allow_write: parameters.allow_write,
        allow_command: parameters.allow_command,
        autonomous: true,
        ..Args::default()
    };
    let climb = Climb {
        scope: parameters.scope,
        rounds: parameters.rounds,
        ledger: parameters.ledger,
        focus: parameters.focus,
        only: parameters.only,
    };
    Ok((args, climb))
}
