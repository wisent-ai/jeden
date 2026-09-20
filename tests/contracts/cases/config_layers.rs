//! Which configuration file decides, when the superseded one still exists.
//!
//! `~/.jeden/config.json` is the file `~/.jeden/config.yml` replaced. A
//! detached session started in the home directory died on a model route
//! Brama no longer serves because that old file was still answering: the
//! CLI wrote the new route into the current file, and the run read the old
//! one, which in the home directory was also applied as the project layer
//! and therefore came last.

use crate::home::Home;
use serde_json::{json, Value};
use std::fs;

const LEGACY_ROUTE: &str = "kimi/kimi-for-coding";
const CURRENT_ROUTE: &str = "openrouter/openrouter/free";
const MODEL_KEY: &str = "model";

fn write_legacy(home: &Home, value: Value) -> std::path::PathBuf {
    let directory = home.root.join("home/.jeden");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("config.json");
    fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    path
}

fn set(home: &Home, key: &str, value: &str) {
    home.ok(&["config", "set", key, value]);
}

fn answered(home: &Home, key: &str) -> String {
    String::from_utf8(home.ok(&["config", "get", key]).stdout)
        .unwrap()
        .trim()
        .to_string()
}

fn answered_in(home: &Home, key: &str, cwd: &str) -> String {
    let mut argv = vec!["config", "get"];
    argv.push(key);
    argv.push("--cwd");
    argv.push(cwd);
    String::from_utf8(home.ok(&argv).stdout)
        .unwrap()
        .trim()
        .to_string()
}
#[test]
fn the_value_the_cli_writes_is_the_value_every_read_answers_with() {
    let home = Home::new("config-layers-write");
    let legacy = write_legacy(
        &home,
        json!({"model": LEGACY_ROUTE, "sshHosts": {"mini": {"target": "charless-mac-mini"}}}),
    );
    assert_eq!(answered(&home, MODEL_KEY), LEGACY_ROUTE);
    set(&home, MODEL_KEY, CURRENT_ROUTE);
    assert_eq!(answered(&home, MODEL_KEY), CURRENT_ROUTE);
    assert!(
        !legacy.exists(),
        "everything the superseded file held moved, so it is not kept beside its replacement"
    );
    let current: Value =
        serde_json::from_slice(&fs::read(home.root.join("home/.jeden/config.yml")).unwrap())
            .unwrap();
    assert_eq!(current[MODEL_KEY], CURRENT_ROUTE, "{current}");
    assert_eq!(
        current["sshHosts"]["mini"]["target"], "charless-mac-mini",
        "what the old file held is carried over, not dropped: {current}"
    );
}

#[test]
fn a_run_in_the_home_directory_reads_the_current_file_not_the_superseded_one() {
    let home = Home::new("config-layers-home");
    let directory = home.root.join("home");
    write_legacy(&home, json!({"model": LEGACY_ROUTE}));
    fs::write(
        directory.join(".jeden/config.yml"),
        serde_json::to_vec_pretty(&json!({"model": CURRENT_ROUTE})).unwrap(),
    )
    .unwrap();
    assert_eq!(
        answered_in(&home, MODEL_KEY, directory.to_str().unwrap()),
        CURRENT_ROUTE,
        "the home directory is one layer, not the superseded file applied twice"
    );
}

#[test]
fn a_superseded_file_left_with_nothing_of_its_own_is_removed() {
    let home = Home::new("config-layers-retire");
    let legacy = write_legacy(&home, json!({"model": LEGACY_ROUTE}));
    set(&home, MODEL_KEY, CURRENT_ROUTE);
    assert!(
        !legacy.exists(),
        "a file whose every key moved is not kept beside its replacement"
    );
    assert_eq!(answered(&home, MODEL_KEY), CURRENT_ROUTE);
}
