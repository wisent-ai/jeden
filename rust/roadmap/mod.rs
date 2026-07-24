use crate::capability::{CapabilityDescriptor, CapabilityHealth, CapabilityKind, FunctionTarget};
use crate::tui::{PickerItem, PickerSpec};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const ROADMAP_SCHEMA_VERSION: u32 = 1;
const LOCK_RETRIES: usize = 500;
const LOCK_WAIT: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoadmapStatus {
    Backlog,
    Planned,
    InProgress,
    Implemented,
    NotRun,
    Failed,
    ExternalBlocked,
    Passed,
    Dropped,
}

impl RoadmapStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Planned => "planned",
            Self::InProgress => "in_progress",
            Self::Implemented => "implemented",
            Self::NotRun => "not_run",
            Self::Failed => "failed",
            Self::ExternalBlocked => "external_blocked",
            Self::Passed => "passed",
            Self::Dropped => "dropped",
        }
    }
}

impl fmt::Display for RoadmapStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RoadmapStatus {
    type Err = RoadmapError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "backlog" => Ok(Self::Backlog),
            "planned" => Ok(Self::Planned),
            "in_progress" => Ok(Self::InProgress),
            "implemented" => Ok(Self::Implemented),
            "not_run" => Ok(Self::NotRun),
            "failed" => Ok(Self::Failed),
            "external_blocked" => Ok(Self::ExternalBlocked),
            "passed" => Ok(Self::Passed),
            "dropped" => Ok(Self::Dropped),
            other => Err(RoadmapError::Invalid(format!(
                "unknown roadmap status '{other}'; expected backlog|planned|in_progress|implemented|not_run|failed|external_blocked|passed|dropped"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceCriterion {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceLink {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_id: Option<String>,
    pub added_at: String,
    pub added_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoadmapItem {
    pub id: String,
    pub title: String,
    pub area: String,
    pub priority: String,
    pub status: RoadmapStatus,
    pub summary: String,
    #[serde(default)]
    pub implementation: String,
    #[serde(default)]
    pub rationale: String,
    #[serde(default)]
    pub implementation_order: String,
    #[serde(default)]
    pub acceptance: Vec<AcceptanceCriterion>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub external_prerequisites: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<EvidenceLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub created_at: String,
    pub created_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoadmapFile {
    pub schema_version: u32,
    pub revision: u64,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub items: Vec<RoadmapItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoadmapGraph {
    pub revision: u64,
    pub nodes: Vec<RoadmapGraphNode>,
    pub edges: Vec<RoadmapGraphEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoadmapGraphNode {
    pub id: String,
    pub title: String,
    pub status: RoadmapStatus,
    pub priority: String,
    pub area: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoadmapGraphEdge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckReport {
    pub ok: bool,
    pub schema_version: u32,
    pub revision: u64,
    pub item_count: usize,
    pub errors: Vec<String>,
}

#[derive(Debug)]
pub enum RoadmapError {
    Io(String),
    Invalid(String),
    RevisionConflict { expected: u64, actual: u64 },
    NotFound(String),
    Usage(String),
}

impl fmt::Display for RoadmapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) | Self::Invalid(message) | Self::Usage(message) => {
                f.write_str(message)
            }
            Self::RevisionConflict { expected, actual } => write!(
                f,
                "roadmap revision conflict: expected {expected}, found {actual}"
            ),
            Self::NotFound(id) => write!(f, "roadmap item not found: {id}"),
        }
    }
}

impl From<std::io::Error> for RoadmapError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<String> for RoadmapError {
    fn from(error: String) -> Self {
        Self::Io(error)
    }
}

pub struct RoadmapStore {
    cwd: PathBuf,
    path: PathBuf,
    lock_path: PathBuf,
}

struct StableLock {
    path: PathBuf,
    _file: File,
}

impl Drop for StableLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl StableLock {
    fn acquire(path: &Path) -> Result<Self, RoadmapError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        for _ in 0..LOCK_RETRIES {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id())?;
                    file.sync_all()?;
                    return Ok(Self {
                        path: path.to_path_buf(),
                        _file: file,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    thread::sleep(LOCK_WAIT);
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(RoadmapError::Io(format!(
            "timed out waiting for roadmap lock {}",
            path.display()
        )))
    }
}

impl RoadmapStore {
    pub fn new(cwd: &Path) -> Self {
        let path = cwd.join("roadmap/roadmap.yaml");
        Self {
            cwd: cwd.to_path_buf(),
            lock_path: cwd.join("roadmap/.roadmap.lock"),
            path,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<RoadmapFile, RoadmapError> {
        let bytes = fs::read(&self.path).map_err(|error| {
            RoadmapError::Io(format!("cannot read {}: {error}", self.path.display()))
        })?;
        let mut roadmap: RoadmapFile = serde_yaml::from_slice(&bytes).map_err(|error| {
            RoadmapError::Invalid(format!("invalid {}: {error}", self.path.display()))
        })?;
        normalize(&mut roadmap);
        Ok(roadmap)
    }

    pub fn check(&self) -> CheckReport {
        match self.load() {
            Ok(roadmap) => {
                let errors = self.validation_errors(&roadmap, true);
                CheckReport {
                    ok: errors.is_empty(),
                    schema_version: roadmap.schema_version,
                    revision: roadmap.revision,
                    item_count: roadmap.items.len(),
                    errors,
                }
            }
            Err(error) => CheckReport {
                ok: false,
                schema_version: 0,
                revision: 0,
                item_count: 0,
                errors: vec![error.to_string()],
            },
        }
    }

    pub fn mutate<F>(
        &self,
        expected_revision: u64,
        event_type: &str,
        event_data: Value,
        change: F,
    ) -> Result<RoadmapFile, RoadmapError>
    where
        F: FnOnce(&mut RoadmapFile) -> Result<(), RoadmapError>,
    {
        let _guard = StableLock::acquire(&self.lock_path)?;
        let mut roadmap = self.load()?;
        if roadmap.revision != expected_revision {
            return Err(RoadmapError::RevisionConflict {
                expected: expected_revision,
                actual: roadmap.revision,
            });
        }
        change(&mut roadmap)?;
        roadmap.revision = roadmap.revision.saturating_add(1);
        normalize(&mut roadmap);
        let errors = self.validation_errors(&roadmap, true);
        if !errors.is_empty() {
            return Err(RoadmapError::Invalid(errors.join("\n")));
        }
        atomic_write_yaml(&self.path, &roadmap)?;
        let mut payload = event_data;
        if let Some(map) = payload.as_object_mut() {
            map.insert("revision".into(), json!(roadmap.revision));
            map.insert(
                "roadmapPath".into(),
                json!(relative_path(&self.cwd, &self.path)),
            );
        }
        if let Err(error) = crate::agent::record_roadmap_event(&self.cwd, event_type, payload) {
            eprintln!(
                "Warning: roadmap committed at revision {} but session provenance failed: {error}",
                roadmap.revision
            );
        }
        Ok(roadmap)
    }

    pub fn graph(&self) -> Result<RoadmapGraph, RoadmapError> {
        let roadmap = self.load()?;
        let nodes = roadmap
            .items
            .iter()
            .map(|item| RoadmapGraphNode {
                id: item.id.clone(),
                title: item.title.clone(),
                status: item.status.clone(),
                priority: item.priority.clone(),
                area: item.area.clone(),
            })
            .collect();
        let mut edges = Vec::new();
        for item in &roadmap.items {
            for dependency in &item.depends_on {
                edges.push(RoadmapGraphEdge {
                    from: item.id.clone(),
                    to: dependency.clone(),
                });
            }
        }
        edges.sort_by(|left, right| (&left.from, &left.to).cmp(&(&right.from, &right.to)));
        Ok(RoadmapGraph {
            revision: roadmap.revision,
            nodes,
            edges,
        })
    }

    pub fn render(&self) -> Result<String, RoadmapError> {
        let roadmap = self.load()?;
        let errors = self.validation_errors(&roadmap, true);
        if !errors.is_empty() {
            return Err(RoadmapError::Invalid(errors.join("\n")));
        }
        Ok(render_markdown(&roadmap))
    }

    fn validation_errors(&self, roadmap: &RoadmapFile, validate_capabilities: bool) -> Vec<String> {
        let mut errors = Vec::new();
        if roadmap.schema_version != ROADMAP_SCHEMA_VERSION {
            errors.push(format!(
                "unsupported schemaVersion {}; expected {}",
                roadmap.schema_version, ROADMAP_SCHEMA_VERSION
            ));
        }
        let mut ids = BTreeSet::new();
        for item in &roadmap.items {
            if item.id.trim().is_empty() {
                errors.push("roadmap item has an empty id".into());
            } else if !ids.insert(item.id.clone()) {
                errors.push(format!("duplicate roadmap item id: {}", item.id));
            }
            if item.title.trim().is_empty() {
                errors.push(format!("{} has an empty title", item.id));
            }
            if item.summary.trim().is_empty() {
                errors.push(format!("{} has an empty summary", item.id));
            }
            if !matches!(item.priority.as_str(), "P0" | "P1" | "P2" | "P3") {
                errors.push(format!(
                    "{} has invalid priority {}",
                    item.id, item.priority
                ));
            }
            if item.acceptance.is_empty() {
                errors.push(format!("{} has no acceptance criteria", item.id));
            }
            let mut acceptance_ids = BTreeSet::new();
            for criterion in &item.acceptance {
                if criterion.id.trim().is_empty() || criterion.text.trim().is_empty() {
                    errors.push(format!("{} has an empty acceptance criterion", item.id));
                }
                if !acceptance_ids.insert(criterion.id.clone()) {
                    errors.push(format!(
                        "{} has duplicate acceptance id {}",
                        item.id, criterion.id
                    ));
                }
            }
            for evidence in &item.evidence {
                if evidence.uri.trim().is_empty() {
                    errors.push(format!("{} has an empty evidence URI", item.id));
                }
                if let Some(criterion) = &evidence.acceptance_id {
                    if !acceptance_ids.contains(criterion) {
                        errors.push(format!(
                            "{} evidence references missing acceptance {}",
                            item.id, criterion
                        ));
                    }
                }
            }
            if item.status == RoadmapStatus::Passed && item.evidence.is_empty() {
                errors.push(format!("{} cannot be passed without evidence", item.id));
            }
            if item.status == RoadmapStatus::ExternalBlocked
                && item.external_prerequisites.is_empty()
            {
                errors.push(format!(
                    "{} cannot be external_blocked without externalPrerequisites",
                    item.id
                ));
            }
        }
        for item in &roadmap.items {
            for dependency in &item.depends_on {
                if dependency == &item.id {
                    errors.push(format!("{} cannot depend on itself", item.id));
                } else if !ids.contains(dependency) {
                    errors.push(format!(
                        "{} depends on missing roadmap item {}",
                        item.id, dependency
                    ));
                }
            }
        }
        errors.extend(cycle_errors(roadmap));
        if validate_capabilities {
            let snapshot = crate::capability::for_cwd(&self.cwd);
            let known = snapshot
                .descriptors
                .iter()
                .map(|descriptor| descriptor.id.clone())
                .collect::<BTreeSet<_>>();
            for item in &roadmap.items {
                for capability in &item.capabilities {
                    if !known.contains(capability.as_str()) {
                        errors.push(format!(
                            "{} references nonexistent capability {}",
                            item.id, capability
                        ));
                    }
                }
            }
        }
        errors.sort();
        errors.dedup();
        errors
    }
}

fn normalize(roadmap: &mut RoadmapFile) {
    for item in &mut roadmap.items {
        item.id = item.id.trim().to_ascii_uppercase();
        item.title = item.title.trim().to_string();
        item.area = item.area.trim().to_ascii_lowercase();
        item.priority = item.priority.trim().to_ascii_uppercase();
        item.summary = item.summary.trim().to_string();
        item.depends_on.sort();
        item.depends_on.dedup();
        item.capabilities.sort();
        item.capabilities.dedup();
        item.external_prerequisites.sort();
        item.external_prerequisites.dedup();
        item.acceptance
            .sort_by(|left, right| left.id.cmp(&right.id));
        item.evidence.sort_by(|left, right| {
            (&left.acceptance_id, &left.uri).cmp(&(&right.acceptance_id, &right.uri))
        });
    }
    roadmap.items.sort_by(|left, right| left.id.cmp(&right.id));
}

fn cycle_errors(roadmap: &RoadmapFile) -> Vec<String> {
    fn visit(
        id: &str,
        dependencies: &BTreeMap<&str, Vec<&str>>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
        errors: &mut Vec<String>,
    ) {
        if visited.contains(id) {
            return;
        }
        if !visiting.insert(id.to_string()) {
            errors.push(format!("dependency cycle includes {id}"));
            return;
        }
        if let Some(next) = dependencies.get(id) {
            for dependency in next {
                visit(dependency, dependencies, visiting, visited, errors);
            }
        }
        visiting.remove(id);
        visited.insert(id.to_string());
    }

    let dependencies = roadmap
        .items
        .iter()
        .map(|item| {
            (
                item.id.as_str(),
                item.depends_on
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut errors = Vec::new();
    for item in &roadmap.items {
        visit(
            &item.id,
            &dependencies,
            &mut visiting,
            &mut visited,
            &mut errors,
        );
    }
    errors
}

fn atomic_write_yaml(path: &Path, roadmap: &RoadmapFile) -> Result<(), RoadmapError> {
    let parent = path
        .parent()
        .ok_or_else(|| RoadmapError::Io("roadmap path has no parent".into()))?;
    fs::create_dir_all(parent)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = parent.join(format!(".roadmap.yaml.tmp-{}-{nonce}", std::process::id()));
    let yaml =
        serde_yaml::to_string(roadmap).map_err(|error| RoadmapError::Invalid(error.to_string()))?;
    let result = (|| -> Result<(), RoadmapError> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(yaml.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn relative_path(cwd: &Path, path: &Path) -> String {
    path.strip_prefix(cwd)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn next_item_id(roadmap: &RoadmapFile) -> String {
    let next = roadmap
        .items
        .iter()
        .filter_map(|item| item.id.strip_prefix("JED-")?.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    format!("JED-{next:03}")
}

fn actor() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".into())
}

fn now() -> String {
    crate::agent::now_stamp()
}

fn find_item<'a>(roadmap: &'a RoadmapFile, id: &str) -> Result<&'a RoadmapItem, RoadmapError> {
    roadmap
        .items
        .iter()
        .find(|item| item.id.eq_ignore_ascii_case(id))
        .ok_or_else(|| RoadmapError::NotFound(id.to_string()))
}

fn find_item_mut<'a>(
    roadmap: &'a mut RoadmapFile,
    id: &str,
) -> Result<&'a mut RoadmapItem, RoadmapError> {
    roadmap
        .items
        .iter_mut()
        .find(|item| item.id.eq_ignore_ascii_case(id))
        .ok_or_else(|| RoadmapError::NotFound(id.to_string()))
}

fn render_markdown(roadmap: &RoadmapFile) -> String {
    let mut out = String::new();
    out.push_str("# Jeden Production Roadmap\n\n");
    out.push_str("<!-- Generated by `jeden roadmap render` from `roadmap/roadmap.yaml`. Do not edit by hand. -->\n\n");
    out.push_str(&format!(
        "Schema version: `{}` · Revision: `{}` · Items: `{}`\n\n",
        roadmap.schema_version,
        roadmap.revision,
        roadmap.items.len()
    ));
    out.push_str("## Status legend\n\n");
    out.push_str("`backlog` · `planned` · `in_progress` · `implemented` · `not_run` · `failed` · `external_blocked` · `passed` · `dropped`\n\n");
    if !roadmap.context.trim().is_empty() {
        out.push_str("## Migrated production-program context\n\n");
        out.push_str("The preserved context below describes the original 23-scope certification program. The canonical item list that follows is versioned independently and may extend that original program.\n\n");
        out.push_str(roadmap.context.trim());
        out.push_str("\n\n");
    }
    for item in &roadmap.items {
        out.push_str(&format!("## {}. {}\n\n", item.id, item.title));
        out.push_str(&format!(
            "- **Area:** `{}`\n- **Priority:** `{}`\n- **Status:** `{}`\n",
            item.area, item.priority, item.status
        ));
        out.push_str(&format!("- **Summary:** {}\n", item.summary));
        if !item.depends_on.is_empty() {
            out.push_str(&format!(
                "- **Depends on:** {}\n",
                item.depends_on.join(", ")
            ));
        }
        if !item.capabilities.is_empty() {
            out.push_str(&format!(
                "- **Capabilities:** {}\n",
                item.capabilities.join(", ")
            ));
        }
        if !item.external_prerequisites.is_empty() {
            out.push_str("- **External prerequisites:**\n");
            for prerequisite in &item.external_prerequisites {
                out.push_str(&format!("  - {}\n", prerequisite));
            }
        }
        if let Some(reason) = &item.reason {
            out.push_str(&format!("- **Status reason:** {}\n", reason));
        }
        if !item.implementation.is_empty() {
            out.push_str(&format!(
                "\n### Files and modules\n\n{}\n",
                item.implementation
            ));
        }
        if !item.rationale.is_empty() {
            out.push_str(&format!("\n### Rationale\n\n{}\n", item.rationale));
        }
        if !item.implementation_order.is_empty() {
            out.push_str(&format!(
                "\n### Implementation order\n\n{}\n",
                item.implementation_order
            ));
        }
        out.push_str("\n### Acceptance criteria\n\n");
        for criterion in &item.acceptance {
            let has_evidence = item
                .evidence
                .iter()
                .any(|evidence| evidence.acceptance_id.as_deref() == Some(&criterion.id));
            out.push_str(&format!(
                "- [{}] **{}** — {}\n",
                if has_evidence { "x" } else { " " },
                criterion.id,
                criterion.text
            ));
        }
        if !item.evidence.is_empty() {
            out.push_str("\n### Evidence\n\n");
            for evidence in &item.evidence {
                let criterion = evidence
                    .acceptance_id
                    .as_deref()
                    .map(|id| format!(" ({id})"))
                    .unwrap_or_default();
                out.push_str(&format!("- `{}`{}\n", evidence.uri, criterion));
            }
        }
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

#[derive(Default)]
struct ParsedOptions {
    values: BTreeMap<String, Vec<String>>,
    positionals: Vec<String>,
}

impl ParsedOptions {
    fn parse(args: &[String]) -> Result<Self, RoadmapError> {
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

    fn one(&self, name: &str) -> Option<&str> {
        self.values
            .get(name)
            .and_then(|values| values.last())
            .map(String::as_str)
    }

    fn many(&self, name: &str) -> Vec<String> {
        self.values.get(name).cloned().unwrap_or_default()
    }
}

fn expected_revision(store: &RoadmapStore, options: &ParsedOptions) -> Result<u64, RoadmapError> {
    match options.one("revision") {
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| RoadmapError::Usage(format!("invalid --revision value: {value}"))),
        None => Ok(store.load()?.revision),
    }
}

fn format_json<T: Serialize>(value: &T) -> Result<String, RoadmapError> {
    serde_json::to_string_pretty(value)
        .map(|text| text + "\n")
        .map_err(|error| RoadmapError::Invalid(error.to_string()))
}

pub fn execute(cwd: &Path, args: &[String], json_output: bool) -> Result<String, RoadmapError> {
    let store = RoadmapStore::new(cwd);
    let command = args.first().map(String::as_str).unwrap_or("list");
    let options = ParsedOptions::parse(args.get(1..).unwrap_or_default())?;
    match command {
        "list" => {
            let roadmap = store.load()?;
            let status = options
                .one("status")
                .map(RoadmapStatus::from_str)
                .transpose()?;
            let area = options.one("area");
            let priority = options.one("priority");
            let items = roadmap
                .items
                .iter()
                .filter(|item| status.as_ref().map(|v| v == &item.status).unwrap_or(true))
                .filter(|item| area.map(|v| v == item.area).unwrap_or(true))
                .filter(|item| priority.map(|v| v == item.priority).unwrap_or(true))
                .cloned()
                .collect::<Vec<_>>();
            if json_output {
                format_json(&json!({
                    "schemaVersion": roadmap.schema_version,
                    "revision": roadmap.revision,
                    "items": items,
                }))
            } else if items.is_empty() {
                Ok(format!("No roadmap items matched. (revision {})\n", roadmap.revision))
            } else {
                let mut output = format!("Roadmap revision {}\n", roadmap.revision);
                for item in items {
                    output.push_str(&format!(
                        "{}\t{}\t{}\t{}\t{}\n",
                        item.id, item.status, item.priority, item.area, item.title
                    ));
                }
                Ok(output)
            }
        }
        "show" => {
            let id = options
                .positionals
                .first()
                .ok_or_else(|| RoadmapError::Usage("Usage: roadmap show <id>".into()))?;
            let roadmap = store.load()?;
            let item = find_item(&roadmap, id)?;
            if json_output {
                format_json(item)
            } else {
                Ok(render_markdown(&RoadmapFile {
                    schema_version: roadmap.schema_version,
                    revision: roadmap.revision,
                    context: String::new(),
                    items: vec![item.clone()],
                }))
            }
        }
        "graph" => {
            let graph = store.graph()?;
            if json_output {
                format_json(&graph)
            } else {
                let mut output = format!("Roadmap dependency graph (revision {})\n", graph.revision);
                for node in &graph.nodes {
                    output.push_str(&format!("{} [{}] {}\n", node.id, node.status, node.title));
                }
                for edge in &graph.edges {
                    output.push_str(&format!("{} -> {}\n", edge.from, edge.to));
                }
                Ok(output)
            }
        }
        "add" => {
            let positional_title = options.positionals.join(" ");
            if options.one("title").is_some() && !positional_title.trim().is_empty() {
                return Err(RoadmapError::Usage(
                    "roadmap add accepts the title either positionally or through --title, not both"
                        .into(),
                ));
            }
            let title = options
                .one("title")
                .map(str::to_string)
                .unwrap_or(positional_title);
            if title.trim().is_empty() {
                return Err(RoadmapError::Usage(
                    "Usage: roadmap add <title> | --title <title> --area <area> --priority <P0|P1|P2|P3> --summary <text> --acceptance <text> [--revision <n>]".into(),
                ));
            }
            let revision = expected_revision(&store, &options)?;
            let area = options.one("area").unwrap_or("general").to_string();
            let priority = options.one("priority").unwrap_or("P2").to_string();
            let summary = options.one("summary").unwrap_or(&title).to_string();
            let acceptance = options
                .many("acceptance")
                .into_iter()
                .enumerate()
                .map(|(index, text)| AcceptanceCriterion {
                    id: format!("acceptance-{}", index + 1),
                    text,
                })
                .collect::<Vec<_>>();
            let dependencies = options
                .many("depends-on")
                .into_iter()
                .flat_map(|value| value.split(',').map(str::to_string).collect::<Vec<_>>())
                .collect::<Vec<_>>();
            let capabilities = options.many("capability");
            let prerequisites = options.many("external-prerequisite");
            let status = options
                .one("status")
                .map(RoadmapStatus::from_str)
                .transpose()?
                .unwrap_or(RoadmapStatus::Backlog);
            let created_at = now();
            let created_by = actor();
            let mut created_id = String::new();
            let roadmap = store.mutate(
                revision,
                "roadmap_item_created",
                json!({"title": title}),
                |roadmap| {
                    let id = options
                        .one("id")
                        .map(str::to_ascii_uppercase)
                        .unwrap_or_else(|| next_item_id(roadmap));
                    created_id = id.clone();
                    roadmap.items.push(RoadmapItem {
                        id,
                        title: title.clone(),
                        area,
                        priority,
                        status,
                        summary,
                        implementation: options
                            .one("implementation")
                            .unwrap_or_default()
                            .to_string(),
                        rationale: options
                            .one("rationale")
                            .unwrap_or_default()
                            .to_string(),
                        implementation_order: options
                            .one("implementation-order")
                            .unwrap_or_default()
                            .to_string(),
                        acceptance,
                        depends_on: dependencies,
                        capabilities,
                        external_prerequisites: prerequisites,
                        evidence: Vec::new(),
                        reason: None,
                        created_at,
                        created_by,
                    });
                    Ok(())
                },
            )?;
            if json_output {
                format_json(find_item(&roadmap, &created_id)?)
            } else {
                Ok(format!(
                    "Created {} at roadmap revision {}.\n",
                    created_id, roadmap.revision
                ))
            }
        }
        "drop" => mutate_status(
            &store,
            &options,
            RoadmapStatus::Dropped,
            true,
            json_output,
        ),
        "start" => mutate_status(
            &store,
            &options,
            RoadmapStatus::InProgress,
            false,
            json_output,
        ),
        "implemented" => mutate_status(
            &store,
            &options,
            RoadmapStatus::Implemented,
            false,
            json_output,
        ),
        "block" => mutate_status(
            &store,
            &options,
            RoadmapStatus::ExternalBlocked,
            false,
            json_output,
        ),
        "pass" => mutate_status(
            &store,
            &options,
            RoadmapStatus::Passed,
            false,
            json_output,
        ),
        "status" => {
            let id = options
                .positionals
                .first()
                .ok_or_else(|| RoadmapError::Usage("Usage: roadmap status <id> <status>".into()))?;
            let status = options
                .positionals
                .get(1)
                .ok_or_else(|| RoadmapError::Usage("Usage: roadmap status <id> <status>".into()))?
                .parse()?;
            mutate_status_explicit(&store, &options, id, status, false, 2, json_output)
        }
        "depends" | "undepends" => {
            let id = options.positionals.first().ok_or_else(|| {
                RoadmapError::Usage(format!("Usage: roadmap {command} <id> <dependency-id>"))
            })?;
            let dependency = options.positionals.get(1).ok_or_else(|| {
                RoadmapError::Usage(format!("Usage: roadmap {command} <id> <dependency-id>"))
            })?;
            let revision = expected_revision(&store, &options)?;
            let add = command == "depends";
            let event = if add {
                "roadmap_item_updated"
            } else {
                "roadmap_item_updated"
            };
            let roadmap = store.mutate(
                revision,
                event,
                json!({"itemId": id, "dependencyId": dependency, "operation": command}),
                |roadmap| {
                    if find_item(roadmap, dependency).is_err() {
                        return Err(RoadmapError::NotFound(dependency.clone()));
                    }
                    let item = find_item_mut(roadmap, id)?;
                    if add {
                        item.depends_on.push(dependency.to_ascii_uppercase());
                    } else {
                        let before = item.depends_on.len();
                        item.depends_on
                            .retain(|value| !value.eq_ignore_ascii_case(dependency));
                        if before == item.depends_on.len() {
                            return Err(RoadmapError::Invalid(format!(
                                "{} does not depend on {}",
                                item.id, dependency
                            )));
                        }
                    }
                    Ok(())
                },
            )?;
            if json_output {
                format_json(find_item(&roadmap, id)?)
            } else {
                Ok(format!(
                    "Updated {} at roadmap revision {}.\n",
                    id, roadmap.revision
                ))
            }
        }
        "acceptance" => acceptance_command(&store, &options, json_output),
        "check" => {
            let report = store.check();
            if json_output {
                format_json(&report)
            } else if report.ok {
                Ok(format!(
                    "Roadmap valid: schema {}, revision {}, {} items.\n",
                    report.schema_version, report.revision, report.item_count
                ))
            } else {
                Err(RoadmapError::Invalid(report.errors.join("\n")))
            }
        }
        "render" => {
            let markdown = store.render()?;
            let target = cwd.join("docs/JEDEN_NEXT_PHASES_PLAN.md");
            let mirror = cwd.join("roadmap/views/JEDEN_NEXT_PHASES_PLAN.md");
            atomic_write_text(&target, &markdown)?;
            atomic_write_text(&mirror, &markdown)?;
            if json_output {
                format_json(&json!({
                    "revision": store.load()?.revision,
                    "outputs": [relative_path(cwd, &target), relative_path(cwd, &mirror)]
                }))
            } else {
                Ok(format!(
                    "Rendered {} and {}.\n",
                    relative_path(cwd, &target),
                    relative_path(cwd, &mirror)
                ))
            }
        }
        "work" => work_command(&store, &options, json_output),
        other => Err(RoadmapError::Usage(format!(
            "unknown roadmap command: {other}; expected list|show|add|drop|start|implemented|block|pass|status|depends|undepends|graph|acceptance|check|render|work"
        ))),
    }
}

fn mutate_status(
    store: &RoadmapStore,
    options: &ParsedOptions,
    status: RoadmapStatus,
    reject_dependents: bool,
    json_output: bool,
) -> Result<String, RoadmapError> {
    let id = options
        .positionals
        .first()
        .ok_or_else(|| RoadmapError::Usage(format!("Usage: roadmap {} <id>", status.as_str())))?;
    mutate_status_explicit(
        store,
        options,
        id,
        status,
        reject_dependents,
        1,
        json_output,
    )
}

fn mutate_status_explicit(
    store: &RoadmapStore,
    options: &ParsedOptions,
    id: &str,
    status: RoadmapStatus,
    reject_dependents: bool,
    reason_start: usize,
    json_output: bool,
) -> Result<String, RoadmapError> {
    let revision = expected_revision(store, options)?;
    let positional_reason = options
        .positionals
        .get(reason_start..)
        .unwrap_or_default()
        .join(" ");
    let reason = options.one("reason").map(str::to_string).or_else(|| {
        (!positional_reason.trim().is_empty()).then(|| positional_reason.trim().to_string())
    });
    let mut prerequisites = options.many("external-prerequisite");
    if status == RoadmapStatus::ExternalBlocked && prerequisites.is_empty() {
        if let Some(reason) = reason.as_ref() {
            prerequisites.push(reason.clone());
        }
    }
    if status == RoadmapStatus::ExternalBlocked && prerequisites.is_empty() {
        return Err(RoadmapError::Usage(
            "roadmap block requires a reason or --external-prerequisite".into(),
        ));
    }
    let evidence_uris = options.many("evidence");
    let evidence_added_at = now();
    let evidence_added_by = actor();
    let event_type = if status == RoadmapStatus::Passed {
        "roadmap_item_passed"
    } else if status == RoadmapStatus::ExternalBlocked {
        "roadmap_item_blocked"
    } else if status == RoadmapStatus::Dropped {
        "roadmap_item_dropped"
    } else {
        "roadmap_item_updated"
    };
    let roadmap = store.mutate(
        revision,
        event_type,
        json!({
            "itemId": id,
            "status": status.as_str(),
            "reason": reason.clone(),
            "externalPrerequisites": prerequisites.clone(),
            "evidence": evidence_uris.clone()
        }),
        |roadmap| {
            if reject_dependents {
                let dependents = roadmap
                    .items
                    .iter()
                    .filter(|item| {
                        item.depends_on
                            .iter()
                            .any(|value| value.eq_ignore_ascii_case(id))
                    })
                    .map(|item| item.id.clone())
                    .collect::<Vec<_>>();
                if !dependents.is_empty() {
                    return Err(RoadmapError::Invalid(format!(
                        "cannot drop {id}; depended on by {}",
                        dependents.join(", ")
                    )));
                }
            }
            let item = find_item_mut(roadmap, id)?;
            item.status = status;
            item.reason = reason;
            for prerequisite in prerequisites {
                if !item.external_prerequisites.contains(&prerequisite) {
                    item.external_prerequisites.push(prerequisite);
                }
            }
            for uri in evidence_uris {
                item.evidence.push(EvidenceLink {
                    uri,
                    acceptance_id: None,
                    added_at: evidence_added_at.clone(),
                    added_by: evidence_added_by.clone(),
                });
            }
            Ok(())
        },
    )?;
    if json_output {
        format_json(find_item(&roadmap, id)?)
    } else {
        Ok(format!(
            "Updated {} at roadmap revision {}.\n",
            id, roadmap.revision
        ))
    }
}

fn acceptance_command(
    store: &RoadmapStore,
    options: &ParsedOptions,
    json_output: bool,
) -> Result<String, RoadmapError> {
    let operation = options
        .positionals
        .first()
        .map(String::as_str)
        .unwrap_or("list");
    let item_id = options.positionals.get(1).ok_or_else(|| {
        RoadmapError::Usage("Usage: roadmap acceptance <list|add|evidence> <item-id> ...".into())
    })?;
    if operation == "list" {
        let roadmap = store.load()?;
        let item = find_item(&roadmap, item_id)?;
        if json_output {
            return format_json(&json!({
                "itemId": item.id,
                "acceptance": item.acceptance,
                "evidence": item.evidence,
            }));
        }
        let mut output = format!("Acceptance for {}\n", item.id);
        for criterion in &item.acceptance {
            let evidence = item
                .evidence
                .iter()
                .filter(|entry| entry.acceptance_id.as_deref() == Some(&criterion.id))
                .count();
            output.push_str(&format!(
                "{}\t{}\t{} evidence\n",
                criterion.id, criterion.text, evidence
            ));
        }
        return Ok(output);
    }
    let revision = expected_revision(store, options)?;
    match operation {
        "add" => {
            let text = options.positionals.get(2..).unwrap_or_default().join(" ");
            if text.trim().is_empty() {
                return Err(RoadmapError::Usage(
                    "Usage: roadmap acceptance add <item-id> <criterion>".into(),
                ));
            }
            let mut criterion_id = String::new();
            let roadmap = store.mutate(
                revision,
                "roadmap_item_updated",
                json!({"itemId": item_id, "operation": "add", "text": text}),
                |roadmap| {
                    let item = find_item_mut(roadmap, item_id)?;
                    criterion_id = options
                        .one("id")
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("acceptance-{}", item.acceptance.len() + 1));
                    item.acceptance.push(AcceptanceCriterion {
                        id: criterion_id.clone(),
                        text,
                    });
                    Ok(())
                },
            )?;
            if json_output {
                format_json(find_item(&roadmap, item_id)?)
            } else {
                Ok(format!(
                    "Added {} to {} at roadmap revision {}.\n",
                    criterion_id, item_id, roadmap.revision
                ))
            }
        }
        "evidence" => {
            let criterion_id = options.positionals.get(2).ok_or_else(|| {
                RoadmapError::Usage(
                    "Usage: roadmap acceptance evidence <item-id> <acceptance-id> <artifact-uri>"
                        .into(),
                )
            })?;
            let uri = options.positionals.get(3).ok_or_else(|| {
                RoadmapError::Usage(
                    "Usage: roadmap acceptance evidence <item-id> <acceptance-id> <artifact-uri>"
                        .into(),
                )
            })?;
            let roadmap = store.mutate(
                revision,
                "roadmap_evidence_attached",
                json!({"itemId": item_id, "acceptanceId": criterion_id, "uri": uri}),
                |roadmap| {
                    let item = find_item_mut(roadmap, item_id)?;
                    if !item
                        .acceptance
                        .iter()
                        .any(|criterion| criterion.id == *criterion_id)
                    {
                        return Err(RoadmapError::Invalid(format!(
                            "{} has no acceptance criterion {}",
                            item.id, criterion_id
                        )));
                    }
                    item.evidence.push(EvidenceLink {
                        uri: uri.clone(),
                        acceptance_id: Some(criterion_id.clone()),
                        added_at: now(),
                        added_by: actor(),
                    });
                    Ok(())
                },
            )?;
            if json_output {
                format_json(find_item(&roadmap, item_id)?)
            } else {
                Ok(format!(
                    "Attached evidence to {} at roadmap revision {}.\n",
                    item_id, roadmap.revision
                ))
            }
        }
        other => Err(RoadmapError::Usage(format!(
            "unknown acceptance operation: {other}"
        ))),
    }
}

fn work_command(
    store: &RoadmapStore,
    options: &ParsedOptions,
    json_output: bool,
) -> Result<String, RoadmapError> {
    let item_id = options
        .positionals
        .first()
        .ok_or_else(|| RoadmapError::Usage("Usage: roadmap work <item-id>".into()))?;
    let roadmap = store.load()?;
    let item = find_item(&roadmap, item_id)?.clone();
    if item.status == RoadmapStatus::Dropped || item.status == RoadmapStatus::Passed {
        return Err(RoadmapError::Invalid(format!(
            "cannot work on {} while status is {}",
            item.id, item.status
        )));
    }
    let blockers = item
        .depends_on
        .iter()
        .filter_map(|dependency| find_item(&roadmap, dependency).ok())
        .filter(|dependency| dependency.status != RoadmapStatus::Passed)
        .map(|dependency| format!("{} ({})", dependency.id, dependency.status))
        .collect::<Vec<_>>();
    if !blockers.is_empty() {
        return Err(RoadmapError::Invalid(format!(
            "{} is blocked by unresolved dependencies: {}",
            item.id,
            blockers.join(", ")
        )));
    }
    let plan = format!(
        "Roadmap item {}: {}\n\nAcceptance criteria:\n{}",
        item.id,
        item.title,
        item.acceptance
            .iter()
            .map(|criterion| format!("- [{}] {}", criterion.id, criterion.text))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let todos = item
        .acceptance
        .iter()
        .map(|criterion| {
            (
                format!("{}: {}", criterion.id, criterion.text),
                "pending".to_string(),
            )
        })
        .collect::<Vec<_>>();
    crate::slash::activate_roadmap_work(
        &store.cwd,
        &item.id,
        &format!("Complete roadmap item {}: {}", item.id, item.title),
        &plan,
        &todos,
    )?;
    let session_path = crate::agent::record_roadmap_event(
        &store.cwd,
        "roadmap_item_started",
        json!({
            "itemId": item.id,
            "revision": roadmap.revision,
            "artifactPolicy": "new session artifacts and branches inherit activeRoadmapItem"
        }),
    )?;
    if json_output {
        format_json(&json!({
            "itemId": item.id,
            "revision": roadmap.revision,
            "sessionPath": session_path.to_string_lossy(),
            "goal": format!("Complete roadmap item {}: {}", item.id, item.title),
            "todoCount": todos.len(),
        }))
    } else {
        Ok(format!(
            "Roadmap work activated for {}. Goal, plan, and {} todos now point to this item.\nSession: {}\n",
            item.id,
            todos.len(),
            session_path.display()
        ))
    }
}

fn atomic_write_text(path: &Path, content: &str) -> Result<(), RoadmapError> {
    let parent = path
        .parent()
        .ok_or_else(|| RoadmapError::Io("output path has no parent".into()))?;
    fs::create_dir_all(parent)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = parent.join(format!(
        ".{}.tmp-{}-{nonce}",
        path.file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default(),
        std::process::id()
    ));
    let result = (|| -> Result<(), RoadmapError> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(content.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub fn split_command_line(input: &str) -> Result<Vec<String>, RoadmapError> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in input.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if character == active {
                quote = None;
            } else {
                current.push(character);
            }
            continue;
        }
        if character == '\'' || character == '"' {
            quote = Some(character);
        } else if character.is_whitespace() {
            if !current.is_empty() {
                args.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if escaped || quote.is_some() {
        return Err(RoadmapError::Usage(
            "unterminated escape or quote in roadmap command".into(),
        ));
    }
    if !current.is_empty() {
        args.push(current);
    }
    Ok(args)
}

pub fn picker(cwd: &Path) -> Result<PickerSpec, RoadmapError> {
    let roadmap = RoadmapStore::new(cwd).load()?;
    let mut items = vec![PickerItem::action(
        "Add roadmap item",
        "/roadmap add --title \"\" --area general --priority P2 --summary \"\" --acceptance \"\"",
    )
    .detail("Prefill the required title, area, priority, summary, and acceptance fields")
    .badge("ADD")
    .prefill()];
    items.extend(roadmap.items.into_iter().map(|item| {
        PickerItem::action(
            format!("{}  {}", item.id, item.title),
            format!("/roadmap show {}", item.id),
        )
        .detail(format!(
            "{} · {} · {}",
            item.status, item.priority, item.area
        ))
        .badge(item.priority)
    }));
    let mut picker = PickerSpec::new("Roadmap", items);
    picker.prompt = "Search roadmap items:".into();
    picker.empty_message = "No roadmap items match".into();
    Ok(picker)
}

pub fn capability_descriptors(cwd: &Path) -> Vec<CapabilityDescriptor> {
    let store = RoadmapStore::new(cwd);
    let path = store.path().to_path_buf();
    let health = match store.load() {
        Ok(roadmap) => {
            let errors = store.validation_errors(&roadmap, false);
            if errors.is_empty() {
                CapabilityHealth::healthy()
            } else {
                CapabilityHealth::unavailable(errors.join("; "))
            }
        }
        Err(error) => CapabilityHealth::unavailable(format!("{}: {error}", path.display())),
    };
    let planned =
        CapabilityHealth::unavailable("Planned by JED-024; no review runtime is executable yet");
    vec![
        CapabilityDescriptor::new(
            "service/roadmap-registry",
            CapabilityKind::Service,
            "jeden-core",
            "Roadmap registry",
            "Versioned repository roadmap with revision-guarded atomic mutations",
            FunctionTarget::Service {
                name: "roadmap-registry".into(),
            },
        )
        .operation("read")
        .operation("mutate")
        .operation("validate")
        .operation("render")
        .health(health),
        CapabilityDescriptor::new(
            "service/review-runtime",
            CapabilityKind::Service,
            "jeden-roadmap",
            "Review runtime",
            "Planned typed review execution contract",
            FunctionTarget::Service {
                name: "review-runtime".into(),
            },
        )
        .operation("planned")
        .health(planned.clone()),
        CapabilityDescriptor::new(
            "slash/review",
            CapabilityKind::SlashCommand,
            "jeden-roadmap",
            "/review",
            "Planned native review command",
            FunctionTarget::BuiltinSlash {
                command: "review".into(),
            },
        )
        .operation("planned")
        .health(planned.clone()),
        CapabilityDescriptor::new(
            "view/review",
            CapabilityKind::View,
            "jeden-roadmap",
            "Review",
            "Planned native review picker",
            FunctionTarget::NativeView {
                command: "review".into(),
            },
        )
        .operation("planned")
        .health(planned),
    ]
}
