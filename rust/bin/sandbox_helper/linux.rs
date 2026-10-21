//! The Linux half of the same contract the macOS module beside it holds:
//! confine a child to declared read and write roots before it starts.
//!
//! Landlock is the kernel's own filesystem confinement and needs no
//! privilege, no daemon and no signature. Every number below is the
//! kernel's: the system calls from libc, the flags and rights from the
//! Landlock UAPI header and the ABI version each right arrived in from the
//! kernel's Landlock documentation, each cited where it is declared.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

/// `LANDLOCK_CREATE_RULESET_VERSION`: ask landlock_create_ruleset for the
/// highest ABI version the kernel supports
/// (https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/uapi/linux/landlock.h).
const CREATE_RULESET_VERSION: libc::c_uint = 1 << 0;
/// `LANDLOCK_RULE_PATH_BENEATH`, the rule type a filesystem root is added as
/// (https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/uapi/linux/landlock.h).
const RULE_PATH_BENEATH: libc::c_int = 1;
/// The flags landlock_create_ruleset with an attribute, landlock_add_rule
/// and landlock_restrict_self take: the manual says they must be 0
/// (https://man7.org/linux/man-pages/man2/landlock_create_ruleset.2.html).
const NO_FLAGS: libc::c_uint = 0;
/// The attribute size a version query passes beside its null attribute
/// (https://man7.org/linux/man-pages/man2/landlock_create_ruleset.2.html).
const NO_ATTRIBUTE: usize = 0;
/// What a Landlock system call returns when it succeeds; it returns -1 and
/// sets errno on error, and a ruleset or version is never negative
/// (https://man7.org/linux/man-pages/man2/landlock_add_rule.2.html).
const SUCCEEDED: libc::c_long = 0;
/// prctl(PR_SET_NO_NEW_PRIVS) takes 1 to set the bit
/// (https://man7.org/linux/man-pages/man2/PR_SET_NO_NEW_PRIVS.2const.html).
const SET: libc::c_ulong = 1;
/// prctl(PR_SET_NO_NEW_PRIVS) takes 0 in every argument after the first
/// (https://man7.org/linux/man-pages/man2/PR_SET_NO_NEW_PRIVS.2const.html).
const UNUSED: libc::c_ulong = 0;

// Filesystem rights, in the order the ABI introduced them. The ruleset handles every right the
// running kernel knows and no more: asking for one a kernel does not implement is refused
// outright, which would turn a working confinement into no confinement at all.

/// `LANDLOCK_ACCESS_FS_EXECUTE`
/// (https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/uapi/linux/landlock.h).
const EXECUTE: u64 = 1 << 0;
/// `LANDLOCK_ACCESS_FS_READ_FILE`
/// (https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/uapi/linux/landlock.h).
const READ_FILE: u64 = 1 << 2;
/// `LANDLOCK_ACCESS_FS_READ_DIR`
/// (https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/uapi/linux/landlock.h).
const READ_DIR: u64 = 1 << 3;
/// Every right of ABI 1, `LANDLOCK_ACCESS_FS_EXECUTE` (1 << 0) through
/// `LANDLOCK_ACCESS_FS_MAKE_SYM` (1 << 12)
/// (https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/uapi/linux/landlock.h).
const ABI1_ALL: u64 = (1 << 13) - 1;
/// `LANDLOCK_ACCESS_FS_REFER`
/// (https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/uapi/linux/landlock.h).
const REFER: u64 = 1 << 13;
/// `LANDLOCK_ACCESS_FS_TRUNCATE`
/// (https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/uapi/linux/landlock.h).
const TRUNCATE: u64 = 1 << 14;
/// `LANDLOCK_ACCESS_FS_IOCTL_DEV`
/// (https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/uapi/linux/landlock.h).
const IOCTL_DEV: u64 = 1 << 15;
/// The ABI version that added `LANDLOCK_ACCESS_FS_REFER`
/// (https://docs.kernel.org/userspace-api/landlock.html).
const REFER_ABI: i32 = 2;
/// The ABI version that added `LANDLOCK_ACCESS_FS_TRUNCATE`
/// (https://docs.kernel.org/userspace-api/landlock.html).
const TRUNCATE_ABI: i32 = 3;
/// The ABI version that added `LANDLOCK_ACCESS_FS_IOCTL_DEV`
/// (https://docs.kernel.org/userspace-api/landlock.html).
const IOCTL_DEV_ABI: i32 = 5;

#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
}

#[repr(C, packed)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: libc::c_int,
}

fn abi_version() -> Result<i32, String> {
    let version = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<RulesetAttr>(),
            NO_ATTRIBUTE,
            CREATE_RULESET_VERSION,
        )
    };
    if version < SUCCEEDED {
        return Err(format!(
            "this kernel exposes no Landlock ABI: {}",
            std::io::Error::last_os_error()
        ));
    }
    i32::try_from(version)
        .map_err(|error| format!("the Landlock ABI version {version} is not an i32: {error}"))
}

fn handled_rights(abi: i32) -> u64 {
    let mut rights = ABI1_ALL;
    if abi >= REFER_ABI {
        rights |= REFER;
    }
    if abi >= TRUNCATE_ABI {
        rights |= TRUNCATE;
    }
    if abi >= IOCTL_DEV_ABI {
        rights |= IOCTL_DEV;
    }
    rights
}

/// Each root resolved to the path the kernel checks. The caller passes only
/// roots that exist, so one that no longer resolves is refused by name rather
/// than left out of the ruleset.
fn canonical_roots(values: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    let mut roots = values
        .into_iter()
        .map(|path| {
            fs::canonicalize(&path).map_err(|error| {
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

fn add_root(ruleset: libc::c_int, root: &Path, allowed: u64) -> Result<(), String> {
    let directory = fs::File::open(root)
        .map_err(|error| format!("cannot open {} for the ruleset: {error}", root.display()))?;
    let rule = PathBeneathAttr {
        allowed_access: allowed,
        parent_fd: directory.as_raw_fd(),
    };
    let added = unsafe {
        libc::syscall(
            libc::SYS_landlock_add_rule,
            ruleset,
            RULE_PATH_BENEATH,
            &rule as *const PathBeneathAttr,
            NO_FLAGS,
        )
    };
    if added != SUCCEEDED {
        return Err(format!(
            "cannot allow {}: {}",
            root.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// Apply the confinement to this process; every child inherits it.
fn apply(read_roots: &[PathBuf], write_roots: &[PathBuf]) -> Result<i32, String> {
    let abi = abi_version()?;
    let handled = handled_rights(abi);
    let attribute = RulesetAttr {
        handled_access_fs: handled,
    };
    let ruleset = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &attribute as *const RulesetAttr,
            std::mem::size_of::<RulesetAttr>(),
            NO_FLAGS,
        )
    };
    if ruleset < SUCCEEDED {
        return Err(format!(
            "cannot create a Landlock ruleset: {}",
            std::io::Error::last_os_error()
        ));
    }
    let ruleset = libc::c_int::try_from(ruleset).map_err(|error| {
        format!("the Landlock ruleset descriptor {ruleset} is not a file descriptor: {error}")
    })?;
    let readable = (EXECUTE | READ_FILE | READ_DIR) & handled;
    for root in read_roots {
        add_root(ruleset, root, readable)?;
    }
    for root in write_roots {
        add_root(ruleset, root, handled)?;
    }
    let no_new_privs =
        unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, SET, UNUSED, UNUSED, UNUSED) };
    if libc::c_long::from(no_new_privs) != SUCCEEDED {
        return Err(format!(
            "cannot set no-new-privs: {}",
            std::io::Error::last_os_error()
        ));
    }
    if unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset, NO_FLAGS) } != SUCCEEDED {
        return Err(format!(
            "cannot enforce the Landlock ruleset: {}",
            std::io::Error::last_os_error()
        ));
    }
    unsafe { libc::close(ruleset) };
    Ok(abi)
}

/// Enforce a profile that permits reading `/` and writing nowhere, then
/// prove it by failing to write a file named for this process and a random
/// identity, so no clock is read to make the name unique. A kernel that
/// accepts the ruleset and still allows the write has enforced nothing.
fn probe() -> Result<(), String> {
    let marker = env::temp_dir().join(format!(
        "jeden-sandbox-probe-{}-{}",
        process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let abi = apply(&[PathBuf::from("/")], &[])?;
    match fs::write(&marker, b"sandbox must deny this write") {
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound
            ) =>
        {
            println!("landlock abi {abi} denied a write outside every declared root");
            Ok(())
        }
        Err(error) => Err(format!(
            "sandbox probe failed with unexpected error: {error}"
        )),
        Ok(()) => Err("the Landlock ruleset was accepted but did not deny writes".into()),
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
    apply(&read_roots, &write_roots)?;
    let error = Command::new(program).args(arguments).exec();
    Err(format!("sandboxed exec failed: {error}"))
}

pub(super) fn run_main() {
    if let Err(error) = run() {
        eprintln!("jeden-sandbox-helper: {error}");
        process::exit(libc::EXIT_FAILURE);
    }
}
