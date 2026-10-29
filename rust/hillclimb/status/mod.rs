//! Read the selected ledger without taking or disturbing its writer ownership.
mod repository;
pub(crate) use repository::RepositorySelection;

use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{ledger, usage, Ending, Round};
use crate::Args;

#[derive(Serialize)]
pub(crate) struct Report<'a> {
    pub(crate) ledger: PathBuf,
    pub(crate) rounds: Vec<Round>,
    pub(crate) accepted: usize,
    pub(crate) total: usize,
    pub(crate) ending: Option<Ending>,
    pub(crate) round: Option<u64>,
    pub(crate) repository: Option<&'a str>,
    #[serde(rename = "withoutRepository")]
    pub(crate) without_repository: bool,
}

impl Report<'static> {
    pub(super) fn new(ledger: PathBuf, rounds: Vec<Round>) -> Self {
        let accepted = rounds.iter().filter(|round| round.ending == Ending::Accepted).count();
        let total = rounds.len();
        Self { ledger, rounds, accepted, total, ending: None, round: None, repository: None, without_repository: false }
    }
}

pub(crate) fn read<'a>(
    root: &Path,
    selected: Option<&Path>,
    last: Option<NonZeroUsize>,
    ending: Option<Ending>,
    number: Option<u64>,
    repository: RepositorySelection<'a>,
) -> Result<Report<'a>, String> {
    #[derive(Default)]
    struct History {
        rounds: VecDeque<Round>,
        accepted: usize,
        total: usize,
    }
    let path = ledger::path(root, selected)?;
    let mut history = History::default();
    ledger::visit(&path, |round| {
        let observed = std::iter::once(&round);
        history.total += observed.clone().count();
        history.accepted += observed.filter(|round| round.ending == Ending::Accepted).count();
        if ending.is_some_and(|selected| round.ending != selected)
            || number.is_some_and(|selected| round.round != selected)
            || !repository.matches(round.repository.as_deref()) {
            return;
        }
        if last.is_some_and(|limit| history.rounds.len() == limit.get()) {
            let _ = history.rounds.pop_front();
        }
        history.rounds.push_back(round);
    })?;
    Ok(Report { ledger: path, rounds: history.rounds.into(), accepted: history.accepted, total: history.total,
        ending, round: number, repository: repository.name(), without_repository: repository.is_missing() })
}

struct ObservedSelection<'a, 's>(&'a Report<'s>);

impl std::fmt::Display for ObservedSelection<'_, '_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.round {
            Some(number) => write!(formatter, "recorded round {number}")?,
            None => formatter.write_str("all recorded round identities")?,
        }
        formatter.write_str(match self.0.ending {
            Some(Ending::Accepted) => ", accepted ending",
            Some(Ending::Failed) => ", failed ending",
            None => ", all endings",
        })?;
        match self.0.repository {
            Some(name) => write!(formatter, ", recorded repository {name:?}"),
            None if self.0.without_repository => formatter.write_str(", without a recorded repository identity"),
            None => formatter.write_str(", all recorded repository identities"),
        }
    }
}

pub(super) fn render(report: &Report<'_>) -> String {
    if report.rounds.is_empty() {
        if report.total > report.rounds.len() {
            return format!(
                "No rounds match this history selection ({}); {} recorded rounds; {} accepted overall; ledger {}",
                ObservedSelection(report), report.total, report.accepted, ledger::PathLabel(&report.ledger)
            );
        }
        return format!(
            "No hill-climb rounds have been recorded in ledger {} ({}). Start one with: jeden hillclimb --allow-write --allow-command [--scope organization] [focus]",
            ledger::PathLabel(&report.ledger), ObservedSelection(report)
        );
    }
    format!("{}\nShowing {} selected of {} recorded rounds; {} accepted overall; {}; ledger {}",
        ledger::digest(&report.rounds), report.rounds.len(), report.total, report.accepted, ObservedSelection(report), ledger::PathLabel(&report.ledger))
}

#[derive(Default)]
struct Selection<'a> {
    ledger: Option<&'a Path>,
    last: Option<NonZeroUsize>,
    ending: Option<Ending>,
    round: Option<u64>,
    repository: Option<&'a str>,
    without_repository: bool,
    json: bool,
}

fn selection<'a>(words: impl IntoIterator<Item = &'a str>) -> Result<Selection<'a>, String> {
    let mut selected = Selection::default();
    let mut words = words.into_iter();
    while let Some(word) = words.next() {
        match word {
            "--status" => {}
            "--json" => selected.json = true,
            "--ledger" => {
                let value = words.next().ok_or_else(|| usage("hillclimb --status --ledger requires a file path"))?;
                selected.ledger = Some(Path::new(value));
            }
            "--last" => {
                let value = words.next().ok_or_else(|| usage("hillclimb --status --last requires a positive record count"))?;
                selected.last = Some(value.parse::<NonZeroUsize>().map_err(|error| usage(format!(
                    "hillclimb --status --last requires a positive whole record count; got {value:?}: {error}"
                )))?);
            }
            "--ending" => {
                let value = words.next().ok_or_else(|| usage("hillclimb --status --ending requires a recorded ending"))?;
                let decoder = serde::de::value::StrDeserializer::<serde::de::value::Error>::new(value);
                selected.ending = Some(Ending::deserialize(decoder).map_err(|error| usage(format!(
                    "hillclimb --status --ending cannot select {value:?}: {error}"
                )))?);
            }
            "--round" => {
                let value = words.next().ok_or_else(|| usage("hillclimb --status --round requires a recorded round identifier"))?;
                selected.round = Some(value.parse::<u64>().map_err(|error| usage(format!(
                    "hillclimb --status --round requires an unsigned whole round identifier within u64; got {value:?}: {error}"
                )))?);
            }
            "--repository" => {
                let value = words.next().filter(|value| !value.starts_with('-')).ok_or_else(|| usage(
                    "hillclimb --status --repository requires a recorded identity; option-like identities use --repository=VALUE"
                ))?;
                selected.repository = Some(value);
            }
            "--without-repository" => selected.without_repository = true,
            other => match other.strip_prefix("--repository=") {
                Some(value) => selected.repository = Some(value),
                None => return Err(usage(format!(
                    "hillclimb history does not take {other}; use --ledger, --last, --ending, --round, --repository, --without-repository or --json (the CLI also accepts --cwd); no write or command grants are required"
                ))),
            },
        }
    }
    Ok(selected)
}

fn output(root: &Path, selected: Selection<'_>, json: bool) -> Result<String, String> {
    let repository = RepositorySelection::new(selected.repository, selected.without_repository).map_err(usage)?;
    let report = read(root, selected.ledger, selected.last, selected.ending, selected.round, repository)?;
    if json || selected.json {
        serde_json::to_string_pretty(&report).map(|text| text + "\n").map_err(|error|
            format!("the hill-climb history for ledger {:?} cannot be encoded as JSON: {error}", report.ledger))
    } else {
        Ok(render(&report) + "\n")
    }
}

pub(crate) fn text(root: &Path, args: &str) -> Result<String, String> {
    let words = crate::slash::common::split_args(args);
    output(root, selection(words.iter().map(String::as_str))?, false)
}

pub(super) fn command(args: &Args) -> Result<String, String> {
    output(&args.cwd, selection(args.positionals.iter().map(String::as_str))?, args.json)
}
