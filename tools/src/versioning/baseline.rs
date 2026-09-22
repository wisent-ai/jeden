//! Regenerating released-surface.json from jeden's most recently published
//! artifact.
//!
//! The baseline a version gate measures against has to come from something a
//! user can obtain. Jeden publishes signed per-platform binaries to GitHub
//! Releases, so that is the channel read here; not crates.io, where the
//! `jeden` name belongs to an unrelated third party's `cargo new` stub.
//!
//! In order: find every release asset named `jeden-<version>-<target>.tar.gz`
//! and take the version of the newest release, never the version Cargo.toml
//! declares; take `version` and `minimumVersion` from that release's signed
//! `manifest.dsse.json` and the exact `sourceSha` from its `channel.json`, so
//! every number is the artifact's own claim; reproduce that revision with
//! `git archive` and extract its surface.
//!
//! MARKER_PREFIX is the machine-readable statement of where a baseline came
//! from: the first whitespace-delimited token of `source`, read back by
//! .github/workflows/version-check.yml.

use crate::release::output;
use crate::repository_root;
use crate::surface::surface;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::time::{SystemTime, UNIX_EPOCH};

const REPOSITORY: &str = "wisent-ai/jeden";
pub(super) const MARKER_PREFIX: &str = "gh-release:";
const BASELINE_FILE: &str = "released-surface.json";
const CLAIM_FILES: [&str; 2] = ["manifest.dsse.json", "channel.json"];

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn gh(arguments: &[&str]) -> Result<Value, String> {
    let text = output(
        Command::new("gh")
            .args(arguments)
            .args(["--repo", REPOSITORY]),
    )?;
    serde_json::from_str(&text).map_err(|error| format!("gh {}: {error}", arguments.join(" ")))
}

/// `(tag, version, asset names)` of every release serving a versioned jeden
/// artifact, newest publication first.
fn published_releases() -> Result<Vec<(String, String, Vec<String>)>, String> {
    let asset = Regex::new(r"^jeden-(?P<version>.+?)-(?P<target>[a-z0-9_]+-[a-z0-9_-]+)\.tar\.gz$")
        .map_err(|error| error.to_string())?;
    let mut listed = gh(&["release", "list", "--json", "tagName,createdAt"])?
        .as_array()
        .cloned()
        .ok_or("gh release list returned no list")?;
    listed.sort_by(|left, right| right["createdAt"].as_str().cmp(&left["createdAt"].as_str()));
    let mut found = Vec::new();
    for release in listed {
        let tag = release["tagName"]
            .as_str()
            .ok_or("a release carries no tag name")?
            .to_string();
        let names = gh(&["release", "view", &tag, "--json", "assets"])?["assets"]
            .as_array()
            .ok_or_else(|| format!("{tag}: no asset list"))?
            .iter()
            .filter_map(|entry| entry["name"].as_str().map(str::to_string))
            .collect::<Vec<_>>();
        let versions = names
            .iter()
            .filter_map(|name| asset.captures(name))
            .map(|captures| captures["version"].to_string())
            .collect::<BTreeSet<_>>();
        if versions.len() > 1 {
            return Err(format!("{tag}: serves more than one version: {versions:?}"));
        }
        if let Some(version) = versions.into_iter().next() {
            found.push((tag, version, names));
        }
    }
    if found.is_empty() {
        return Err(format!(
            "{REPOSITORY} serves no jeden-<version>-<target>.tar.gz asset, so there is no published baseline to recover"
        ));
    }
    Ok(found)
}

/// The one release the baseline is recovered from: the version of the newest
/// release, then the lexicographically first tag serving it, so the marker
/// does not depend on which parallel per-target job finished last.
fn canonical_release() -> Result<(String, String, Vec<String>), String> {
    let releases = published_releases()?;
    let newest = releases[0].1.clone();
    releases
        .into_iter()
        .filter(|(_, version, _)| *version == newest)
        .min_by(|left, right| left.0.cmp(&right.0))
        .ok_or_else(|| "no release serves the newest version".into())
}

fn text_claim(value: &Value, tag: &str, key: &str) -> Result<String, String> {
    value[key]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{tag}: {key} is missing from the published evidence"))
}

/// The release's own statements: published version, floor, source revision.
fn artifact_claims(tag: &str, names: &[String], scratch: &Path) -> Result<[String; 3], String> {
    let missing = CLAIM_FILES
        .iter()
        .filter(|file| !names.iter().any(|name| name == *file))
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "{tag}: no {}, so its version cannot be trusted",
            missing.join(", ")
        ));
    }
    let mut download = Command::new("gh");
    download.args(["release", "download", tag, "--repo", REPOSITORY]);
    for file in CLAIM_FILES {
        download.args(["--pattern", file]);
    }
    output(download.arg("--dir").arg(scratch))?;
    let read = |file: &str| -> Result<Value, String> {
        let bytes = fs::read(scratch.join(file)).map_err(|error| format!("{file}: {error}"))?;
        serde_json::from_slice(&bytes).map_err(|error| format!("{file}: {error}"))
    };
    let envelope = read("manifest.dsse.json")?;
    let payload = STANDARD
        .decode(
            envelope["payload"]
                .as_str()
                .ok_or("manifest.dsse.json has no payload")?,
        )
        .map_err(|error| format!("manifest payload: {error}"))?;
    let manifest: Value =
        serde_json::from_slice(&payload).map_err(|error| format!("manifest payload: {error}"))?;
    let channel = read("channel.json")?;
    let version = text_claim(&manifest, tag, "version")?;
    let floor = text_claim(&manifest, tag, "minimumVersion")?;
    let source = text_claim(&channel, tag, "sourceSha")?;
    let triple = Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+$").map_err(|error| error.to_string())?;
    if !triple.is_match(&floor) {
        return Err(format!("{tag}: minimumVersion {floor:?} is not a stable triple, so the shared rule would have no version slot to advance"));
    }
    Ok([version, floor, source])
}

/// The surface of the exact revision the published artifact was built from.
fn published_surface(source_sha: &str, scratch: &Path) -> Result<Vec<String>, String> {
    let kind = output(Command::new("git").args(["cat-file", "-t", source_sha]))?;
    if kind.trim() != "commit" {
        return Err(format!("{source_sha} is a {}, not a commit", kind.trim()));
    }
    let archive = Command::new("git")
        .args(["archive", source_sha])
        .current_dir(repository_root())
        .output()
        .map_err(|error| format!("cannot start git archive: {error}"))?;
    if !archive.status.success() {
        return Err(format!(
            "cannot reproduce {source_sha}: {}",
            String::from_utf8_lossy(&archive.stderr).trim()
        ));
    }
    let tree = scratch.join("tree");
    tar::Archive::new(archive.stdout.as_slice())
        .unpack(&tree)
        .map_err(|error| format!("cannot unpack {source_sha}: {error}"))?;
    surface(&tree)
}

fn scratch() -> Result<Scratch, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let path = repository_root()
        .join(".wisent-output")
        .join(format!("baseline-{}-{stamp}", process::id()));
    fs::create_dir_all(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(Scratch(path))
}

/// The marker this generator would produce now, without reproducing a source
/// revision: a shallow CI checkout does not have the published commit, and
/// regenerating the surface at check time is the one shape that cannot refuse.
pub(super) fn marker() -> Result<String, String> {
    let (tag, _, _) = canonical_release()?;
    Ok(format!("{MARKER_PREFIX}{tag}"))
}

/// Rewrite released-surface.json, or print the document with `to_stdout`.
pub(super) fn regenerate(to_stdout: bool) -> Result<(), String> {
    let (tag, version, names) = canonical_release()?;
    let scratch = scratch()?;
    let [claimed, floor, source_sha] = artifact_claims(&tag, &names, &scratch.0)?;
    if claimed != version {
        return Err(format!(
            "{tag}: asset is named {version} but its signed manifest says {claimed}"
        ));
    }
    let recovered = published_surface(&source_sha, &scratch.0)?;
    let count = recovered.len();
    let document = json!({
        "version": version,
        "minimumVersion": floor,
        "source": format!(
            "{MARKER_PREFIX}{tag} surface reproduced with `git archive` from {source_sha}, the sourceSha that release's channel.json declares; version and minimumVersion copied from its signed manifest.dsse.json"
        ),
        "surface": recovered,
    });
    let text = serde_json::to_string_pretty(&document).map_err(|error| error.to_string())? + "\n";
    if to_stdout {
        print!("{text}");
        return Ok(());
    }
    let path = repository_root().join(BASELINE_FILE);
    fs::write(&path, text).map_err(|error| format!("{}: {error}", path.display()))?;
    eprintln!("{BASELINE_FILE}: {version} (floor {floor}), {count} names");
    Ok(())
}
