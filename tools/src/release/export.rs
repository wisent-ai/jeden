//! Exporting the private Git crates `Cargo.lock` names as one immutable,
//! checksum-protected input a release worker mounts offline.

use super::cargo::{cargo_program, source_config, PROVENANCE_SCHEMA_VERSION};
use super::{digest_file, output};
use crate::repository_root;
use flate2::{Compression, GzBuilder};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::process::{self, Command};
use std::time::{SystemTime, UNIX_EPOCH};

const INPUT_NAME: &str = "private-cargo-sources";

/// A scratch directory inside the build output, removed however the export
/// ends.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// `path` made absolute with `.` and `..` resolved lexically, so a refusal is
/// decided before anything is created.
fn normalized(path: &Path) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
    let mut clean = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            other => clean.push(other),
        }
    }
    Ok(clean)
}

pub(super) fn export(requested: &Path) -> Result<(), String> {
    let root = repository_root();
    let build = root.join(".wisent-output");
    let archive = normalized(requested)?;
    if !archive.starts_with(&build) || archive == build {
        return Err(format!(
            "private source output must be inside {}",
            build.display()
        ));
    }
    let parent = archive
        .parent()
        .ok_or("the archive path names no directory")?;
    fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    let lock_path = root.join("Cargo.lock");
    let lock_digest = digest_file(&lock_path)?;
    let program = cargo_program()?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let scratch = Scratch(build.join(format!("cargo-input-{}-{stamp}", process::id())));
    let vendor = scratch.0.join("vendor");
    fs::create_dir_all(&scratch.0).map_err(|error| format!("{}: {error}", scratch.0.display()))?;
    output(
        Command::new(&program)
            .args(["vendor", "--locked", "--versioned-dirs", "--quiet"])
            .arg(&vendor),
    )?;
    let metadata: Value = serde_json::from_str(&output(Command::new(&program).args([
        "metadata",
        "--locked",
        "--offline",
        "--format-version=1",
    ]))?)
    .map_err(|error| format!("cargo metadata: {error}"))?;
    let mut packages = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata lists no packages")?
        .iter()
        .filter(|package| {
            package["source"]
                .as_str()
                .is_some_and(|source| source.starts_with("git+"))
        })
        .collect::<Vec<_>>();
    packages.sort_by_key(|package| {
        (
            package["name"].as_str().map(str::to_string),
            package["version"].as_str().map(str::to_string),
        )
    });
    if packages.is_empty() {
        return Err("Cargo.lock contains no private Git source packages".into());
    }
    let payload = scratch.0.join("payload");
    let sources = payload.join("sources");
    fs::create_dir_all(&sources).map_err(|error| format!("{}: {error}", sources.display()))?;
    let mut records = Vec::new();
    for package in &packages {
        let (Some(name), Some(version), Some(source)) = (
            package["name"].as_str(),
            package["version"].as_str(),
            package["source"].as_str(),
        ) else {
            return Err(format!("cargo metadata package is incomplete: {package}"));
        };
        let directory = format!("{name}-{version}");
        let vendored = vendor.join(&directory);
        if !vendored.join(".cargo-checksum.json").is_file() {
            return Err(format!(
                "Cargo did not vendor checksum-protected source: {directory}"
            ));
        }
        fs::rename(&vendored, sources.join(&directory))
            .map_err(|error| format!("{}: {error}", vendored.display()))?;
        records.push(json!({"name": name, "version": version, "source": source}));
    }
    if digest_file(&lock_path)? != lock_digest {
        return Err("Cargo.lock changed while exporting private sources".into());
    }
    let provenance = json!({
        "schema_version": PROVENANCE_SCHEMA_VERSION,
        "cargo_lock_sha256": lock_digest,
        "packages": records,
    });
    write_json(&payload.join("provenance.json"), &provenance)?;
    let locked = records
        .iter()
        .filter_map(|record| record["source"].as_str().map(str::to_string))
        .collect::<BTreeSet<_>>();
    fs::write(payload.join("config.toml"), source_config(&locked)?)
        .map_err(|error| format!("config.toml: {error}"))?;
    let staged = scratch.0.join(format!("{INPUT_NAME}.tar.gz"));
    pack(&payload, &staged)?;
    let sha256 = digest_file(&staged)?;
    fs::rename(&staged, &archive).map_err(|error| format!("{}: {error}", archive.display()))?;
    let revision = output(Command::new("git").args(["rev-parse", "HEAD"]))?;
    let receipt = json!({
        "source_revision": revision.trim(),
        "archive": archive.display().to_string(),
        "sha256": sha256,
        "input": {
            "uri": format!("stado://sources/jeden/{INPUT_NAME}/{sha256}/{INPUT_NAME}.tar.gz"),
            "sha256": sha256,
            "mount": INPUT_NAME,
            "extract": true,
        },
        "provenance": provenance,
    });
    let mut receipt_path = archive.into_os_string();
    receipt_path.push(".json");
    write_json(Path::new(&receipt_path), &receipt)?;
    println!("{}", pretty(&receipt)?);
    Ok(())
}

fn pretty(value: &Value) -> Result<String, String> {
    serde_json::to_string_pretty(value).map_err(|error| error.to_string())
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    fs::write(path, pretty(value)? + "\n").map_err(|error| format!("{}: {error}", path.display()))
}

/// A gzip-compressed tar of the payload's entries in name order, with owners,
/// times and the gzip header cleared, so the same sources give the same bytes.
fn pack(payload: &Path, archive: &Path) -> Result<(), String> {
    let file = File::create(archive).map_err(|error| format!("{}: {error}", archive.display()))?;
    let encoder = GzBuilder::new().mtime(0).write(file, Compression::best());
    let mut tar = tar::Builder::new(encoder);
    tar.mode(tar::HeaderMode::Deterministic);
    tar.follow_symlinks(false);
    append_sorted(&mut tar, payload, Path::new(""))?;
    tar.into_inner()
        .and_then(|encoder| encoder.finish())
        .map_err(|error| format!("{}: {error}", archive.display()))?;
    Ok(())
}

fn append_sorted<W: std::io::Write>(
    tar: &mut tar::Builder<W>,
    directory: &Path,
    prefix: &Path,
) -> Result<(), String> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    entries.sort();
    for path in entries {
        let name = prefix.join(path.file_name().ok_or("an entry has no name")?);
        tar.append_path_with_name(&path, &name)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if path.is_dir() && !path.is_symlink() {
            append_sorted(tar, &path, &name)?;
        }
    }
    Ok(())
}
