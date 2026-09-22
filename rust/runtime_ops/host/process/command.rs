//! What a caller asks to be run, and how its input and output are wired.
//!
//! Split out of `runtime_ops/host/process.rs`, which had grown past the
//! module line cap.

use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ManagedStdio {
    Captured,
    InheritedForeground,
}

#[derive(Clone, Debug)]
pub struct ManagedCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub env: Vec<(OsString, Option<OsString>)>,
    pub stdin: Option<Vec<u8>>,
    pub preserve_descendants: bool,
    pub(super) stdio: ManagedStdio,
}

impl ManagedCommand {
    pub fn new(program: impl Into<OsString>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            env: Vec::new(),
            stdin: None,
            preserve_descendants: false,
            stdio: ManagedStdio::Captured,
        }
    }

    pub(crate) fn inherit_stdio_for_foreground(&mut self) {
        self.stdio = ManagedStdio::InheritedForeground;
    }
}
