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

/// A setting written into the file can leave it again, and the pinnable
/// languages `config set` accepts are the ones `config list` advertises —
/// the schema used to carry its own copy of all sixty-five codes.
#[test]
fn a_written_setting_can_be_removed_and_the_language_set_has_one_source() {
    let home = Home::new("settings-unset");
    home.ok(&["config", "set", "ui.language", "pl"]);
    assert_eq!(home.config()["ui"]["language"], "pl");

    let removed = home.ok(&["config", "unset", "ui.language", "--json"]);
    let removed: serde_json::Value = serde_json::from_slice(&removed.stdout).unwrap();
    assert_eq!(removed["removed"], json!(true));
    assert_eq!(removed["value"], "auto");
    // The object the setting lived in goes with it: the file says nothing
    // about the language again, which `config reset` cannot do.
    assert_eq!(home.config().get("ui"), None);
    let again = home.ok(&["config", "unset", "ui.language", "--json"]);
    let again: serde_json::Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(again["removed"], json!(false));

    let listed = home.ok(&["config", "list", "--json"]);
    let listed: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    let advertised: Vec<String> = listed["ui.language"]["enum"]
        .as_array()
        .expect("the listing advertises the choices")
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect();
    assert!(advertised.len() > 2, "{advertised:?}");

    let refused = home.run(&["config", "set", "ui.language", "nosuchlanguage"]);
    assert!(!refused.status.success());
    let sentence = String::from_utf8(refused.stderr).unwrap();
    // Every advertised choice is accepted, and the refusal names the same
    // set: one declaration behind both, not a schema copy beside a parser.
    for choice in &advertised {
        assert!(sentence.contains(choice.as_str()), "{choice}: {sentence}");
        home.ok(&["config", "set", "ui.language", choice]);
    }
    home.ok(&["config", "unset", "ui.language"]);
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
