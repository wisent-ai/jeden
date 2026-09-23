//! What `jeden context` promises a caller: the exact section to read, an
//! honest state for every source, and a refusal that names the mistake.
//!
//! Every case drives the real binary against a real corpus on this
//! filesystem. Nothing is mocked, and no source is replaced by a stub: the
//! ground-truth cases measure what an unconfigured and then an unreachable
//! endpoint actually report.

use crate::fixture::Workspace;
use serde_json::Value;

/// The line ranges the seeded corpus really has. They are the contract under
/// test: a recommendation must name a section, not a file.
const LEASE_SECTION: &str = "notes/fleet.md:5-9";
const OFFICE_SECTION: &str = "notes/fleet.md:10-12";
/// The seeded source file is six lines, so its first chunk is the whole file.
const CODE_CHUNK: &str = "src/lease.rs:1-6";
const SEEDED_SECTIONS: u64 = 3;

#[test]
fn a_recommendation_names_the_section_to_read() {
    let workspace = Workspace::new("recommend");
    workspace.configure(serde_json::json!({"sources": "files", "roots": "."}));
    let run = workspace.run(&["context", "recommend", "lease renewal", "--json"]);
    assert!(
        run.success,
        "recommend failed: {}\n{}",
        run.stdout, run.stderr
    );
    let advice = run.json();
    let first = advice["recommendations"]
        .as_array()
        .and_then(|hits| hits.first().cloned())
        .expect("the seeded corpus produces at least one recommendation");
    assert_eq!(first["source"], "files");
    assert_eq!(
        first["locator"], LEASE_SECTION,
        "the locator must name the lease section's own line range, not the file: {}",
        first["locator"]
    );
    assert_eq!(first["title"], "Lease renewal");
    assert!(
        first["snippet"]
            .as_str()
            .unwrap_or_default()
            .contains("stado lease renew"),
        "the snippet must carry the answering line: {}",
        first["snippet"]
    );
    let matched = strings(&first["matched"]);
    assert!(
        matched.contains(&"lease".to_string()) && matched.contains(&"renewal".to_string()),
        "both query terms matched the section, so both must be reported: {matched:?}"
    );
    let files = source(&advice, "files");
    assert_eq!(files["available"], true);
    assert!(
        files["considered"].as_u64().unwrap_or_default() >= SEEDED_SECTIONS,
        "the seeded file holds three sections: {}",
        files["considered"]
    );
}

#[test]
fn source_code_is_part_of_the_corpus() {
    let workspace = Workspace::new("code");
    workspace.configure(serde_json::json!({"sources": "files", "roots": "."}));
    let advice = workspace
        .run(&["context", "recommend", "renew_lease deadline", "--json"])
        .json();
    let code = advice["recommendations"]
        .as_array()
        .and_then(|hits| {
            hits.iter()
                .find(|hit| {
                    hit["locator"]
                        .as_str()
                        .unwrap_or_default()
                        .starts_with("src/lease.rs:")
                })
                .cloned()
        })
        .unwrap_or_else(|| {
            panic!(
                "the seeded source file must be recommended: {}",
                advice["recommendations"]
            )
        });
    assert_eq!(code["source"], "files");
    assert_eq!(
        code["locator"], CODE_CHUNK,
        "a code chunk names its own line window: {}",
        code["locator"]
    );
    assert!(
        code["snippet"]
            .as_str()
            .unwrap_or_default()
            .contains("fn renew_lease(target: &str, deadline: Instant)"),
        "the snippet must carry the matching declaration: {}",
        code["snippet"]
    );
}

#[test]
fn a_binary_file_is_not_recommended() {
    let workspace = Workspace::new("binary");
    workspace.configure(serde_json::json!({"sources": "files", "roots": "."}));
    let advice = workspace
        .run(&["context", "recommend", "lease renewal", "--json"])
        .json();
    let locators: Vec<String> = advice["recommendations"]
        .as_array()
        .map(|hits| {
            hits.iter()
                .filter_map(|hit| hit["locator"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !locators.iter().any(|locator| locator.contains("lease.bin")),
        "a file with a zero byte is not text and must not be recommended: {locators:?}"
    );
}

#[test]
fn an_answer_off_the_query_is_not_recommended() {
    let workspace = Workspace::new("off-query");
    workspace.configure(serde_json::json!({"sources": "files", "roots": "."}));
    let advice = workspace
        .run(&["context", "recommend", "lease renewal", "--json"])
        .json();
    let locators: Vec<String> = advice["recommendations"]
        .as_array()
        .map(|hits| {
            hits.iter()
                .filter_map(|hit| hit["locator"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !locators.iter().any(|locator| locator == OFFICE_SECTION),
        "the office-colour section shares no query term and must not be recommended: {locators:?}"
    );
}

#[test]
fn every_source_reports_its_own_state() {
    let workspace = Workspace::new("sources");
    workspace.configure(serde_json::json!({"sources": "files", "roots": "."}));
    let report = workspace.run(&["context", "sources", "--json"]).json();
    let rows = report["sources"]
        .as_array()
        .expect("the report lists every source");
    assert_eq!(
        rows.len(),
        declared_sources(),
        "every declared source owes a row: {rows:?}"
    );
    for row in rows {
        assert!(
            !row["detail"].as_str().unwrap_or_default().trim().is_empty(),
            "every source owes an observed state: {row}"
        );
    }
    assert_eq!(source(&report, "files")["available"], true);
    let ground_truth = source(&report, "ground-truth");
    assert_eq!(ground_truth["available"], false);
    assert_eq!(
        ground_truth["detail"],
        "no endpoint: set context.advisor.groundTruthUrl or WISENT_GROUND_TRUTH_API"
    );
    assert_eq!(ground_truth["origin"], "unset");
}

#[test]
fn a_configured_but_unreachable_index_says_so() {
    let workspace = Workspace::new("unreachable-index");
    // Port 1 accepts nothing on this machine, so the probe measures a real
    // refused connection rather than a simulated one.
    workspace.configure(serde_json::json!({
        "sources": "ground-truth",
        "groundTruthUrl": "http://127.0.0.1:1",
    }));
    let report = workspace.run(&["context", "sources", "--json"]).json();
    let ground_truth = source(&report, "ground-truth");
    assert_eq!(ground_truth["available"], false);
    let detail = ground_truth["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("http://127.0.0.1:1/health") && detail.contains("unreachable"),
        "the diagnostic must name the endpoint it could not reach: {detail}"
    );
    assert_eq!(
        ground_truth["origin"], "config context.advisor.groundTruthUrl",
        "the report must say where the endpoint came from"
    );
    let advice = workspace
        .run(&["context", "recommend", "lease renewal", "--json"])
        .json();
    assert!(
        advice["recommendations"]
            .as_array()
            .map(|hits| hits.is_empty())
            .unwrap_or_default(),
        "an unreachable index answers nothing: {}",
        advice["recommendations"]
    );
    assert_eq!(source(&advice, "ground-truth")["available"], false);
}

#[test]
fn an_unknown_source_is_refused_by_name() {
    let workspace = Workspace::new("unknown-source");
    let run = workspace.run(&["context", "recommend", "lease", "--source", "nonsense"]);
    assert!(
        !run.success,
        "an unknown source must refuse: {}",
        run.stdout
    );
    assert!(
        run.stderr.contains(
            "unknown source(s): nonsense. Known sources: files, ground-truth, memory, transcripts"
        ),
        "the refusal must name the mistake and the choices: {}",
        run.stderr
    );
}

#[test]
fn a_recommendation_without_a_task_is_refused() {
    let workspace = Workspace::new("no-task");
    let run = workspace.run(&["context", "recommend"]);
    assert!(!run.success, "an empty task must refuse: {}", run.stdout);
    assert!(
        run.stderr.contains("context recommend requires a task")
            && run.stderr.contains("jeden context sources"),
        "the refusal must name the requirement and show the usage: {}",
        run.stderr
    );
}

/// How many sources the binary declares, read from the refusal it produces
/// for an unknown one, so this file holds no second copy of that list.
fn declared_sources() -> usize {
    let workspace = Workspace::new("declared-sources");
    let refusal = workspace
        .run(&["context", "recommend", "x", "--source", "nonsense"])
        .stderr;
    refusal
        .split("Known sources: ")
        .nth(1)
        .map(|tail| tail.trim().split(", ").count())
        .expect("the refusal names the known sources")
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn source(document: &Value, name: &str) -> Value {
    document["sources"]
        .as_array()
        .expect("the document lists sources")
        .iter()
        .find(|row| row["source"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("no source named {name} in {document}"))
}
