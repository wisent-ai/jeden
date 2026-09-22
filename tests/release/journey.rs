//! One recorded release-boundary journey: the tool it drives, every command
//! it runs with its exact environment, and the report it keeps.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) const INPUT_ENV: &str = "WISENT_INPUT_PRIVATE_CARGO_SOURCES_DIR";
pub(crate) const SYSTEM_PATH: &str = "/usr/bin:/bin";

pub(crate) fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub(crate) fn sha256(path: &Path) -> String {
    let bytes = fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn env_map() -> BTreeMap<String, String> {
    env::vars().collect()
}

pub(crate) struct Journey {
    pub(crate) report: PathBuf,
    trace: Value,
    tool: PathBuf,
}

impl Journey {
    pub(crate) fn start(scope: &str) -> Self {
        let report = root()
            .join(".wisent-output/release-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&report).expect("create report directory");
        let tool_target = root().join("target/tools");
        let cargo = env::var("CARGO").expect("cargo test sets CARGO");
        let built = Command::new(cargo)
            .args([
                "build",
                "--quiet",
                "--locked",
                "--manifest-path",
                "tools/Cargo.toml",
            ])
            .arg("--target-dir")
            .arg(&tool_target)
            .current_dir(root())
            .status()
            .expect("build jeden-tools");
        assert!(built.success(), "jeden-tools did not build");
        let mut journey = Self {
            report,
            trace: json!({"status": "failed", "scope": scope, "commands": []}),
            tool: tool_target.join("debug/jeden-tools"),
        };
        let revision = match env::var("WISENT_SOURCE_COMMIT") {
            Ok(revision) if !revision.is_empty() => revision,
            _ => {
                let patch = journey
                    .run(&["git", "diff", "--binary", "HEAD"], &env_map(), 0)
                    .0;
                fs::write(journey.report.join("source.patch"), patch).expect("write source patch");
                let head = journey.run(&["git", "rev-parse", "HEAD"], &env_map(), 0).0;
                head.trim().to_string()
            }
        };
        journey.trace["source_revision"] = json!(revision);
        journey
    }

    fn save(&self) {
        let text = serde_json::to_string_pretty(&self.trace).expect("encode report") + "\n";
        fs::write(self.report.join("report.json"), text).expect("write report");
    }

    /// Run a command from the repository root with exactly `env`, keep its
    /// output and exit status in the report, and require `expected`.
    fn run(
        &mut self,
        argv: &[&str],
        env: &BTreeMap<String, String>,
        expected: i32,
    ) -> (String, String) {
        let label = self.trace["commands"].as_array().map_or(0, Vec::len);
        let output = Command::new(argv[0])
            .args(&argv[1..])
            .env_clear()
            .envs(env)
            .current_dir(root())
            .output()
            .unwrap_or_else(|error| panic!("cannot start {argv:?}: {error}"));
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        fs::write(self.report.join(format!("{label}.stdout")), &stdout).expect("write stdout");
        fs::write(self.report.join(format!("{label}.stderr")), &stderr).expect("write stderr");
        let code = output.status.code();
        self.trace["commands"]
            .as_array_mut()
            .expect("commands")
            .push(json!({
                "argv": argv, "exit_code": code, "stdout": stdout, "stderr": stderr,
            }));
        self.save();
        assert_eq!(code, Some(expected), "{argv:?}\n{stderr}");
        (stdout, stderr)
    }

    pub(crate) fn tool(
        &mut self,
        arguments: &[&str],
        env: &BTreeMap<String, String>,
        expected: i32,
    ) -> (String, String) {
        let tool = self.tool.display().to_string();
        let mut argv = vec![tool.as_str()];
        argv.extend_from_slice(arguments);
        self.run(&argv, env, expected)
    }

    pub(crate) fn passed(mut self) {
        self.trace["status"] = json!("passed");
        self.save();
        println!("{}", self.report.join("report.json").display());
    }

    /// Staging refuses a missing output directory, a missing input and a
    /// missing toolchain before creating anything, then stages a binary
    /// identical to the one Cargo built, and that binary runs.
    pub(crate) fn stage_native(&mut self, env: &BTreeMap<String, String>, binary: &str) {
        let stager = ["release", "stage", "--bin", binary];
        let mut no_output = env.clone();
        no_output.remove("WISENT_OUTPUT_DIR");
        let refused = self.tool(&stager, &no_output, 1).1;
        assert!(
            refused.contains("WISENT_OUTPUT_DIR is required for native staging"),
            "{refused}"
        );
        let staged = self.report.join("staged native");
        let mut worker = env.clone();
        worker.insert("PATH".into(), SYSTEM_PATH.into());
        worker.insert("WISENT_OUTPUT_DIR".into(), staged.display().to_string());
        self.trace["worker_path"] = json!(SYSTEM_PATH);
        let mut no_input = worker.clone();
        no_input.insert(INPUT_ENV.into(), String::new());
        let refused = self.tool(&stager, &no_input, 1).1;
        assert!(
            refused.contains(&format!("{INPUT_ENV} is required")),
            "{refused}"
        );
        assert!(!staged.exists());
        let mut no_toolchain = worker.clone();
        no_toolchain.insert("PATH".into(), String::new());
        let missing_home = self.report.join("missing-cargo-home");
        no_toolchain.insert("CARGO_HOME".into(), missing_home.display().to_string());
        let refused = self.tool(&stager, &no_toolchain, 1).1;
        assert!(
            refused.contains("Cargo is unavailable on PATH and at "),
            "{refused}"
        );
        assert!(!staged.exists());
        self.tool(&stager, &worker, 0);
        let executable = staged.join("bin").join(binary);
        let version = self
            .run(&[&executable.display().to_string(), "--version"], env, 0)
            .0;
        assert_eq!(
            sha256(&executable),
            sha256(&root().join("target/release").join(binary))
        );
        self.trace["artifact"] = json!({
            "path": executable, "sha256": sha256(&executable), "version": version.trim(),
        });
    }
}
