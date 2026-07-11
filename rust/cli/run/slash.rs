//! Slash help, builtin routing, model/settings slash, and self-update.

use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::cli::auth::{format_auth_status, logout, refresh, start_login};
use crate::cli::config::load_config;
use crate::cli::config::schema::config_command;
use crate::{config_path, read_json, session_root, slash, Args};


fn format_slash_help(cwd: &Path) -> String {
    let mut out = String::from("Jeden slash commands:\n");
    for descriptor in crate::capability::slash_descriptors(cwd) {
        let Some(command) = descriptor.ui.action.as_deref() else { continue };
        out.push_str(&format!("/{:<15} {}\n", command.trim_start_matches('/'), descriptor.ui.description));
    }
    out
}

/// True for every slash command Jeden handles itself (canonical list + aliases).
/// Unknown slash input forwards to the model as a prompt instead of hard-erroring.
pub(crate) fn is_builtin_slash(command: &str) -> bool {
    crate::capability::is_builtin_slash(command)
}

const UPDATE_MAX_BYTES: usize = 256 * 1024 * 1024;
const UPDATE_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateManifest {
    schema_version: u32,
    version: String,
    artifact: String,
    sha256: String,
    signature: String,
}

fn fetch_update_bytes(location: &str) -> Result<Vec<u8>, String> {
    if location.starts_with("https://") {
        let response = reqwest::blocking::Client::builder().timeout(Duration::from_secs(30)).build().map_err(|error| error.to_string())?
            .get(location).send().map_err(|error| format!("download {location}: {error}"))?;
        if !response.status().is_success() { return Err(format!("download {location} returned {}", response.status())); }
        if response.content_length().is_some_and(|size| size > UPDATE_MAX_BYTES as u64) { return Err("update artifact exceeds 256 MiB".into()); }
        let bytes = response.bytes().map_err(|error| error.to_string())?;
        if bytes.len() > UPDATE_MAX_BYTES { return Err("update artifact exceeds 256 MiB".into()); }
        return Ok(bytes.to_vec());
    }
    let path = location.strip_prefix("file://").unwrap_or(location);
    let metadata = fs::metadata(path).map_err(|error| format!("read {path}: {error}"))?;
    if metadata.len() > UPDATE_MAX_BYTES as u64 { return Err("update artifact exceeds 256 MiB".into()); }
    fs::read(path).map_err(|error| format!("read {path}: {error}"))
}

fn resolve_artifact(manifest_location: &str, artifact: &str) -> String {
    if artifact.contains("://") || Path::new(artifact).is_absolute() { return artifact.into(); }
    if manifest_location.starts_with("https://") {
        return reqwest::Url::parse(manifest_location).ok().and_then(|base| base.join(artifact).ok()).map(|url| url.to_string()).unwrap_or_else(|| artifact.into());
    }
    let path = Path::new(manifest_location.strip_prefix("file://").unwrap_or(manifest_location));
    path.parent().unwrap_or_else(|| Path::new(".")).join(artifact).display().to_string()
}

fn verify_manifest(manifest: &UpdateManifest, signing_key: &[u8]) -> Result<(), String> {
    if manifest.schema_version != 1 { return Err(format!("unsupported update manifest schema {}", manifest.schema_version)); }
    if manifest.version.trim().is_empty() || manifest.version.len() > 64 || !manifest.version.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')) || manifest.sha256.len() != 64 || !manifest.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) { return Err("update manifest has invalid version or SHA-256".into()); }
    let signature = hex::decode(&manifest.signature).map_err(|_| "update manifest signature is not hex".to_string())?;
    let payload = format!("{}\n{}\n{}", manifest.version, manifest.artifact, manifest.sha256.to_ascii_lowercase());
    let mut mac = Hmac::<Sha256>::new_from_slice(signing_key).map_err(|_| "invalid update signing key".to_string())?;
    mac.update(payload.as_bytes());
    mac.verify_slice(&signature).map_err(|_| "update manifest signature verification failed".to_string())
}

fn run_health(binary: &Path, cwd: &Path) -> Result<(), String> {
    let started = Instant::now();
    let mut child = Command::new(binary).args(["doctor", "--json", "--cwd"]).arg(cwd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
        .map_err(|error| format!("post-update health failed to start {}: {error}", binary.display()))?;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("health probe failed with {status}")),
            Ok(None) if started.elapsed() < UPDATE_PROBE_TIMEOUT => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => { let _ = child.kill(); let _ = child.wait(); return Err("health probe timed out".into()); }
            Err(error) => { let _ = child.kill(); return Err(format!("health probe failed: {error}")); }
        }
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path).map_err(|error| error.to_string())?.permissions();
    permissions.set_mode(0o755); fs::set_permissions(path, permissions).map_err(|error| error.to_string())
}
#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> { Ok(()) }

pub(crate) fn update_command() -> Result<String, String> {
    let manifest_location = env::var("JEDEN_UPDATE_MANIFEST").map_err(|_| "JEDEN_UPDATE_MANIFEST must point to a verified HTTPS or local manifest".to_string())?;
    let signing_key = env::var("JEDEN_UPDATE_SIGNING_KEY").map_err(|_| "JEDEN_UPDATE_SIGNING_KEY is required".to_string())?;
    if signing_key.len() < 32 { return Err("JEDEN_UPDATE_SIGNING_KEY must contain at least 32 bytes".into()); }
    let manifest_bytes = fetch_update_bytes(&manifest_location)?;
    let manifest: UpdateManifest = serde_json::from_slice(&manifest_bytes).map_err(|error| format!("invalid update manifest: {error}"))?;
    verify_manifest(&manifest, signing_key.as_bytes())?;
    let artifact_location = resolve_artifact(&manifest_location, &manifest.artifact);
    let artifact = fetch_update_bytes(&artifact_location)?;
    let actual = hex::encode(Sha256::digest(&artifact));
    if actual != manifest.sha256.to_ascii_lowercase() { return Err(format!("update checksum mismatch: expected {}, got {actual}", manifest.sha256)); }

    let target = match env::var_os("JEDEN_UPDATE_TARGET") { Some(path) => PathBuf::from(path), None => std::env::current_exe().map_err(|error| error.to_string())? };
    let parent = target.parent().ok_or("update target has no parent directory")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let stage = parent.join(format!(".jeden-update-{}-{}.stage", std::process::id(), manifest.version));
    let backup = parent.join(format!(".jeden-update-{}-{}.backup", std::process::id(), manifest.version));
    let result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new().create_new(true).write(true).open(&stage).map_err(|error| format!("stage update: {error}"))?;
        file.write_all(&artifact).map_err(|error| error.to_string())?; file.sync_all().map_err(|error| error.to_string())?; drop(file);
        make_executable(&stage)?;
        run_health(&stage, parent)?;
        if target.exists() { fs::rename(&target, &backup).map_err(|error| format!("backup current binary: {error}"))?; }
        if let Err(error) = fs::rename(&stage, &target) {
            if backup.exists() { let _ = fs::rename(&backup, &target); }
            return Err(format!("atomic update swap failed: {error}"));
        }
        if let Ok(directory) = fs::File::open(parent) { let _ = directory.sync_all(); }
        if let Err(error) = run_health(&target, parent) {
            let failed = parent.join(format!(".jeden-update-{}-failed", std::process::id()));
            let _ = fs::rename(&target, &failed);
            if backup.exists() { fs::rename(&backup, &target).map_err(|rollback| format!("{error}; rollback failed: {rollback}"))?; }
            let _ = fs::remove_file(failed);
            return Err(format!("{error}; previous binary restored"));
        }
        if backup.exists() { let _ = fs::remove_file(&backup); }
        Ok(())
    })();
    if result.is_err() { let _ = fs::remove_file(&stage); if backup.exists() && !target.exists() { let _ = fs::rename(&backup, &target); } }
    result?;
    Ok(format!("Jeden update {} installed and post-health verified\n", manifest.version))
}

pub(crate) fn resolve_model_route(cwd: &Path, model: &str) -> Result<(), String> {
    let runtime_config = load_config(cwd);
    let endpoint = env::var("BRAMA_URL").ok().or(runtime_config.model_router_url);
    let client = crate::control_plane::brama::BramaClient::configured(endpoint, env::var("BRAMA_TOKEN").ok());
    crate::control_plane::model_catalog(cwd, &client, false)
        .and_then(|catalog| catalog.resolve(model).map(|_| ()))
        .map_err(|error| error.to_string())
}

fn handle_model_slash(
    cwd: &Path,
    current_model: Option<&str>,
    args: &str,
) -> Result<String, String> {
    let next = args.trim();
    if next.is_empty() {
        let configured = current_model
            .map(str::to_string)
            .or_else(|| load_config(cwd).model)
            .or_else(|| env::var("JEDEN_MODEL").ok());
        return Ok(configured.map(|model| format!("Current model route: {model}.")).unwrap_or_else(|| "No model route selected; choose one advertised by Brama.".into()));
    }
    resolve_model_route(cwd, next)?;
    let path = config_path(cwd);
    let mut config = read_json::<Value>(&path);
    if !config.is_object() {
        config = json!({});
    }
    config
        .as_object_mut()
        .expect("object")
        .insert("model".into(), Value::String(next.to_string()));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(
        &path,
        serde_json::to_string_pretty(&config).map_err(|error| error.to_string())? + "\n",
    )
    .map_err(|error| error.to_string())?;
    Ok(format!("Model route set to {}.", next))
}

fn handle_settings_slash(cwd: &Path, args: &str) -> Result<String, String> {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "status" {
        return Ok(format_auth_status(cwd));
    }
    let mut json = false;
    let positionals = trimmed
        .split_whitespace()
        .filter_map(|part| {
            if part == "--json" {
                json = true;
                None
            } else {
                Some(part.to_string())
            }
        })
        .collect::<Vec<_>>();
    let Some(verb) = positionals.first().map(String::as_str) else {
        return Ok(format_auth_status(cwd));
    };
    if !matches!(verb, "list" | "path" | "get" | "set" | "reset") {
        return Err(
            "Usage: /settings [status|list|path|get <key>|set <key> <value>|reset <key>] [--json]"
                .into(),
        );
    }
    // max_tokens/max_steps are irrelevant to config_command; Default fills them
    // number-free.
    config_command(&Args {
        command: "config".into(),
        cwd: cwd.to_path_buf(),
        json,
        positionals,
        ..Default::default()
    })
}


pub(crate) fn handle_slash(cwd: &Path, input: &str, model: Option<&str>) -> Result<String, String> {
    let trimmed = input.trim();
    let mut parts = trimmed.split_whitespace();
    let command = parts.next().unwrap_or("");
    if command.eq_ignore_ascii_case("/update") {
        return update_command();
    }
    let session_root = session_root();
    let slash_context = slash::SlashContext {
        cwd,
        model,
        session_root: &session_root,
    };
    if let Some(result) = slash::handle_local(&slash_context, trimmed) {
        return result;
    }
    match command {
        "/help" | "/commands" => {
            let help = format_slash_help(cwd);
            Ok(help)
        }
        "/settings" => handle_settings_slash(cwd, parts.collect::<Vec<_>>().join(" ").as_str()),
        "/setup" | "/providers" => Ok(format_auth_status(cwd)),
        "/login" => start_login(cwd, parts.collect::<Vec<_>>().join(" ").as_str()),
        "/logout" => logout(cwd, parts.collect::<Vec<_>>().join(" ").as_str()),
        "/refresh" => refresh(parts.collect::<Vec<_>>().join(" ").as_str()),
        "/model" | "/models" | "/switch" => {
            handle_model_slash(cwd, model, parts.collect::<Vec<_>>().join(" ").as_str())
        }
        "/exit" | "/quit" => Ok("Exit is handled by the interactive input loop.".into()),
        _ => Err(format!("Unknown Rust slash command: {}", command)),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::ffi::{OsStr, OsString};
    use std::sync::{atomic::{AtomicU64, Ordering}, Mutex};

    static UPDATE_ENV: Mutex<()> = Mutex::new(());
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    const SIGNING_KEY: &str = "local-updater-fixture-signing-key-32-bytes";

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!("jeden-{label}-{}-{sequence}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path { &self.0 }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    struct UpdateEnvironment(Vec<(&'static str, Option<OsString>)>);

    impl UpdateEnvironment {
        fn set(manifest: &Path, target: &Path) -> Self {
            let values = [
                ("JEDEN_UPDATE_MANIFEST", manifest.as_os_str()),
                ("JEDEN_UPDATE_SIGNING_KEY", OsStr::new(SIGNING_KEY)),
                ("JEDEN_UPDATE_TARGET", target.as_os_str()),
            ];
            let previous = values.iter().map(|(key, _)| (*key, env::var_os(key))).collect();
            for (key, value) in values { env::set_var(key, value); }
            Self(previous)
        }
    }

    impl Drop for UpdateEnvironment {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..) {
                match value {
                    Some(value) => env::set_var(key, value),
                    None => env::remove_var(key),
                }
            }
        }
    }

    fn signed_manifest(root: &Path, artifact: &[u8], sha256: &str) -> PathBuf {
        let artifact_name = "candidate.sh";
        fs::write(root.join(artifact_name), artifact).unwrap();
        let version = "fixture-1";
        let payload = format!("{version}\n{artifact_name}\n{sha256}");
        let mut mac = Hmac::<Sha256>::new_from_slice(SIGNING_KEY.as_bytes()).unwrap();
        mac.update(payload.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());
        let manifest = json!({
            "schemaVersion": 1,
            "version": version,
            "artifact": artifact_name,
            "sha256": sha256,
            "signature": signature,
        });
        let path = root.join("manifest.json");
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        path
    }

    #[test]
    fn update_command_rejects_bad_checksum_without_replacing_existing_target() {
        let _env_lock = UPDATE_ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = TestDirectory::new("update-bad-checksum");
        let target = root.path().join("jeden");
        let original = b"#!/bin/sh\nexit 0\n";
        fs::write(&target, original).unwrap();
        let artifact = b"#!/bin/sh\nexit 9\n";
        let declared_sha = hex::encode(Sha256::digest(b"different artifact"));
        let manifest = signed_manifest(root.path(), artifact, &declared_sha);
        let _environment = UpdateEnvironment::set(&manifest, &target);

        let error = update_command().unwrap_err();

        assert!(error.contains("update checksum mismatch"), "unexpected error: {error}");
        assert_eq!(fs::read(&target).unwrap(), original);
    }

    #[test]
    fn update_command_restores_existing_target_when_installed_post_health_fails() {
        let _env_lock = UPDATE_ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = TestDirectory::new("update-post-health-rollback");
        let target = root.path().join("jeden");
        let original = b"#!/bin/sh\nexit 0\n";
        fs::write(&target, original).unwrap();
        let artifact = b"#!/bin/sh\ncase \"$0\" in\n  *.stage) exit 0 ;;\n  *) exit 23 ;;\nesac\n";
        let sha256 = hex::encode(Sha256::digest(artifact));
        let manifest = signed_manifest(root.path(), artifact, &sha256);
        let _environment = UpdateEnvironment::set(&manifest, &target);

        let error = update_command().unwrap_err();

        assert!(error.contains("health probe failed"), "unexpected error: {error}");
        assert!(error.contains("previous binary restored"), "unexpected error: {error}");
        assert_eq!(fs::read(&target).unwrap(), original);
    }
}
