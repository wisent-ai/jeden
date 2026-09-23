//! The release build boundary, through the real `jeden-tools release`
//! commands: private Git crates exported from Cargo.lock and consumed offline,
//! native staging, and every refusal on the way.
//!
//! Both journeys need what only their host has, so neither runs in a plain
//! `cargo test`:
//!
//! - `export_and_stage_offline` exports the real locked private crates, which
//!   needs read access to them; run it on a development host with
//!   `cargo test --target-dir target/qualification --test release -- --ignored export_and_stage_offline`.
//! - `stage_from_declared_input` consumes a release worker's declared
//!   `private-cargo-sources` input; the release recipe runs it through
//!   `jeden-tools release cargo test --target-dir target/qualification`.
//!
//! Each run keeps its source revision, commands, outputs, exit codes and the
//! staged executable's hash in `.wisent-output/release-tests/<id>/report.json`.

mod journey;

use journey::{env_map, root, sha256, Journey, INPUT_ENV, SYSTEM_PATH};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[test]
#[ignore = "consumes the release worker's declared private-cargo-sources input"]
fn stage_from_declared_input() {
    let mut journey = Journey::start("native staging from declared input");
    let mut env = env_map();
    let declared = env.get(INPUT_ENV).is_some_and(|value| !value.is_empty());
    assert!(declared, "{INPUT_ENV} is not declared");
    env.insert("CARGO_NET_OFFLINE".into(), "true".into());
    let lock = sha256(&root().join("Cargo.lock"));
    journey.stage_native(&env, "jeden");
    assert_eq!(sha256(&root().join("Cargo.lock")), lock);
    journey.passed();
}

fn private_packages(packages: &Value) -> BTreeMap<String, Value> {
    packages
        .as_array()
        .expect("package list")
        .iter()
        .filter(|package| {
            package["source"]
                .as_str()
                .is_some_and(|source| source.starts_with("git+"))
        })
        .map(|package| {
            (
                package["name"].as_str().expect("name").to_string(),
                package.clone(),
            )
        })
        .collect()
}

#[test]
#[ignore = "exports the real locked private crates, which needs read access to them"]
fn export_and_stage_offline() {
    let mut journey = Journey::start("private source export and native helper staging");
    let mut env = env_map();
    env.remove(INPUT_ENV);
    env.remove("WISENT_OUTPUT_DIR");
    env.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
    env.insert("GCM_INTERACTIVE".into(), "never".into());
    let lock = sha256(&root().join("Cargo.lock"));
    let build = ["release", "cargo", "build", "--locked", "--offline"];
    let refused = journey.tool(&build, &env, 1).1;
    assert!(
        refused.contains(&format!("{INPUT_ENV} is required")),
        "{refused}"
    );
    let archive = journey.report.join("private-cargo-sources.tar.gz");
    let archive_text = archive.display().to_string();
    let exported = journey
        .tool(&["release", "export", &archive_text], &env, 0)
        .0;
    let receipt: Value = serde_json::from_str(&exported).expect("export prints its receipt");
    assert_eq!(receipt["sha256"], sha256(&archive));
    let inputs = journey.report.join("private sources");
    let compressed = fs::File::open(&archive).expect("open archive");
    tar::Archive::new(flate2::read::GzDecoder::new(compressed))
        .unpack(&inputs)
        .expect("unpack the exported input");
    // Public registry paths stay as they are so real compiler output is
    // reused; the package locations below prove the private crates came from
    // this archive rather than an ambient Git checkout.
    env.retain(|key, _| {
        !(key.starts_with("GIT_") || key.starts_with("GITHUB_") || key.starts_with("GH_"))
            && !key.starts_with("CARGO_REGISTRIES_")
    });
    env.insert("CARGO_NET_OFFLINE".into(), "true".into());
    env.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
    env.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
    env.insert("GIT_CONFIG_GLOBAL".into(), "/dev/null".into());
    env.insert(INPUT_ENV.into(), inputs.display().to_string());
    let mut worker = env.clone();
    worker.insert("PATH".into(), SYSTEM_PATH.into());
    let metadata = [
        "release",
        "cargo",
        "metadata",
        "--locked",
        "--offline",
        "--format-version=1",
    ];
    let listed: Value =
        serde_json::from_str(&journey.tool(&metadata, &worker, 0).0).expect("metadata");
    let actual = private_packages(&listed["packages"]);
    let declared = private_packages(&receipt["provenance"]["packages"]);
    assert_eq!(
        actual.keys().collect::<Vec<_>>(),
        declared.keys().collect::<Vec<_>>()
    );
    for (name, package) in &actual {
        assert_eq!(package["source"], declared[name]["source"]);
        let manifest = PathBuf::from(package["manifest_path"].as_str().expect("manifest path"));
        assert!(
            manifest.starts_with(inputs.join("sources")),
            "{name}: {}",
            manifest.display()
        );
    }
    journey.stage_native(&env, "jeden-sandbox-helper");
    assert_eq!(sha256(&root().join("Cargo.lock")), lock);
    let provenance_path = inputs.join("provenance.json");
    let original = fs::read(&provenance_path).expect("read provenance");
    let mut provenance: Value = serde_json::from_slice(&original).expect("provenance JSON");
    // A lockfile that changed anywhere but its private packages - a jeden
    // version bump rewrites jeden's own entry - still matches the input.
    provenance["cargo_lock_sha256"] = json!(format!("{:x}", Sha256::digest(b"different lockfile")));
    fs::write(&provenance_path, provenance.to_string()).expect("write altered provenance");
    journey.tool(&metadata, &worker, 0);
    provenance["packages"][0]["version"] = json!("0.0.0-not-locked");
    fs::write(&provenance_path, provenance.to_string()).expect("write altered provenance");
    let mismatch = journey.tool(&build, &env, 1).1;
    assert!(
        mismatch.contains("private Cargo sources do not match Cargo.lock"),
        "{mismatch}"
    );
    fs::write(&provenance_path, original).expect("restore provenance");
    let first = &receipt["provenance"]["packages"][0];
    let missing = inputs.join("sources").join(format!(
        "{}-{}",
        first["name"].as_str().expect("name"),
        first["version"].as_str().expect("version")
    ));
    fs::remove_dir_all(&missing).expect("remove one exported package");
    let helper = [
        "release",
        "cargo",
        "build",
        "--locked",
        "--offline",
        "--release",
        "--bin",
        "jeden-sandbox-helper",
    ];
    journey.tool(&helper, &env, 101);
    assert_eq!(sha256(&root().join("Cargo.lock")), lock);
    journey.passed();
}
