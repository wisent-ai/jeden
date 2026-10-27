use std::num::{NonZeroU32, NonZeroU64};
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

use crate::hillclimb::{Climb, Scope};
use crate::{Args, Command};

type Failure = (&'static str, String);

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
