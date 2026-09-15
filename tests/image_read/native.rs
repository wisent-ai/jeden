use super::prepare_outbound_messages;
use crate::model_router::ModelAttachment;
use crate::tool_runtime::runtime_ops::{ArtifactSink, CancellationToken, OperationContext};
use crate::tool_runtime::{execute, format_tool_result, ToolRuntime};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{fs, path::PathBuf, sync::Arc};

struct Workspace(PathBuf);

impl Workspace {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace = Self(
            root.join(".wisent-output/image-read-tests")
                .join(uuid::Uuid::new_v4().to_string()),
        );
        fs::create_dir_all(&workspace.0).unwrap();
        fs::copy(root.join("web/og-image.png"), workspace.0.join("image.png")).unwrap();
        fs::write(
            workspace.0.join("caption.txt"),
            "Supporting caption from a real text file",
        )
        .unwrap();
        workspace
    }

    fn runtime(&self) -> ToolRuntime<'_> {
        ToolRuntime {
            cwd: &self.0,
            artifact_dir: None,
            operation: OperationContext::new(
                CancellationToken::new(),
                ArtifactSink::new(self.0.join("artifacts")),
            ),
            allow_write: false,
            allow_command: false,
            interactive: false,
            ask_user: None,
        }
    }

    fn image(&self, max_bytes: usize) -> Value {
        execute(
            &self.runtime(),
            "read_image",
            &json!({"path": "image.png", "maxBytes": max_bytes}),
        )
        .unwrap()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn a_real_image_read_reaches_the_provider_as_pixels_not_base64_prose() {
    let workspace = Workspace::new();
    let result = workspace.image(512 * 1024);
    let history = vec![json!({"role": "user", "content": format_tool_result(&result)})];
    let outbound = prepare_outbound_messages(&workspace.0, &history, &[]).unwrap();
    let parts = outbound[0]["content"]
        .as_array()
        .expect("provider needs multimodal content");
    let image = parts
        .iter()
        .find_map(|part| part.pointer("/image_url/url").and_then(Value::as_str))
        .unwrap();
    let encoded = image.strip_prefix("data:image/png;base64,").unwrap();
    assert_eq!(
        STANDARD.decode(encoded).unwrap(),
        fs::read(workspace.0.join("image.png")).unwrap()
    );
    let text: Value = serde_json::from_str(parts[0]["text"].as_str().unwrap()).unwrap();
    assert!(
        text["result"].get("base64").is_none(),
        "encoded pixels must not also consume the text context"
    );
}

#[test]
fn batch_image_reads_keep_supporting_text_and_input_attachments() {
    let workspace = Workspace::new();
    let caption = execute(
        &workspace.runtime(),
        "read_file",
        &json!({"path": "caption.txt"}),
    )
    .unwrap();
    let result = json!([workspace.image(512 * 1024), caption]);
    let history = vec![json!({"role": "user", "content": format_tool_result(&result)})];
    let input =
        ModelAttachment::text(Arc::from(b"The caller's attached instruction".as_slice())).unwrap();
    let outbound = prepare_outbound_messages(&workspace.0, &history, &[input]).unwrap();
    let parts = outbound[0]["content"].as_array().unwrap();
    let body: Value = serde_json::from_str(parts[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        body["result"][1]["content"],
        "Supporting caption from a real text file"
    );
    assert!(parts
        .iter()
        .any(|part| part["text"] == "The caller's attached instruction"));
    assert!(
        parts
            .iter()
            .any(|part| part.pointer("/image_url/url").is_some()),
        "batch images must reach the visual input"
    );
}

#[test]
fn a_truncated_image_read_is_refused_before_a_model_request() {
    let workspace = Workspace::new();
    let result = workspace.image(16);
    let history = vec![json!({"role": "user", "content": format_tool_result(&result)})];
    let error = prepare_outbound_messages(&workspace.0, &history, &[]).unwrap_err();
    assert!(
        error.contains("image.png"),
        "the refusal must identify the incomplete image: {error}"
    );
}
