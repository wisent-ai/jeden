//! One isolated workspace per case: its own home, its own session root, its
//! own seeded documentation corpus, and its own project configuration.
//!
//! Nothing here touches the operator's `~/.jeden`, memory store, or Omp tool
//! directory. Runs keep their state under this checkout's ignored build
//! directory, which `~/agents/operations.md` requires.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn new(tag: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/context-runs")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for directory in [
            "home",
            "sessions",
            "workspace/notes",
            "workspace/src",
            "workspace/.jeden",
        ] {
            fs::create_dir_all(root.join(directory)).expect("create the isolated directory");
        }
        let workspace = Self { root };
        workspace.seed_corpus();
        workspace
    }

    pub fn cwd(&self) -> PathBuf {
        self.root.join("workspace")
    }

    pub fn session_root(&self) -> PathBuf {
        self.root.join("sessions")
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// A small corpus with one prose section that answers a question, one
    /// that does not, a source file, and a file that is not text at all, so
    /// a recommendation can be wrong in a visible way rather than trivially
    /// right.
    fn seed_corpus(&self) {
        fs::write(
            self.cwd().join("notes/fleet.md"),
            "# Fleet notes\n\nIntroduction with no answer in it.\n\n## Lease renewal\n\nA lease is renewed with `stado lease renew --target mini`.\nAn expired lease is refused with `lease expired`.\n\n## Colour of the office\n\nThe office is painted grey.\n",
        )
        .expect("seed the prose corpus");
        fs::write(
            self.cwd().join("src/lease.rs"),
            "use std::time::Instant;\n\nfn renew_lease(target: &str, deadline: Instant) -> bool {\n    // A renewal past its deadline is refused rather than extended.\n    Instant::now() < deadline && !target.is_empty()\n}\n",
        )
        .expect("seed the source corpus");
        fs::write(self.cwd().join("src/lease.bin"), b"lease\0renewal\0binary")
            .expect("seed a file that is not text");
    }

    /// The project configuration a case wants, written where Jeden reads a
    /// project override: `<cwd>/.jeden/config.json`.
    pub fn configure(&self, advisor: serde_json::Value) {
        let document = serde_json::json!({"context": {"advisor": advisor}});
        fs::write(
            self.cwd().join(".jeden/config.json"),
            serde_json::to_string_pretty(&document).expect("serialize the project configuration"),
        )
        .expect("write the project configuration");
    }

    /// The binary under test, with the operator's environment removed: an
    /// isolated home, an isolated session root, and no inherited
    /// ground-truth endpoint.
    pub fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_jeden"));
        command
            .current_dir(self.cwd())
            .env("HOME", self.root.join("home"))
            .env("JEDEN_SESSION_ROOT", self.session_root())
            .env_remove("WISENT_GROUND_TRUTH_API")
            .env_remove("GROUND_TRUTH_API")
            .env_remove("JEDEN_LANGUAGE");
        command
    }

    pub fn run(&self, arguments: &[&str]) -> Run {
        let output = self
            .command()
            .args(arguments)
            .output()
            .expect("run the jeden binary");
        Run {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }
    }

    /// Every transcript recorded under this workspace's session root, joined.
    /// A turn records its own ledger and the read-only inspection records
    /// another, so a case looks through all of them rather than guessing
    /// which directory is which.
    pub fn transcript(&self) -> String {
        let mut directories: Vec<PathBuf> = fs::read_dir(self.session_root())
            .expect("read the session root")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        directories.sort();
        assert!(
            !directories.is_empty(),
            "a run records at least one session directory"
        );
        directories
            .iter()
            .filter_map(|directory| fs::read_to_string(directory.join("transcript.jsonl")).ok())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn read_to_string(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

pub struct Run {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|error| {
            panic!(
                "stdout is not JSON ({error}): {}\nstderr: {}",
                self.stdout, self.stderr
            )
        })
    }
}
