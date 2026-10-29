//! Reading the options a roadmap command was given, and refusing what it
//! cannot act on.

use super::super::model::RoadmapError;
use super::super::store::RoadmapStore;
use serde::Serialize;
use std::collections::BTreeMap;

pub(super) struct ParsedOptions {
    values: BTreeMap<String, Vec<String>>,
    pub(super) positionals: Vec<String>,
}

impl ParsedOptions {
    /// `--name value` and `--name=value` pairs, each name keeping every
    /// value given, and every other word as a positional.
    pub(super) fn parse(args: &[String]) -> Result<Self, RoadmapError> {
        let mut values: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut positionals = Vec::new();
        let mut tokens = args.iter();
        while let Some(token) = tokens.next() {
            let Some(flag) = token.strip_prefix("--") else {
                positionals.push(token.clone());
                continue;
            };
            let (name, value) = match flag.split_once('=') {
                Some((name, value)) => (name.to_string(), value.to_string()),
                None => {
                    let value = tokens
                        .next()
                        .cloned()
                        .ok_or_else(|| RoadmapError::Usage(format!("--{flag} requires a value")))?;
                    (flag.to_string(), value)
                }
            };
            values.entry(name).or_default().push(value);
        }
        Ok(Self {
            values,
            positionals,
        })
    }

    pub(super) fn one(&self, name: &str) -> Option<&str> {
        self.values
            .get(name)
            .and_then(|values| values.last())
            .map(String::as_str)
    }

    /// Every value given for `name`; an option never given has none.
    pub(super) fn many(&self, name: &str) -> Vec<String> {
        match self.values.get(name) {
            Some(values) => values.clone(),
            None => Vec::new(),
        }
    }
}

pub(super) fn expected_revision(
    store: &RoadmapStore,
    options: &ParsedOptions,
) -> Result<u64, RoadmapError> {
    match options.one("revision") {
        Some(value) => value.parse::<u64>().map_err(|error| {
            RoadmapError::Usage(format!("invalid --revision value {value}: {error}"))
        }),
        None => Ok(store.load()?.revision),
    }
}

pub(super) fn format_json<T: Serialize>(value: &T) -> Result<String, RoadmapError> {
    serde_json::to_string_pretty(value)
        .map(|text| text + "\n")
        .map_err(|error| RoadmapError::Invalid(error.to_string()))
}
