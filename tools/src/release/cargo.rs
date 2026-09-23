//! Running Cargo with the declared private-source input, and staging the
//! native binaries a release worker packages.

use crate::repository_root;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) const INPUT_ENV: &str = "WISENT_INPUT_PRIVATE_CARGO_SOURCES_DIR";
pub(super) const OUTPUT_ENV: &str = "WISENT_OUTPUT_DIR";
/// The provenance layout `export` writes and `cargo` accepts. Published inputs
/// carry it, so it changes only together with a new input and both readers.
pub(super) const PROVENANCE_SCHEMA_VERSION: u64 = 1;

/// A non-empty environment value, trimmed; an unset or blank one is absent.
pub(super) fn setting(name: &str) -> Option<String> {
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
    let lock_path = repository_root().join("Cargo.lock");
    let lock = fs::read_to_string(&lock_path)
        .map_err(|error| format!("{}: {error}", lock_path.display()))?;
    let locked = locked_private_packages(&lock)?;
    let carried = provenance["packages"]
        .as_array()
        .ok_or("private Cargo source provenance lists no packages")?
        .iter()
        .map(|package| {
            match (
                package["name"].as_str(),
                package["version"].as_str(),
                package["source"].as_str(),
            ) {
                (Some(name), Some(version), Some(source)) => {
                    Ok([name.to_string(), version.to_string(), source.to_string()])
                }
                _ => Err(format!(
                    "a private Cargo source package is incomplete: {package}"
                )),
            }
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if carried != locked {
        return Err(format!(
            "private Cargo sources do not match Cargo.lock; export and publish a new input \
             (the input carries {}, Cargo.lock locks {})",
            listed(&carried),
            listed(&locked)
        ));
    }
    let sources = carried
        .into_iter()
        .map(|[_, _, source]| source)
        .collect::<BTreeSet<_>>();
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

/// Every private Git package `Cargo.lock` locks, as name, version and source.
///
/// This is what an exported input has to match, because it is all the input
/// carries. The input used to be bound to the whole lockfile's digest, and
/// every version bump of jeden rewrites jeden's own entry there: release run
/// 7ebda0f9 (0.1.22) and six queued changes before it died at their first
/// quality step on an unchanged pair of private crates.
fn locked_private_packages(lock: &str) -> Result<BTreeSet<[String; 3]>, String> {
    let mut packages = BTreeSet::new();
    for block in lock.split("[[package]]").skip(1) {
        let field = |key: &str| -> Result<Option<String>, String> {
            let prefix = format!("{key} = ");
            match block
                .lines()
                .find_map(|line| line.strip_prefix(prefix.as_str()))
            {
                None => Ok(None),
                Some(value) => value
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                    .map(|value| Some(value.to_string()))
                    .ok_or_else(|| format!("Cargo.lock {key} is not a plain string: {value}")),
            }
        };
        // Registry packages name a `registry+` source and path packages none;
        // only Git sources are exported.
        let Some(source) = field("source")?.filter(|source| source.starts_with("git+")) else {
            continue;
        };
        let name =
            field("name")?.ok_or_else(|| format!("Cargo.lock locks {source} without a name"))?;
        let version = field("version")?
            .ok_or_else(|| format!("Cargo.lock locks {name} without a version"))?;
        packages.insert([name, version, source]);
    }
    Ok(packages)
}

fn listed(packages: &BTreeSet<[String; 3]>) -> String {
    packages
        .iter()
        .map(|[name, version, source]| format!("{name} {version} from {source}"))
        .collect::<Vec<_>>()
        .join(", ")
}
