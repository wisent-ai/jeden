//! `jeden hillclimb`: improve a repository, or every repository of an
//! organization, round after round without end. Each round a fresh read-only
//! conversation reads the product and the ledger of earlier rounds and chooses
//! the one next improvement — a capability the product lacks, or an existing
//! one made better — as a rough objective; Pursuit then turns it into a
//! contract, carries it out and has it independently accepted or rejected, and
//! the round is written to the ledger before the next one chooses.
//!
//! The climb stops when `--rounds` rounds have run, when the operator stops
//! it, or when a round fails the way the round before it failed: the same
//! failure twice means nothing the climb can choose will get past it, and it
//! is reported with its cause instead of being repeated.

pub(crate) mod execution;
mod ledger;
pub(crate) mod status;
mod target;

use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent::{Conversation, RunHooks};
use crate::cli::invocation::refusal::usage;
use crate::Args;
use ledger::{Ending, Round};

const PROMPT: &str = include_str!("prompt.txt");

/// What one climb improves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Scope {
    /// The one repository `--cwd` is.
    #[default]
    Repository,
    /// Every repository directly under `--cwd`.
    Organization,
}

/// The hill-climb's own options, read from the positionals the invocation
/// parser handed on; what is left is the focus.
pub(crate) struct Climb {
    pub(crate) scope: Scope,
    pub(crate) rounds: Option<NonZeroU64>,
    pub(crate) ledger: Option<PathBuf>,
    pub(crate) focus: String,
    /// The repositories an organization climb keeps to; none means all of them.
    pub(crate) only: Vec<String>,
}

fn options(positionals: &[String]) -> Result<Climb, String> {
    let mut climb = Climb {
        scope: Scope::default(),
        rounds: None,
        ledger: None,
        focus: String::new(),
        only: Vec::new(),
    };
    let mut focus = Vec::new();
    let mut words = positionals.iter();
    while let Some(word) = words.next() {
        let mut value = |flag: &str| {
            words
                .next()
                .cloned()
                .ok_or_else(|| usage(format!("hillclimb {flag} requires a value")))
        };
        match word.as_str() {
            "--scope" => {
                climb.scope = crate::vocabulary::named(&value("--scope")?)
                    .map_err(|cause| usage(format!("hillclimb --scope: {cause}")))?
            }
            "--rounds" => {
                let text = value("--rounds")?;
                climb.rounds = Some(text.parse().map_err(|error| {
                    usage(format!(
                        "hillclimb --rounds must be a whole number of rounds above zero, not {text:?} ({error}); leave it out to climb without end"
                    ))
                })?);
            }
            "--ledger" => climb.ledger = Some(PathBuf::from(value("--ledger")?)),
            "--only" => climb.only.push(value("--only")?),
            other if other.starts_with("--") => {
                return Err(usage(format!(
                    "hillclimb does not take {other}; it takes --scope, --only, --rounds, --ledger and the shared run options"
                )))
            }
            other => focus.push(other.to_owned()),
        }
    }
    climb.focus = focus.join(" ");
    Ok(climb)
}

/// The repositories directly under `root`, by name: directories holding `.git`.
fn repositories(root: &Path) -> Result<Vec<String>, String> {
    let unreadable = |error: std::io::Error| {
        format!(
            "the organization directory {} cannot be read: {error}",
            root.display()
        )
    };
    let mut names = Vec::new();
    for entry in std::fs::read_dir(root).map_err(unreadable)? {
        let entry = entry.map_err(unreadable)?;
        if entry.path().join(".git").exists() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// Where the chooser looks, said for its prompt; refused when the scope has
/// nothing to improve. `only`, when it names repositories, keeps the
/// organization's climb to them, each refused by name when the directory
/// does not hold it.
fn place(scope: Scope, root: &Path, only: &[String]) -> Result<String, String> {
    match scope {
        Scope::Repository if !only.is_empty() => {
            Err(usage("hillclimb --only names repositories of an organization; give it with --scope organization"))
        }
        Scope::Repository if root.join(".git").exists() => Ok(format!("the repository {}", root.display())),
        Scope::Repository => Err(usage(format!(
            "{} is not a repository (no .git); give --cwd a repository, or --scope organization for a directory of them",
            root.display()
        ))),
        Scope::Organization => {
            let mut names = repositories(root)?;
            if let Some(missing) = only.iter().find(|name| !names.contains(name)) {
                return Err(usage(format!("{} holds no repository {missing}; --only names repositories directly under it", root.display())));
            }
            if !only.is_empty() {
                names.retain(|name| only.contains(name));
            }
            if names.is_empty() {
                return Err(usage(format!(
                    "{} holds no repository directly under it; --scope organization climbs the repositories of one directory",
                    root.display()
                )));
            }
            let which = if only.is_empty() { "whose repositories are" } else { "only these of its repositories" };
            Ok(format!("the organization at {}, {which}: {}", root.display(), names.join(", ")))
        }
    }
}

/// What the chooser answered.
#[derive(Debug, Deserialize)]
struct Proposal {
    objective: String,
    repository: Option<String>,
    kind: Option<String>,
    why: Option<String>,
}

/// The proposal in `answer`: the JSON object it holds, refused naming the answer when it holds none.
fn proposal(answer: &str) -> Result<Proposal, String> {
    let object = match (answer.find('{'), answer.rfind('}')) {
        (Some(start), Some(end)) if start < end => &answer[start..=end],
        _ => {
            return Err(format!(
                "the round's chooser answered no JSON object: {answer}"
            ))
        }
    };
    let proposal: Proposal = serde_json::from_str(object).map_err(|error| {
        format!("the round's chooser answered a proposal that cannot be read ({error}): {object}")
    })?;
    if proposal.objective.trim().is_empty() {
        return Err(format!(
            "the round's chooser proposed no objective: {object}"
        ));
    }
    Ok(proposal)
}

/// Ask a fresh read-only conversation for the next improvement.
fn choose(
    args: &Args,
    where_: &str,
    focus: &str,
    rounds: &[Round],
    hooks: &mut RunHooks<'static>,
) -> Result<Proposal, String> {
    let mut read_only = args.clone();
    read_only.command = crate::Command::Run;
    read_only.positionals.clear();
    read_only.allow_write = false;
    read_only.allow_command = false;
    read_only.yolo = false;
    read_only.goal = None;
    read_only.autonomous = true;
    let prompt = PROMPT
        .replace("{scope}", where_)
        .replace("{where}", where_)
        .replace("{focus}", focus)
        .replace("{ledger}", &ledger::digest(rounds));
    let mut conversation = Conversation::new_stage(&args.cwd)?;
    let answer = conversation.run_turn(&read_only, &prompt, &[], hooks)?;
    if hooks.cancelled() {
        return Err("hillclimb cancelled while choosing the next improvement".into());
    }
    proposal(&answer)
}

fn now() -> Result<u64, String> {
    crate::clock::unix_millis().map_err(|error| format!("the clock cannot be read: {error}"))
}

/// One round: choose, pursue, and say how it ended.
fn round(
    args: &Args,
    number: u64,
    where_: &str,
    climb: &Climb,
    rounds: &[Round],
    hooks: &mut RunHooks<'static>,
) -> Result<Round, String> {
    let started_at_ms = now()?;
    let chosen = choose(args, where_, &climb.focus, rounds, hooks)?;
    let selected = target::arguments(args, climb.scope, &climb.only, chosen.repository.as_deref())?;
    (hooks.progress)(&format!("hillclimb round {number}: {}", chosen.objective));
    let (ending, summary, error, receipt, run_directory) =
        match crate::autonomy::pursue_objective(&selected, chosen.objective.clone(), hooks) {
            Ok(outcome) => (
                Ending::Accepted,
                Some(outcome.verdict.summary),
                None,
                Some(outcome.receipt_path),
                Some(outcome.run_directory),
            ),
            Err(failure) => (
                Ending::Failed,
                None,
                Some(failure.message),
                failure.receipt_path,
                None,
            ),
        };
    Ok(Round {
        round: number,
        started_at_ms,
        ended_at_ms: now()?,
        objective: chosen.objective,
        repository: chosen.repository,
        kind: chosen.kind,
        why: chosen.why,
        ending,
        summary,
        error,
        receipt,
        run_directory,
    })
}

/// Whether the climb runs another round after `ran` rounds this invocation.
fn another(limit: Option<NonZeroU64>, ran: usize) -> bool {
    match limit {
        Some(limit) => (ran as u64) < limit.get(),
        None => true,
    }
}

pub(crate) fn command(args: &Args) -> Result<String, String> {
    if args.positionals.iter().any(|word| word == "--status") {
        return status::command(args);
    }
    let mut hooks = RunHooks::inert();
    hooks.progress = Box::new(|line| eprintln!("{line}"));
    let outcome = execution::run(args, options(&args.positionals)?, &mut hooks)?;
    if args.json {
        return serde_json::to_string_pretty(&outcome)
            .map(|text| text + "\n")
            .map_err(|error| error.to_string());
    }
    Ok(status::render(&outcome.report) + "\n")
}
