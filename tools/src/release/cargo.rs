//! Running Cargo with the declared private-source input, and staging the
//! native binaries a release worker packages.

use super::digest_file;
use crate::repository_root;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) const INPUT_ENV: &str = "WISENT_INPUT_PRIVATE_CARGO_SOURCES_DIR";
const OUTPUT_ENV: &str = "WISENT_OUTPUT_DIR";
/// The provenance layout `export` writes and `cargo` accepts. Published inputs
/// carry it, so it changes only together with a new input and both readers.
pub(super) const PROVENANCE_SCHEMA_VERSION: u64 = 1;

/// A non-empty environment value, trimmed; an unset or blank one is absent.
fn setting(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Cargo from PATH, then from `$CARGO_HOME/bin`, else `$HOME/.cargo/bin`:
/// release workers may carry only the system PATH and no shell startup files.
/// The Rustup proxy name is kept as found.
pub(super) fn cargo_program() -> Result<PathBuf, String> {
    let name = format!("cargo{}", env::consts::EXE_SUFFIX);
    if let Some(found) = env::var_os("PATH")
        .iter()
        .flat_map(env::split_paths)
        .map(|directory| directory.join(&name))
        .find(|candidate| executable(candidate))
    {
        return Ok(found);
    }
    let home = match (setting("CARGO_HOME"), setting("HOME")) {
        (Some(cargo_home), _) => PathBuf::from(cargo_home),
        (None, Some(home)) => PathBuf::from(home).join(".cargo"),
        (None, None) => {
            return Err("Cargo is unavailable on PATH, and neither CARGO_HOME nor HOME names a Cargo home; provision the Rust toolchain in CARGO_HOME before building".into())
        }
    };
    let program = std::path::absolute(home.join("bin").join(&name))
        .map_err(|error| format!("cannot resolve the Cargo home: {error}"))?;
    if executable(&program) {
        return Ok(program);
    }
    Err(format!(
        "Cargo is unavailable on PATH and at {}; provision the Rust toolchain in CARGO_HOME before building",
        program.display()
    ))
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(path: &Path) -> bool {
    path.is_file()
}

/// The Cargo configuration that replaces every locked Git source with the
/// exported directory. Written by `export` and recomputed by `cargo`, so an
/// input whose configuration disagrees with its own provenance is refused.
/// A locked source's query keys become its Git reference keys as written;
/// Cargo refuses the configuration if one is not a reference it knows.
pub(super) fn source_config(sources: &BTreeSet<String>) -> Result<String, String> {
    let mut lines = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        let located = source.strip_prefix("git+").unwrap_or(source);
        let mut url = url::Url::parse(located)
            .map_err(|error| format!("unsupported locked Git source: {source}: {error}"))?;
        let mut references = BTreeMap::<String, Vec<String>>::new();
        for (key, value) in url.query_pairs() {
            references
                .entry(key.into_owned())
                .or_default()
                .push(value.into_owned());
        }
        url.set_query(None);
        url.set_fragment(None);
        lines.push(format!("[source.private-git-{index}]"));
        lines.push(format!("git = {}", quoted(url.as_str())));
        for (key, values) in &references {
            let [value] = values.as_slice() else {
                return Err(format!("ambiguous locked Git source: {source}"));
            };
            lines.push(format!("{key} = {}", quoted(value)));
        }
        lines.push("replace-with = \"private-cargo-sources\"".into());
        lines.push(String::new());
    }
    lines.push("[source.private-cargo-sources]".into());
    lines.push("directory = \"sources\"".into());
    lines.push(String::new());
    Ok(lines.join("\n"))
}

/// A TOML basic string; JSON string escaping is a subset TOML accepts.
fn quoted(value: &str) -> String {
    Value::String(value.into()).to_string()
}

fn read_json(path: &Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}

pub(super) fn cargo(arguments: &[String]) -> Result<u8, String> {
    let Some(configured) = setting(INPUT_ENV) else {
        return Err(format!(
            "{INPUT_ENV} is required for release Cargo commands"
        ));
    };
    let input = fs::canonicalize(&configured)
        .map_err(|error| format!("{INPUT_ENV} {configured}: {error}"))?;
    let provenance = read_json(&input.join("provenance.json"))?;
    if provenance["schema_version"] != PROVENANCE_SCHEMA_VERSION {
        return Err("unsupported private Cargo source schema".into());
    }
    let lock = digest_file(&repository_root().join("Cargo.lock"))?;
    if provenance["cargo_lock_sha256"].as_str() != Some(lock.as_str()) {
        return Err(
            "private Cargo sources do not match Cargo.lock; export and publish a new input".into(),
        );
    }
    let sources = provenance["packages"]
        .as_array()
        .ok_or("private Cargo source provenance lists no packages")?
        .iter()
        .map(|package| {
            package["source"]
                .as_str()
                .map(str::to_string)
                .ok_or("a private Cargo source package names no source")
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let configuration = input.join("config.toml");
    let declared = fs::read_to_string(&configuration)
        .map_err(|error| format!("{}: {error}", configuration.display()))?;
    if declared != source_config(&sources)? {
        return Err("private Cargo source configuration does not match its provenance".into());
    }
    let directory = format!(
        "source.private-cargo-sources.directory={}",
        quoted(&input.join("sources").display().to_string())
    );
    let status = Command::new(cargo_program()?)
        .arg("--config")
        .arg(&configuration)
        .arg("--config")
        .arg(directory)
        .args(arguments)
        .current_dir(repository_root())
        .status()
        .map_err(|error| format!("cannot start Cargo: {error}"))?;
    match status.code().map(u8::try_from) {
        Some(Ok(code)) => Ok(code),
        _ => Err(format!("Cargo ended without an exit code: {status}")),
    }
}

pub(super) fn stage(binaries: &[String]) -> Result<u8, String> {
    let Some(configured) = setting(OUTPUT_ENV) else {
        return Err(format!("{OUTPUT_ENV} is required for native staging"));
    };
    let output = std::path::absolute(&configured)
        .map_err(|error| format!("{OUTPUT_ENV}: {error}"))?
        .join("bin");
    let target = repository_root().join("target");
    let mut arguments = vec![
        "build".to_string(),
        "--release".into(),
        "--locked".into(),
        "--target-dir".into(),
        target.display().to_string(),
    ];
    for binary in binaries {
        arguments.push("--bin".into());
        arguments.push(binary.clone());
    }
    let tool = env::current_exe().map_err(|error| error.to_string())?;
    eprintln!("native stage: jeden-tools {}", tool.display());
    let status = cargo(&arguments)?;
    if status != 0 {
        return Ok(status);
    }
    fs::create_dir_all(&output).map_err(|error| format!("{}: {error}", output.display()))?;
    for binary in binaries {
        let name = format!("{binary}{}", env::consts::EXE_SUFFIX);
        let source = target.join("release").join(&name);
        let destination = output.join(&name);
        fs::copy(&source, &destination).map_err(|error| {
            format!("{} -> {}: {error}", source.display(), destination.display())
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&destination, fs::Permissions::from_mode(0o755))
                .map_err(|error| format!("{}: {error}", destination.display()))?;
        }
        eprintln!("native stage: {}", destination.display());
    }
    Ok(0)
}
