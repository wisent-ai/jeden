//! Reading the options a roadmap command was given, and refusing what it
//! cannot act on.
//!
//! Split out of `roadmap/mod.rs`, which had grown past the module line cap.

use super::super::model::RoadmapError;
use super::super::store::RoadmapStore;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct ParsedOptions {
    values: BTreeMap<String, Vec<String>>,
    pub(super) positionals: Vec<String>,
}

impl ParsedOptions {
    pub(super) fn parse(args: &[String]) -> Result<Self, RoadmapError> {
        let mut parsed = Self::default();
        let mut index = 0;
        while index < args.len() {
            let token = &args[index];
            if let Some(name) = token.strip_prefix("--") {
                let (name, inline) = name
                    .split_once('=')
                    .map(|(name, value)| (name.to_string(), Some(value.to_string())))
                    .unwrap_or_else(|| (name.to_string(), None));
                let value = match inline {
                    Some(value) => value,
                    None => {
                        index += 1;
                        args.get(index).cloned().ok_or_else(|| {
                            RoadmapError::Usage(format!("--{name} requires a value"))
                        })?
                    }
                };
                parsed.values.entry(name).or_default().push(value);
            } else {
                parsed.positionals.push(token.clone());
            }
            index += 1;
        }
        Ok(parsed)
    }

    pub(super) fn one(&self, name: &str) -> Option<&str> {
        self.values
            .get(name)
            .and_then(|values| values.last())
            .map(String::as_str)
    }

    pub(super) fn many(&self, name: &str) -> Vec<String> {
        self.values.get(name).cloned().unwrap_or_default()
    }
}

pub(super) fn expected_revision(
    store: &RoadmapStore,
    options: &ParsedOptions,
) -> Result<u64, RoadmapError> {
    match options.one("revision") {
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| RoadmapError::Usage(format!("invalid --revision value: {value}"))),
        None => Ok(store.load()?.revision),
    }
}

pub(super) fn format_json<T: Serialize>(value: &T) -> Result<String, RoadmapError> {
    serde_json::to_string_pretty(value)
        .map(|text| text + "\n")
        .map_err(|error| RoadmapError::Invalid(error.to_string()))
}
