use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct IsolatedCwd(PathBuf);

impl IsolatedCwd {
    fn new() -> Self {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "jeden-cli-version-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create isolated CLI working directory");
        assert!(!path.join(".jeden").exists());
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for IsolatedCwd {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn long_version_flag_prints_package_version_without_configuration() {
    let cwd = IsolatedCwd::new();
    let output = Command::new(env!("CARGO_BIN_EXE_jeden"))
        .arg("--version")
        .current_dir(cwd.path())
        .output()
        .expect("run jeden --version");

    assert!(output.status.success(), "status: {:?}", output.status);
    assert_eq!(
        output.stdout,
        format!("jeden {}\n", env!("CARGO_PKG_VERSION")).as_bytes()
    );
    assert!(output.stderr.is_empty(), "stderr: {:?}", output.stderr);
}

#[test]
fn short_version_flag_prints_package_version_without_configuration() {
    let cwd = IsolatedCwd::new();
    let output = Command::new(env!("CARGO_BIN_EXE_jeden"))
        .arg("-V")
        .current_dir(cwd.path())
        .output()
        .expect("run jeden -V");

    assert!(output.status.success(), "status: {:?}", output.status);
    assert_eq!(
        output.stdout,
        format!("jeden {}\n", env!("CARGO_PKG_VERSION")).as_bytes()
    );
    assert!(output.stderr.is_empty(), "stderr: {:?}", output.stderr);
}

#[test]
fn unknown_options_still_fail() {
    let cwd = IsolatedCwd::new();
    let output = Command::new(env!("CARGO_BIN_EXE_jeden"))
        .arg("--definitely-unknown")
        .current_dir(cwd.path())
        .output()
        .expect("run jeden with an unknown option");

    assert!(!output.status.success(), "status: {:?}", output.status);
    assert!(output.stdout.is_empty(), "stdout: {:?}", output.stdout);
    let stderr = String::from_utf8(output.stderr).expect("CLI stderr is UTF-8");
    assert!(
        stderr.contains("unknown option: --definitely-unknown"),
        "stderr: {stderr:?}"
    );
}
