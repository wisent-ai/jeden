//! The operator's own contracts: written, read and reset through the real
//! CLI, served over the real RPC surface, and installed into another
//! harness's rules without disturbing what is already there.

use crate::home::Home;
use serde_json::json;
use std::fs;

#[test]
fn operator_contracts_are_written_read_and_reset_through_the_cli() {
    let home = Home::new("settings-cli");
    home.ok(&[
        "config",
        "set",
        "contracts.communication",
        "Answer in Polish using three plain sentences.",
    ]);
    home.ok(&[
        "config",
        "set",
        "contracts.functionality",
        "Finish the requested behavior before answering.",
    ]);
    assert_eq!(
        home.config()["contracts"]["communication"],
        "Answer in Polish using three plain sentences."
    );
    assert_eq!(
        home.config()["contracts"]["functionality"],
        "Finish the requested behavior before answering."
    );
    let read = home.ok(&["config", "get", "contracts.functionality"]);
    assert_eq!(
        String::from_utf8(read.stdout).unwrap().trim(),
        home.config()["contracts"]["functionality"]
    );
    home.ok(&["config", "reset", "contracts.functionality"]);
    assert_eq!(home.config()["contracts"]["functionality"], "");
    let refused = home.run(&["config", "get", "contracts.style"]);
    assert!(!refused.status.success());
    assert_eq!(
        String::from_utf8(refused.stderr).unwrap().trim(),
        "Error: unknown config key: contracts.style"
    );
    home.passed();
}

#[test]
fn operator_contracts_are_served_through_rpc_for_jeden_desktop() {
    let home = Home::new("settings-rpc");
    let frames = home.rpc(&[
        json!({"id":"set", "method":"config/contracts/set", "params":{"communication":"Short sentences.", "functionality":"Finish the task in full."}}),
        json!({"id":"refuse", "method":"config/contracts/set", "params":{"communication":"Only one field."}}),
    ]);
    assert_eq!(
        home.config()["contracts"]["communication"],
        "Short sentences."
    );
    assert_eq!(
        home.config()["contracts"]["functionality"],
        "Finish the task in full."
    );
    let refused = frames.iter().find(|frame| frame["id"] == "refuse").unwrap();
    assert_eq!(refused["error"]["code"], "invalid_params");
    assert_eq!(
        refused["error"]["message"],
        "functionality must be a string"
    );
    assert_eq!(
        String::from_utf8(
            home.ok(&["config", "get", "contracts.functionality"])
                .stdout
        )
        .unwrap()
        .trim(),
        "Finish the task in full."
    );
    home.passed();
}

#[test]
fn contracts_install_preserves_surrounding_rules_and_replaces_stale_contract() {
    let home = Home::new("install");
    let path = home.root.join("consumer.txt");
    fs::write(&path, "Existing rule one.\n").unwrap();
    let file = path.to_str().unwrap();
    assert!(!home
        .run(&["contracts", "status", "--file", file])
        .status
        .success());
    home.ok(&["contracts", "install", "--file", file]);
    let first = fs::read_to_string(&path).unwrap();
    let rendered = String::from_utf8(home.ok(&["contracts", "render"]).stdout).unwrap();
    assert!(first.starts_with("Existing rule one.\n"));
    assert!(first.contains(rendered.trim()));
    home.ok(&["contracts", "install", "--file", file]);
    assert_eq!(fs::read_to_string(&path).unwrap(), first);
    home.ok(&[
        "config",
        "set",
        "contracts.communication",
        "Answer in three sentences.",
    ]);
    assert!(!home
        .run(&["contracts", "status", "--file", file])
        .status
        .success());
    home.ok(&["contracts", "install", "--file", file]);
    let current = fs::read_to_string(&path).unwrap();
    assert!(current.starts_with("Existing rule one.\n"));
    assert!(current.contains("Answer in three sentences."));
    assert!(!current.contains(rendered.trim()));
    home.ok(&["contracts", "status", "--file", file]);
    let refused = home.run(&["contracts", "install", "--file"]);
    assert!(!refused.status.success());
    assert_eq!(
        String::from_utf8(refused.stderr).unwrap().trim(),
        "Error: --file requires a path"
    );
    home.passed();
}
