//! Read the selected ledger without taking or disturbing its writer ownership.
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{ledger, usage, Ending, Round};
use crate::Args;

#[derive(Serialize)]
pub(crate) struct Report {
    pub(crate) ledger: PathBuf,
    pub(crate) rounds: Vec<Round>,
    pub(crate) accepted: usize,
}

impl Report {
    pub(super) fn new(ledger: PathBuf, rounds: Vec<Round>) -> Self {
        let accepted = rounds.iter().filter(|round| round.ending == Ending::Accepted).count();
        Self { ledger, rounds, accepted }
    }
}

pub(crate) fn read(root: &Path, selected: Option<&Path>) -> Result<Report, String> {
    let path = ledger::path(root, selected)?;
    let rounds = ledger::read(&path)?;
    Ok(Report::new(path, rounds))
}

pub(super) fn render(report: &Report) -> String {
    if report.rounds.is_empty() {
        return format!(
            "No hill-climb rounds have been recorded in ledger {}. Start one with: jeden hillclimb --allow-write --allow-command [--scope organization] [focus]",
            report.ledger.display()
        );
    }
    format!("{}\n{} of {} rounds accepted; ledger {}",
        ledger::digest(&report.rounds), report.accepted, report.rounds.len(), report.ledger.display())
}

pub(crate) fn text(root: &Path, selected: Option<&Path>) -> Result<String, String> {
    read(root, selected).map(|report| render(&report))
}

pub(super) fn command(args: &Args) -> Result<String, String> {
    let mut selected = None;
    let mut words = args.positionals.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--status" => {}
            "--ledger" => {
                let value = words.next().ok_or_else(|| usage("hillclimb --status --ledger requires a file path"))?;
                selected = Some(Path::new(value));
            }
            other => return Err(usage(format!(
                "hillclimb --status does not take {other}; it takes --ledger, --json and --cwd, without write or command grants"
            ))),
        }
    }
    let report = read(&args.cwd, selected)?;
    if args.json {
        serde_json::to_string_pretty(&report).map(|text| text + "\n").map_err(|error| error.to_string())
    } else {
        Ok(render(&report) + "\n")
    }
}
