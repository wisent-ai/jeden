//! The shared native loop used by the CLI and RPC, with caller-owned controls.
use std::num::NonZeroU64;

use serde::Serialize;

use super::{another, ledger, place, round, status::Report, usage, Climb, Scope};
use crate::agent::RunHooks;
use crate::Args;

#[derive(Serialize)]
pub(crate) struct Outcome {
    pub(super) scope: Scope,
    #[serde(flatten)]
    pub(super) report: Report,
}

pub(crate) fn run(
    args: &Args,
    climb: Climb,
    hooks: &mut RunHooks<'static>,
) -> Result<Outcome, String> {
    if !(args.allow_write && args.allow_command) {
        return Err(usage(
            "hillclimb changes the code it improves and runs its checks: it needs --allow-write and --allow-command, or --yolo",
        ));
    }
    let where_ = place(climb.scope, &args.cwd, &climb.only)?;
    let ledger_path = ledger::path(&args.cwd, climb.ledger.as_deref())?;
    let cancelled = || {
        format!(
            "hillclimb cancelled; completed rounds remain in ledger {}",
            ledger_path.display()
        )
    };
    if hooks.cancelled() {
        return Err(cancelled());
    }
    let _ownership = ledger::claim(&ledger_path)?;
    let mut rounds = ledger::read(&ledger_path)?;
    let first_new = rounds.len();
    (hooks.progress)(&format!(
        "hillclimb: {where_}; ledger {} ({} earlier rounds)",
        ledger_path.display(),
        rounds.len()
    ));
    while another(climb.rounds, rounds.len() - first_new) {
        if hooks.cancelled() {
            return Err(cancelled());
        }
        let number = rounds.len() as u64 + NonZeroU64::MIN.get();
        let ended = round(args, number, &where_, &climb, &rounds, hooks)?;
        ledger::append(&ledger_path, &ended)?;
        (hooks.progress)(&format!(
            "hillclimb {}",
            ledger::digest(std::slice::from_ref(&ended))
        ));
        let repeated = match (rounds.last(), &ended.error) {
            (Some(previous), Some(error)) if previous.error.as_deref() == Some(error.as_str()) => {
                Some(error.clone())
            }
            _ => None,
        };
        rounds.push(ended);
        if hooks.cancelled() {
            return Err(cancelled());
        }
        if let Some(error) = repeated {
            return Err(format!(
                "hillclimb stopped after round {number}: it failed as the round before it did, so another round would fail again: {error}; the ledger is {}",
                ledger_path.display()
            ));
        }
    }
    // Reuse the existing allocation; only this invocation's rounds are returned.
    drop(rounds.drain(..first_new));
    Ok(Outcome {
        scope: climb.scope,
        report: Report::new(ledger_path, rounds),
    })
}
