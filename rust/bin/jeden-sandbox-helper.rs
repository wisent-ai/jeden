#[cfg(target_os = "macos")]
mod platform {

    use std::env;
    use std::ffi::{CStr, CString, OsString};
    use std::fs;
    use std::os::raw::{c_char, c_int};
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{self, Command};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[link(name = "sandbox")]
    unsafe extern "C" {
        fn sandbox_init(
            profile: *const c_char,
            flags: u64,
            error_buffer: *mut *mut c_char,
        ) -> c_int;
        fn sandbox_free_error(error_buffer: *mut c_char);
    }

    fn escape_profile_literal(path: &Path) -> String {
        path.to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    }

    fn outside_roots_filter(roots: &[PathBuf]) -> String {
        roots
            .iter()
            .map(|root| {
                format!(
                    "(require-not (subpath \"{}\"))",
                    escape_profile_literal(root)
                )
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn canonical_roots(values: Vec<PathBuf>) -> Vec<PathBuf> {
        let mut roots = values
            .into_iter()
            .map(|path| path.canonicalize().unwrap_or(path))
            .collect::<Vec<_>>();
        roots.sort();
        roots.dedup();
        roots
    }

    fn task_profile(read_roots: &[PathBuf], write_roots: &[PathBuf]) -> String {
        let read_filter = format!(
            "(require-not (literal \"/\")) {}",
            outside_roots_filter(read_roots)
        );
        let write_filter = format!(
        "(require-not (literal \"/dev/null\")) (require-not (regex #\"^/private/var/folders/[^/]+/[^/]+/T/com[.]google[.]Chrome[.][^/]+\")) (require-not (regex #\"^/private/var/folders/[^/]+/[^/]+/X/com[.]google[.]Chrome[.]code_sign_clone.*\")) {}",
        outside_roots_filter(write_roots)
    );
        format!(
        "(version 1)\n(allow default)\n(deny file-read-data (require-all {read_filter}))\n(deny file-write* (require-all {write_filter}))\n(allow file-link (subpath \"/Applications/Google Chrome.app\"))\n"
    )
    }

    fn apply_profile(profile: &str) -> Result<(), String> {
        let profile =
            CString::new(profile).map_err(|_| "sandbox profile contains NUL".to_string())?;
        let mut error_buffer: *mut c_char = std::ptr::null_mut();
        let status = unsafe { sandbox_init(profile.as_ptr(), 0, &mut error_buffer) };
        if status == 0 {
            return Ok(());
        }
        let detail = if error_buffer.is_null() {
            format!("sandbox_init failed with status {status}")
        } else {
            let detail = unsafe { CStr::from_ptr(error_buffer) }
                .to_string_lossy()
                .into_owned();
            unsafe { sandbox_free_error(error_buffer) };
            detail
        };
        Err(detail)
    }

    fn probe() -> Result<(), String> {
        let marker = env::temp_dir().join(format!(
            "jeden-sandbox-probe-{}-{}",
            process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        apply_profile("(version 1)\n(allow default)\n(deny file-write*)\n")?;
        match fs::write(&marker, b"sandbox must deny this write") {
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Ok(()),
            Err(error) => Err(format!(
                "sandbox probe failed with unexpected error: {error}"
            )),
            Ok(()) => Err("sandbox profile was accepted but did not deny filesystem writes".into()),
        }
    }

    fn usage() -> String {
        "usage: jeden-sandbox-helper --probe | [--read <path>]... [--write <path>]... -- <program> [args...]".into()
    }

    fn run() -> Result<(), String> {
        let mut args = env::args_os().skip(1).peekable();
        if args.peek().is_some_and(|value| value == "--probe") {
            args.next();
            if args.next().is_some() {
                return Err(usage());
            }
            return probe();
        }

        let mut read_roots = Vec::new();
        let mut write_roots = Vec::new();
        let mut command = Vec::<OsString>::new();
        while let Some(argument) = args.next() {
            if argument == "--" {
                command.extend(args);
                break;
            }
            if argument == "--read" {
                read_roots.push(PathBuf::from(args.next().ok_or_else(usage)?));
            } else if argument == "--write" {
                write_roots.push(PathBuf::from(args.next().ok_or_else(usage)?));
            } else {
                return Err(usage());
            }
        }
        let program = command.first().cloned().ok_or_else(usage)?;
        if read_roots.is_empty() || write_roots.is_empty() {
            return Err("sandbox helper requires at least one read and write root".into());
        }
        let read_roots = canonical_roots(read_roots);
        let write_roots = canonical_roots(write_roots);
        apply_profile(&task_profile(&read_roots, &write_roots))?;
        let error = Command::new(program).args(command.iter().skip(1)).exec();
        Err(format!("sandboxed exec failed: {error}"))
    }

    pub(super) fn run_main() {
        if let Err(error) = run() {
            eprintln!("jeden-sandbox-helper: {error}");
            process::exit(1);
        }
    }
}

#[cfg(target_os = "macos")]
fn main() {
    platform::run_main();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("jeden-sandbox-helper is only available on macOS");
    std::process::exit(1);
}
