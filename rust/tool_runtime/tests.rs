use super::*;
use crate::tool_runtime::runtime_ops::{ArtifactSink, CancellationToken, OperationContext};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "jeden-tool-runtime-{name}-{}-{}",
            std::process::id(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(
    root: &Path,
    artifact_dir: Option<&Path>,
    allow_write: bool,
    cancellation: CancellationToken,
    tool: &str,
    input: Value,
) -> Result<Value, String> {
    let artifacts = artifact_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.join("operation-artifacts"));
    let runtime = ToolRuntime {
        cwd: root,
        artifact_dir,
        operation: OperationContext::new(cancellation, ArtifactSink::new(artifacts)),
        allow_write,
        allow_command: false,
        interactive: false,
        ask_user: None,
    };
    execute(&runtime, tool, &input)
}

#[test]
fn ranged_read_of_large_file_returns_only_the_requested_line() {
    let root = TempDir::new("large-range");
    let path = root.path().join("large.txt");
    let mut writer = BufWriter::new(File::create(&path).unwrap());
    for line in 1..=10_000 {
        writeln!(writer, "line-{line:05}-abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ").unwrap();
    }
    writer.flush().unwrap();
    assert!(fs::metadata(&path).unwrap().len() > 512_000);

    let result = run(
        root.path(),
        None,
        false,
        CancellationToken::new(),
        "read_file",
        json!({"path":"large.txt", "selector":"5000:5000"}),
    )
    .unwrap();

    assert_eq!(result["content"], "line-05000-abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ");
    assert_eq!(result["lines"], json!([{
        "line": 5000,
        "text": "line-05000-abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
    }]));
    assert_eq!(result["scannedLines"], 10_000);
    assert_eq!(result["truncated"], false);
    assert!(result.to_string().len() < 2_000, "ranged response unexpectedly embedded the file");
}

#[test]
fn recursive_search_honors_nested_gitignore_and_can_explicitly_override_it() {
    let root = TempDir::new("gitignore");
    fs::create_dir_all(root.path().join("nested/deeper")).unwrap();
    fs::create_dir(root.path().join(".git")).unwrap();
    fs::write(root.path().join("nested/.gitignore"), "*.secret\n").unwrap();
    fs::write(root.path().join("nested/visible.txt"), "shared needle\n").unwrap();
    fs::write(root.path().join("nested/deeper/ignored.secret"), "shared needle\n").unwrap();

    let honored = run(
        root.path(),
        None,
        false,
        CancellationToken::new(),
        "search_files",
        json!({"path":".", "query":"shared needle", "gitignore":true}),
    )
    .unwrap();
    assert_eq!(honored["matches"], json!([{
        "path": "nested/visible.txt",
        "line": 1,
        "text": "shared needle"
    }]));

    let overridden = run(
        root.path(),
        None,
        false,
        CancellationToken::new(),
        "search_files",
        json!({"path":".", "query":"shared needle", "gitignore":false}),
    )
    .unwrap();
    assert_eq!(overridden["matches"], json!([
        {"path":"nested/deeper/ignored.secret", "line":1, "text":"shared needle"},
        {"path":"nested/visible.txt", "line":1, "text":"shared needle"}
    ]));
}

#[test]
fn pre_cancelled_recursive_search_stops_before_traversal() {
    let root = TempDir::new("cancelled-search");
    fs::write(root.path().join("would-match.txt"), "needle\n").unwrap();
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let error = run(
        root.path(),
        None,
        false,
        cancellation,
        "search_files",
        json!({"path":".", "query":"needle"}),
    )
    .unwrap_err();

    assert_eq!(error, "search cancelled");
}

#[test]
fn glob_returns_recursive_directories_in_stable_path_order() {
    let root = TempDir::new("sorted-directories");
    fs::create_dir_all(root.path().join("zeta/inner")).unwrap();
    fs::create_dir_all(root.path().join("alpha/deep")).unwrap();

    let result = run(
        root.path(),
        None,
        false,
        CancellationToken::new(),
        "glob_paths",
        json!({"path":".", "patterns":"**"}),
    )
    .unwrap();

    assert_eq!(result["matches"], json!([
        {"path":"alpha", "type":"directory"},
        {"path":"alpha/deep", "type":"directory"},
        {"path":"zeta", "type":"directory"},
        {"path":"zeta/inner", "type":"directory"}
    ]));
}

#[test]
fn notebook_text_roundtrip_preserves_cell_boundaries_and_updated_content() {
    let root = TempDir::new("notebook-roundtrip");
    let initial = "# %% [markdown] cell:1\n# Heading\n\n# %% [code] cell:2\nprint('before')";
    let created = run(
        root.path(),
        None,
        true,
        CancellationToken::new(),
        "write",
        json!({"path":"work.ipynb", "content":initial}),
    )
    .unwrap();

    let first_read = run(
        root.path(),
        None,
        false,
        CancellationToken::new(),
        "read_document",
        json!({"path":"work.ipynb"}),
    )
    .unwrap();
    assert_eq!(first_read["text"], initial);

    let updated = first_read["text"].as_str().unwrap().replace("before", "after");
    run(
        root.path(),
        None,
        true,
        CancellationToken::new(),
        "write",
        json!({
            "path":"work.ipynb",
            "content":updated,
            "expectedSha256":created["sha256"]
        }),
    )
    .unwrap();

    let second_read = run(
        root.path(),
        None,
        false,
        CancellationToken::new(),
        "read_document",
        json!({"path":"work.ipynb"}),
    )
    .unwrap();
    assert_eq!(second_read["text"], "# %% [markdown] cell:1\n# Heading\n\n# %% [code] cell:2\nprint('after')");

    let notebook: Value = serde_json::from_slice(&fs::read(root.path().join("work.ipynb")).unwrap()).unwrap();
    assert_eq!(notebook["cells"][0]["cell_type"], "markdown");
    assert_eq!(notebook["cells"][1]["cell_type"], "code");
    assert_eq!(notebook["cells"][1]["source"], json!(["print('after')"]));
}

#[test]
fn ast_query_and_durable_rewrite_support_apply_and_discard() {
    let root = TempDir::new("ast-rewrite");
    let session = root.path().join("session");
    let artifacts = session.join("artifacts");
    fs::create_dir_all(&artifacts).unwrap();
    fs::write(session.join("transcript.jsonl"), "").unwrap();
    fs::write(session.join("state.json"), "{}").unwrap();
    fs::write(root.path().join("sample.rs"), "fn alpha() {}\nfn beta() {}\n").unwrap();
    let query = "(function_item name: (identifier) @name)";

    let searched = run(
        root.path(),
        Some(&artifacts),
        false,
        CancellationToken::new(),
        "ast_search",
        json!({"path":"sample.rs", "query":query, "capture":"name"}),
    )
    .unwrap();
    assert_eq!(searched["matches"].as_array().unwrap().iter().map(|item| item["text"].as_str().unwrap()).collect::<Vec<_>>(), vec!["alpha", "beta"]);
    assert_eq!(searched["matches"][0]["start"], json!({"line":1, "column":4}));

    let preview = run(
        root.path(),
        Some(&artifacts),
        false,
        CancellationToken::new(),
        "ast_rewrite",
        json!({"path":"sample.rs", "query":query, "capture":"name", "replacement":"renamed_$TEXT"}),
    )
    .unwrap();
    assert_eq!(preview["matchCount"], 2);
    assert!(preview["diff"].as_str().unwrap().contains("fn renamed_alpha() {}"));
    assert_eq!(fs::read_to_string(root.path().join("sample.rs")).unwrap(), "fn alpha() {}\nfn beta() {}\n");
    let apply_id = preview["pendingId"].as_str().unwrap().to_string();

    let applied = run(
        root.path(),
        Some(&artifacts),
        true,
        CancellationToken::new(),
        "ast_rewrite",
        json!({"action":"apply", "pendingId":apply_id}),
    )
    .unwrap();
    assert_eq!(applied["applied"], true);
    assert_eq!(fs::read_to_string(root.path().join("sample.rs")).unwrap(), "fn renamed_alpha() {}\nfn renamed_beta() {}\n");

    let discard_preview = run(
        root.path(),
        Some(&artifacts),
        false,
        CancellationToken::new(),
        "ast_rewrite",
        json!({"path":"sample.rs", "query":query, "capture":"name", "replacement":"discarded"}),
    )
    .unwrap();
    let discard_id = discard_preview["pendingId"].as_str().unwrap().to_string();
    let discarded = run(
        root.path(),
        Some(&artifacts),
        false,
        CancellationToken::new(),
        "ast_rewrite",
        json!({"action":"discard", "pendingId":discard_id}),
    )
    .unwrap();
    assert_eq!(discarded["discarded"], true);
    assert_eq!(fs::read_to_string(root.path().join("sample.rs")).unwrap(), "fn renamed_alpha() {}\nfn renamed_beta() {}\n");
}

#[test]
fn sqlite_mutation_requires_current_digest_and_archive_rejects_escaping_entry() {
    let root = TempDir::new("guarded-storage");
    let database = root.path().join("records.sqlite");
    let connection = Connection::open(&database).unwrap();
    connection.execute("CREATE TABLE records (id INTEGER PRIMARY KEY, name TEXT NOT NULL)", []).unwrap();
    drop(connection);

    let snapshot = run(
        root.path(),
        None,
        false,
        CancellationToken::new(),
        "read_binary_file",
        json!({"path":"records.sqlite", "maxBytes":1}),
    )
    .unwrap();
    let stale = run(
        root.path(),
        None,
        true,
        CancellationToken::new(),
        "write_sqlite",
        json!({
            "path":"records.sqlite", "expectedSha256":"stale", "table":"records",
            "action":"insert", "row":{"id":1, "name":"Ada"}
        }),
    )
    .unwrap_err();
    assert!(stale.contains("expectedSha256 mismatch"));

    let before = run(
        root.path(),
        None,
        false,
        CancellationToken::new(),
        "read_sqlite",
        json!({"path":"records.sqlite", "table":"records"}),
    )
    .unwrap();
    assert_eq!(before["rows"], json!([]));

    let inserted = run(
        root.path(),
        None,
        true,
        CancellationToken::new(),
        "write_sqlite",
        json!({
            "path":"records.sqlite", "expectedSha256":snapshot["sha256"], "table":"records",
            "action":"insert", "row":{"id":1, "name":"Ada"}
        }),
    )
    .unwrap();
    assert_eq!(inserted["affected"], 1);
    let after = run(
        root.path(),
        None,
        false,
        CancellationToken::new(),
        "read_sqlite",
        json!({"path":"records.sqlite", "table":"records"}),
    )
    .unwrap();
    assert_eq!(after["rows"], json!([{"id":1, "name":"Ada"}]));

    let archive_error = run(
        root.path(),
        None,
        true,
        CancellationToken::new(),
        "write_archive",
        json!({
            "path":"missing.zip", "entry":"../escape.txt", "expectedSha256":"irrelevant",
            "action":"upsert", "content":"escaped"
        }),
    )
    .unwrap_err();
    assert_eq!(archive_error, "unsafe archive entry path: ../escape.txt");
    assert!(!root.path().parent().unwrap().join("escape.txt").exists());
}

#[test]
fn configured_agent_tool_allowlist_hard_denies_execution_outside_the_list() {
    const TEST_NAME: &str = "tool_runtime::tests::configured_agent_tool_allowlist_hard_denies_execution_outside_the_list";
    if std::env::var_os("JEDEN_ALLOWLIST_CHILD").as_deref() != Some(std::ffi::OsStr::new("1")) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env("JEDEN_ALLOWLIST_CHILD", "1")
            .env("JEDEN_AGENT_TOOLS", "glob_paths")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "isolated allowlist test failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let root = TempDir::new("agent-tool-deny");
    fs::write(root.path().join("available.txt"), "content that must not be read\n").unwrap();
    let discovered = crate::tools::list_tools(root.path());
    assert!(discovered.iter().any(|tool| tool.name == "glob_paths"));
    assert!(!discovered.iter().any(|tool| tool.name == "read_file"));
    let error = run(
        root.path(),
        None,
        false,
        CancellationToken::new(),
        "read_file",
        json!({"path":"available.txt"}),
    )
    .unwrap_err();

    assert_eq!(error, "tool is not allowed for this agent: read_file");
}
