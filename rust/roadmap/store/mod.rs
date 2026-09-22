//! Reading and writing the roadmap file so two writers cannot lose each
//! other's work.
//!
//! Split out of `roadmap/mod.rs`, which had grown past the module line cap.

use super::model::{RoadmapError, RoadmapFile, RoadmapItem, RoadmapStatus};
use super::normalize::{cycle_errors, normalize};
use super::render::render_markdown;
use super::{CheckReport, RoadmapGraph, RoadmapGraphEdge, RoadmapGraphNode, ROADMAP_SCHEMA_VERSION};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

const LOCK_RETRIES: usize = 500;
const LOCK_WAIT: Duration = Duration::from_millis(10);

pub struct RoadmapStore {
mod lock;

use lock::StableLock;

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
