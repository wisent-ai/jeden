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

#[cfg(target_os = "linux")]
mod platform {
    //! The Linux half of the same contract the macOS module above holds:
    //! confine a child to declared read and write roots before it starts.
    //!
    //! Landlock is the kernel's own filesystem confinement and needs no
    //! privilege, no daemon and no signature. Until this existed Jeden
    //! reported `sandbox launcher not active (landlock=false)` on every
    //! Linux host and refused every session there, while
    //! `/sys/kernel/security/lsm` on the fleet's Linux machine already read
    //! `lockdown,capability,landlock,yama,apparmor,ima,evm`.

    use std::env;
    use std::ffi::OsString;
    use std::fs;
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{self, Command};
    use std::time::{SystemTime, UNIX_EPOCH};

    const CREATE_RULESET: libc::c_long = 444;
    const ADD_RULE: libc::c_long = 445;
    const RESTRICT_SELF: libc::c_long = 446;
    const RULE_PATH_BENEATH: libc::c_int = 1;
    const CREATE_RULESET_VERSION: libc::c_int = 1;

    /// Filesystem rights, in the order the ABI introduced them. The ruleset
    /// handles every right the running kernel knows and no more: asking for
    /// a right a kernel does not implement is refused outright, which would
    /// turn a working confinement into no confinement at all.
    const EXECUTE: u64 = 1;
    const WRITE_FILE: u64 = 1 << 1;
    const READ_FILE: u64 = 1 << 2;
    const READ_DIR: u64 = 1 << 3;
    const ABI1_ALL: u64 = (1 << 13) - 1;
    const REFER: u64 = 1 << 13;
    const TRUNCATE: u64 = 1 << 14;
    const IOCTL_DEV: u64 = 1 << 15;

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
                CREATE_RULESET,
                std::ptr::null::<RulesetAttr>(),
                0usize,
                CREATE_RULESET_VERSION,
            )
        };
        if version < 0 {
            return Err(format!(
                "this kernel exposes no Landlock ABI: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(version as i32)
    }

    fn handled_rights(abi: i32) -> u64 {
        let mut rights = ABI1_ALL;
        if abi >= 2 {
            rights |= REFER;
        }
        if abi >= 3 {
            rights |= TRUNCATE;
        }
        if abi >= 5 {
            rights |= IOCTL_DEV;
        }
        rights
    }

    fn canonical_roots(values: Vec<PathBuf>) -> Vec<PathBuf> {
        let mut roots = values
            .into_iter()
            .filter_map(|path| fs::canonicalize(&path).ok().or(Some(path)))
            .filter(|path| path.exists())
            .collect::<Vec<_>>();
        roots.sort();
        roots.dedup();
        roots
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
                ADD_RULE,
                ruleset,
                RULE_PATH_BENEATH,
                &rule as *const PathBeneathAttr,
                0usize,
            )
        };
        if added != 0 {
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
                CREATE_RULESET,
                &attribute as *const RulesetAttr,
                std::mem::size_of::<RulesetAttr>(),
                0,
            )
        };
        if ruleset < 0 {
            return Err(format!(
                "cannot create a Landlock ruleset: {}",
                std::io::Error::last_os_error()
            ));
        }
        let ruleset = ruleset as libc::c_int;
        let readable = (EXECUTE | READ_FILE | READ_DIR) & handled;
        for root in read_roots {
            add_root(ruleset, root, readable)?;
        }
        for root in write_roots {
            add_root(ruleset, root, handled)?;
        }
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            return Err(format!(
                "cannot set no-new-privs: {}",
                std::io::Error::last_os_error()
            ));
        }
        if unsafe { libc::syscall(RESTRICT_SELF, ruleset, 0) } != 0 {
            return Err(format!(
                "cannot enforce the Landlock ruleset: {}",
                std::io::Error::last_os_error()
            ));
        }
        unsafe { libc::close(ruleset) };
        Ok(abi)
    }

    /// Enforce a profile that permits reading `/` and writing nowhere, then
    /// prove it by failing to write. A kernel that accepts the ruleset and
    /// still allows the write has enforced nothing.
    fn probe() -> Result<(), String> {
        let marker = env::temp_dir().join(format!(
            "jeden-sandbox-probe-{}-{}",
            process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
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
        let mut args = env::args_os().skip(1).peekable();
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
        let program = command.first().cloned().ok_or_else(usage)?;
        if read_roots.is_empty() || write_roots.is_empty() {
            return Err("sandbox helper requires at least one read and write root".into());
        }
        let read_roots = canonical_roots(read_roots);
        let write_roots = canonical_roots(write_roots);
        apply(&read_roots, &write_roots)?;
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn main() {
    platform::run_main();
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn main() {
    eprintln!("jeden-sandbox-helper has no sandbox backend for this platform");
    std::process::exit(1);
}
