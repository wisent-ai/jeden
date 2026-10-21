//! The macOS half: build the sandbox profile from the declared read and
//! write roots, apply it to this process, and only then become the command,
//! so there is no window in which the child runs unconfined.

use std::env;
use std::ffi::{CStr, CString, OsString};
use std::fs;
use std::os::raw::{c_char, c_int};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

#[link(name = "sandbox")]
unsafe extern "C" {
    fn sandbox_init(profile: *const c_char, flags: u64, error_buffer: *mut *mut c_char) -> c_int;
    fn sandbox_free_error(error_buffer: *mut c_char);
}

/// sandbox_init's flags for a profile passed as its text rather than named
/// with SANDBOX_NAMED: none set
/// (https://developer.apple.com/library/archive/documentation/Darwin/Reference/ManPages/man3/sandbox_init.3.html).
const PROFILE_AS_TEXT: u64 = 0;
/// What sandbox_init returns when the profile is in force; on failure it
/// returns -1 and sets its error buffer
/// (https://developer.apple.com/library/archive/documentation/Darwin/Reference/ManPages/man3/sandbox_init.3.html).
const SANDBOX_APPLIED: c_int = 0;
/// The first line of every Seatbelt profile: the profile language version
/// sandbox_init reads, the only one the system defines
/// (https://reverse.put.as/wp-content/uploads/2011/09/Apple-Sandbox-Guide-v1.0.pdf).
const PROFILE_VERSION: &str = "(version 1)";

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

/// Each root resolved to the path the kernel checks. The caller passes only
/// roots that exist, so one that no longer resolves is refused by name rather
/// than confined by a path the kernel never sees.
fn canonical_roots(values: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    let mut roots = values
        .into_iter()
        .map(|path| {
            path.canonicalize().map_err(|error| {
                format!(
                    "sandbox root {} cannot be resolved: {error}",
                    path.display()
                )
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    roots.sort();
    roots.dedup();
    Ok(roots)
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
        "{PROFILE_VERSION}\n(allow default)\n(deny file-read-data (require-all {read_filter}))\n(deny file-write* (require-all {write_filter}))\n(allow file-link (subpath \"/Applications/Google Chrome.app\"))\n"
    )
}

fn apply_profile(profile: &str) -> Result<(), String> {
    let profile =
        CString::new(profile).map_err(|error| format!("sandbox profile contains NUL: {error}"))?;
    let mut error_buffer: *mut c_char = std::ptr::null_mut();
    let status = unsafe { sandbox_init(profile.as_ptr(), PROFILE_AS_TEXT, &mut error_buffer) };
    if status == SANDBOX_APPLIED {
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

/// Apply a profile that denies every write, then prove it by failing to
/// write a file named for this process and a random identity, so no clock is
/// read to make the name unique.
fn probe() -> Result<(), String> {
    let marker = env::temp_dir().join(format!(
        "jeden-sandbox-probe-{}-{}",
        process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    apply_profile(&format!(
        "{PROFILE_VERSION}\n(allow default)\n(deny file-write*)\n"
    ))?;
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
    let invocation: Vec<OsString> = env::args_os().collect();
    let Some((_, given)) = invocation.split_first() else {
        return Err(usage());
    };
    let mut args = given.iter().cloned().peekable();
    if args.peek().is_some_and(|value| value == "--version") {
        args.next();
        if args.next().is_some() {
            return Err(usage());
        }
        println!("jeden-sandbox-helper {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
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
    let (program, arguments) = command.split_first().ok_or_else(usage)?;
    if read_roots.is_empty() || write_roots.is_empty() {
        return Err("sandbox helper requires at least one read and write root".into());
    }
    let read_roots = canonical_roots(read_roots)?;
    let write_roots = canonical_roots(write_roots)?;
    apply_profile(&task_profile(&read_roots, &write_roots))?;
    let error = Command::new(program).args(arguments).exec();
    Err(format!("sandboxed exec failed: {error}"))
}

pub(super) fn run_main() {
    if let Err(error) = run() {
        eprintln!("jeden-sandbox-helper: {error}");
        process::exit(libc::EXIT_FAILURE);
    }
}
