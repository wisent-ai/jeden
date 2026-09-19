use crate::home::Home;
use std::fs;

#[test]
fn defect_reopens_completed_work_and_requires_a_real_repair() {
    let home = Home::new("defect-repair");
    home.ok(&["run", "Create alpha.txt containing exactly ALPHA. Finish only after verifying its contents. Do not run commands, open applications, contact anyone or send notifications.", "--allow-write", "--max-steps", "24", "--json"]);
    let file = home.workspace().join("alpha.txt");
    assert_eq!(fs::read(&file).unwrap(), b"ALPHA");
    let completed = home.state();
    let original = completed["tasks"].as_array().unwrap().iter().find(|task| task["kind"] == "work").unwrap();
    assert_eq!(original["status"], "done");
    let id = original["id"].as_str().unwrap();
    fs::write(&file, b"BROKEN").unwrap();
    home.value(&["todo", "defect", id, "--revision", &completed["revision"].to_string(), "--reason", "alpha.txt contains BROKEN instead of ALPHA. Repair and verify it without running commands or opening applications.", "--json"]);
    let reopened = home.state();
    let task = reopened["tasks"].as_array().unwrap().iter().find(|task| task["id"] == id).unwrap();
    assert_eq!(task["status"], "pending");
    assert!(task["verification"].is_null());
    assert_eq!(task["criteria"], original["criteria"]);
    assert_eq!(fs::read(&file).unwrap(), b"BROKEN");
    home.ok(&["todo", "continue", "--allow-write", "--max-steps", "24", "--json"]);
    assert_eq!(fs::read(&file).unwrap(), b"ALPHA");
    for task in home.state()["tasks"].as_array().unwrap() {
        assert_eq!(task["status"], "done");
        assert_ne!(task["verification"], original["verification"]);
    }
    home.passed();
}

#[test]
fn defect_preserves_pause_and_refuses_cancelled_work() {
    let home = Home::new("defect-refusals");
    let added = home.value(&["todo", "add", "Create alpha.txt containing ALPHA", "--json"]);
    let id = added["requests"][0]["id"].as_str().unwrap();
    home.control(id, "pause");
    home.control(id, "defect");
    let paused = home.state();
    assert_eq!(paused["requests"][0]["paused"], true);
    assert_eq!(paused["tasks"][0]["status"], "paused");
    assert_eq!(paused["tasks"][0]["defectOf"], id);
    for (target, revision, reason) in [(id, paused["revision"].to_string(), "Isolated product journey"), (id, added["revision"].to_string(), "Another defect"), ("missing", paused["revision"].to_string(), "Unknown target"), (id, paused["revision"].to_string(), " ")] {
        let result = home.run(&["todo", "defect", target, "--revision", &revision, "--reason", reason]);
        assert!(!result.status.success());
        assert_eq!(home.state(), paused);
    }
    home.control(id, "cancel");
    let cancelled = home.state();
    let result = home.run(&["todo", "defect", id, "--revision", &cancelled["revision"].to_string(), "--reason", "Cannot revive cancelled work"]);
    assert!(!result.status.success());
    assert_eq!(home.state(), cancelled);
    home.passed();
}
