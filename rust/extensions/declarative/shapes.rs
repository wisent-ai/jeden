//! What a declarative extension is once it has been read.
//!
//! Split out of `extensions/declarative.rs`, which had grown past the module
//! line cap.

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub(super) struct Input {
    pub kind: &'static str,
    pub path: PathBuf,
    pub precedence: usize,
}

#[derive(Clone, Debug)]
pub(super) struct LoadedCapability {
    pub kind: &'static str,
    pub active: bool,
    pub id: String,
    pub path: PathBuf,
    pub healthy: bool,
    pub error: Option<String>,
    pub description: String,
    pub metadata: Value,
}

#[derive(Clone, Debug)]
pub(super) struct Skill {
    pub id: String,
    pub description: String,
    pub prompt: String,
    pub source: PathBuf,
    pub precedence: usize,
    pub always_apply: bool,
    pub matchers: Vec<String>,
    pub assets: Vec<PathBuf>,
    pub metadata: Value,
}

#[derive(Clone, Debug)]
pub(super) struct Rule {
    pub id: String,
    pub description: String,
    pub content: String,
    pub source: PathBuf,
    pub precedence: usize,
    pub always_apply: bool,
    pub matchers: Vec<String>,
}

#[derive(Clone, Debug)]
pub(super) struct Agent {
    pub id: String,
    pub source: PathBuf,
    pub precedence: usize,
    pub value: Value,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Loaded {
    pub capabilities: Vec<LoadedCapability>,
    pub skills: BTreeMap<String, Skill>,
    pub rules: BTreeMap<String, Rule>,
    pub agents: BTreeMap<String, Agent>,
}

#[derive(Clone, Debug)]
pub(crate) struct PromptContribution {
    pub id: String,
    pub kind: &'static str,
    pub content: String,
    pub source: PathBuf,
    pub precedence: usize,
    pub assets: Vec<PathBuf>,
}
