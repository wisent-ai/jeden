use super::{action_or_text, Action};
use serde_json::{json, Value};

#[test]
fn a_fenced_inspection_result_preserves_the_same_data_as_a_bare_result() {
    let result = json!({
        "tasks": [{"text": "Read the recorded image", "criteria": ["The answer describes the observed image"], "kind": "work"}],
        "cancellations": []
    });
    let fenced = format!("```json\n{result}\n```");
    let Action::Final { text, .. } = action_or_text(&fenced, true).unwrap() else {
        panic!("an inspection result was treated as an executable action");
    };
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), result);
    assert!(action_or_text(&fenced, false).is_err());
}

#[test]
fn inspection_still_dispatches_actions_and_refuses_broken_json() {
    let tool = r#"```json
{"action":"tool","tool":"read_file","input":{"path":"observation.txt"}}
```"#;
    let Action::Tool { tool, input } = action_or_text(tool, true).unwrap() else {
        panic!("an inspection tool call was treated as a final result");
    };
    assert_eq!(tool, "read_file");
    assert_eq!(input, json!({"path": "observation.txt"}));
    assert!(action_or_text("```json\n{\"tasks\": invalid}\n```", true).is_err());
    assert!(action_or_text("```json\n{\"tasks\": [", true).is_err());
}
