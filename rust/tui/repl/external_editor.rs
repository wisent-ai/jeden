use std::env;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::tool_runtime::runtime_ops::{ManagedCommand, OperationContext, ProcessManager, TerminationReason};

use super::super::EditorState;

const EXTERNAL_EDITOR_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExternalEditorCommand {
    program: OsString,
    args: Vec<OsString>,
}

impl ExternalEditorCommand {
    fn from_environment() -> Result<Self, String> {
        let visual = env::var("VISUAL").ok();
        let editor = env::var("EDITOR").ok();
        Self::from_configuration(visual.as_deref(), editor.as_deref())
    }

    fn from_configuration(visual: Option<&str>, editor: Option<&str>) -> Result<Self, String> {
        let configured = visual
            .filter(|value| !value.trim().is_empty())
            .or_else(|| editor.filter(|value| !value.trim().is_empty()))
            .ok_or_else(|| "external editor unavailable: set VISUAL or EDITOR".to_string())?;
        Self::parse(configured)
    }

    fn parse(configured: &str) -> Result<Self, String> {
        let mut words = shell_words::split(configured)
            .map_err(|error| format!("invalid external editor command: {error}"))?
            .into_iter();
        let program = words
            .next()
            .filter(|program| !program.is_empty())
            .ok_or_else(|| "invalid external editor command: program is empty".to_string())?;
        Ok(Self {
            program: program.into(),
            args: words.map(OsString::from).collect(),
        })
    }

    fn is_available(&self, cwd: &Path) -> bool {
        let program = Path::new(&self.program);
        if program.components().count() > 1 {
            let resolved = if program.is_absolute() { program.to_path_buf() } else { cwd.join(program) };
            return is_executable(&resolved);
        }
        env::var_os("PATH")
            .into_iter()
            .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
            .any(|directory| is_executable(&directory.join(program)))
    }
}

pub(crate) fn external_editor_health(cwd: &Path) -> Result<(), String> {
    let command = ExternalEditorCommand::from_environment()?;
    if command.is_available(cwd) {
        Ok(())
    } else {
        Err(format!(
            "external editor executable `{}` was not found",
            command.program.to_string_lossy()
        ))
    }
}

pub(crate) fn external_editor(
    editor: &mut EditorState,
    cwd: &Path,
    operation: &OperationContext<'_>,
) -> Result<bool, String> {
    let command = ExternalEditorCommand::from_environment()?;
    if !command.is_available(cwd) {
        return Err(format!(
            "external editor executable `{}` was not found",
            command.program.to_string_lossy()
        ));
    }
    external_editor_with(editor, cwd, operation, command, &env::temp_dir())
}

fn external_editor_with(
    editor: &mut EditorState,
    cwd: &Path,
    operation: &OperationContext<'_>,
    command: ExternalEditorCommand,
    temp_root: &Path,
) -> Result<bool, String> {
    if operation.cancellation().is_cancelled() {
        return Err("external editor cancelled".into());
    }
    let mut temporary = TemporaryEditorFile::create(temp_root)?;
    temporary.write(editor.text().as_bytes())?;

    let mut managed = ManagedCommand::new(command.program, cwd);
    managed.args = command.args;
    managed.args.push(temporary.path().as_os_str().to_owned());
    managed.inherit_stdio_for_foreground();
    let result = ProcessManager.run(operation, managed, EXTERNAL_EDITOR_TIMEOUT)?;
    if result.reason != TerminationReason::Completed {
        return Err(match result.reason {
            TerminationReason::Cancelled => "external editor cancelled".into(),
            TerminationReason::TimedOut => "external editor timed out".into(),
            TerminationReason::Completed => unreachable!(),
        });
    }
    if !result.status.success() {
        return Err(format!(
            "external editor exited with status {}",
            result.status.code().map_or_else(|| "unknown".into(), |code| code.to_string())
        ));
    }

    let bytes = fs::read(temporary.path()).map_err(|error| format!("read external editor file: {error}"))?;
    let text = String::from_utf8(bytes).map_err(|_| "external editor produced invalid UTF-8".to_string())?;
    editor.replace_all_transaction(text).map_err(|error| error.to_string())
}

struct TemporaryEditorFile {
    path: PathBuf,
    file: Option<File>,
}

impl TemporaryEditorFile {
    fn create(root: &Path) -> Result<Self, String> {
        for _ in 0..128 {
            let id = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!("jeden-editor-{}-{id}.txt", std::process::id()));
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => return Ok(Self { path, file: Some(file) }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("create external editor file: {error}")),
            }
        }
        Err("create external editor file: exhausted unique names".into())
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut file = self.file.take().expect("temporary editor file is written once");
        file.write_all(bytes).map_err(|error| format!("write external editor file: {error}"))?;
        file.sync_all().map_err(|error| format!("sync external editor file: {error}"))?;
        Ok(())
    }
}

impl Drop for TemporaryEditorFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(all(test, unix))]
mod tests {
    use std::ffi::OsStr;
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::tool_runtime::runtime_ops::{ArtifactSink, CancellationToken};
    use crate::tui::editor::EditorAction;

    use super::*;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn create(name: &str) -> Self {
            for _ in 0..128 {
                let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
                let root = env::temp_dir().join(format!(
                    "jeden-external-editor-test-{name}-{}-{id}",
                    std::process::id()
                ));
                match fs::create_dir(&root) {
                    Ok(()) => return Self { root },
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("create fixture directory: {error}"),
                }
            }
            panic!("create fixture directory: exhausted unique names");
        }

        fn script(&self, name: &str, body: &str) -> PathBuf {
            let path = self.root.join(name);
            let mut options = OpenOptions::new();
            options.write(true).create_new(true).mode(0o700);
            let mut file = options.open(&path).expect("create fixture script");
            file.write_all(body.as_bytes()).expect("write fixture script");
            file.sync_all().expect("sync fixture script");
            path
        }

        fn operation(&self) -> OperationContext<'static> {
            OperationContext::new(
                CancellationToken::new(),
                ArtifactSink::new(self.root.join("artifacts")),
            )
        }

        fn assert_no_editor_temp_files(&self) {
            let leftovers = fs::read_dir(&self.root)
                .expect("read fixture directory")
                .map(|entry| entry.expect("read fixture entry").file_name())
                .filter(|name| name.to_string_lossy().starts_with("jeden-editor-"))
                .collect::<Vec<_>>();
            assert!(leftovers.is_empty(), "temporary editor files remain: {leftovers:?}");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn temporary_editor_file_is_owner_only_and_removed_on_drop() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = Fixture::create("temporary-permissions");
        let temporary = TemporaryEditorFile::create(&fixture.root).expect("create temporary editor file");

        let mode = temporary
            .path()
            .metadata()
            .expect("read temporary editor file metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);

        drop(temporary);
        fixture.assert_no_editor_temp_files();
    }

    #[test]
    fn external_editor_configuration_prefers_visual_falls_back_to_editor_and_rejects_blanks() {
        assert_eq!(
            ExternalEditorCommand::from_configuration(Some("visual --wait"), Some("editor --fallback")),
            Ok(ExternalEditorCommand {
                program: "visual".into(),
                args: vec!["--wait".into()],
            })
        );
        assert_eq!(
            ExternalEditorCommand::from_configuration(Some(" \t"), Some("editor 'argument with spaces'")),
            Ok(ExternalEditorCommand {
                program: "editor".into(),
                args: vec!["argument with spaces".into()],
            })
        );

        let unavailable = Err("external editor unavailable: set VISUAL or EDITOR".to_string());
        assert_eq!(ExternalEditorCommand::from_configuration(None, None), unavailable);
        assert_eq!(
            ExternalEditorCommand::from_configuration(Some("  "), Some("\n\t")),
            unavailable
        );
    }

    #[test]
    fn external_editor_success_round_trips_unicode_parses_quoted_arg_and_is_one_undo_transaction() {
        let fixture = Fixture::create("success");
        let sentinel = fixture.root.join("shell-interpreted");
        let script = fixture.script(
            "editor.sh",
            "#!/bin/sh\n[ \"$1\" = 'argument with spaces; $(touch shell-interpreted)' ] || exit 91\nprintf 'Zażółć 👩🏽‍💻\\n第二行' > \"$2\"\n",
        );
        let configured = format!(
            "{} 'argument with spaces; $(touch shell-interpreted)'",
            shell_words::quote(&script.to_string_lossy())
        );
        let command = ExternalEditorCommand::parse(&configured).expect("parse fixture editor command");
        assert_eq!(command.program, script.as_os_str());
        assert_eq!(command.args, [OsStr::new("argument with spaces; $(touch shell-interpreted)")]);

        let mut editor = EditorState::default();
        editor.set_text("exact original");
        editor.insert(" 🧪");
        let original = editor.text().to_owned();

        assert_eq!(
            external_editor_with(&mut editor, &fixture.root, &fixture.operation(), command, &fixture.root),
            Ok(true)
        );
        assert_eq!(editor.text(), "Zażółć 👩🏽‍💻\n第二行");
        assert!(!sentinel.exists(), "editor arguments were interpreted by a shell");
        fixture.assert_no_editor_temp_files();

        editor.apply(EditorAction::Undo);
        assert_eq!(editor.text(), original);
        editor.apply(EditorAction::Undo);
        assert_eq!(editor.text(), "exact original");
    }

    #[test]
    fn external_editor_nonzero_exit_preserves_text_and_cleans_temp_file() {
        let fixture = Fixture::create("nonzero");
        let script = fixture.script("editor.sh", "#!/bin/sh\nprintf 'discard me' > \"$1\"\nexit 17\n");
        let mut editor = EditorState::default();
        editor.set_text("keep this 🧪");

        let result = external_editor_with(
            &mut editor,
            &fixture.root,
            &fixture.operation(),
            ExternalEditorCommand { program: script.into_os_string(), args: Vec::new() },
            &fixture.root,
        );

        assert_eq!(result, Err("external editor exited with status 17".into()));
        assert_eq!(editor.text(), "keep this 🧪");
        fixture.assert_no_editor_temp_files();
    }

    #[test]
    fn external_editor_invalid_utf8_preserves_text_and_cleans_temp_file() {
        let fixture = Fixture::create("invalid-utf8");
        let script = fixture.script("editor.sh", "#!/bin/sh\nprintf '\\377' > \"$1\"\n");
        let mut editor = EditorState::default();
        editor.set_text("keep this 🧪");

        let result = external_editor_with(
            &mut editor,
            &fixture.root,
            &fixture.operation(),
            ExternalEditorCommand { program: script.into_os_string(), args: Vec::new() },
            &fixture.root,
        );

        assert_eq!(result, Err("external editor produced invalid UTF-8".into()));
        assert_eq!(editor.text(), "keep this 🧪");
        fixture.assert_no_editor_temp_files();
    }

    #[test]
    fn external_editor_pre_cancel_preserves_text_without_running_editor_or_leaving_temp_file() {
        let fixture = Fixture::create("pre-cancel");
        let sentinel = fixture.root.join("editor-ran");
        let script = fixture.script("editor.sh", "#!/bin/sh\ntouch editor-ran\nprintf 'discard me' > \"$1\"\n");
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let operation = OperationContext::new(cancellation, ArtifactSink::new(fixture.root.join("artifacts")));
        let mut editor = EditorState::default();
        editor.set_text("keep this 🧪");

        let result = external_editor_with(
            &mut editor,
            &fixture.root,
            &operation,
            ExternalEditorCommand { program: script.into_os_string(), args: Vec::new() },
            &fixture.root,
        );

        assert_eq!(result, Err("external editor cancelled".into()));
        assert_eq!(editor.text(), "keep this 🧪");
        assert!(!sentinel.exists(), "pre-cancelled editor process ran");
        fixture.assert_no_editor_temp_files();
    }
}
