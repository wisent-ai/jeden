//! Where the token store lives and what its endpoint, directory and file
//! must be before a key is read from or written to it.

#[cfg(not(unix))]
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

/// A model endpoint as the store keeps it: HTTPS, or HTTP to this machine's
/// loopback, without credentials, query or fragment, and without a trailing
/// slash.
pub(crate) fn canonical_endpoint(endpoint: &str) -> Result<String, String> {
    let url =
        url::Url::parse(endpoint).map_err(|error| format!("invalid model endpoint: {error}"))?;
    let loopback = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(name)) => name == "localhost",
        None => false,
    };
    let secure = url.scheme() == "https";
    let local = url.scheme() == "http" && loopback;
    let userinfo = endpoint.split_once("://").is_some_and(|(_, rest)| {
        rest.split('/')
            .next()
            .is_some_and(|authority| authority.contains('@'))
    });
    if !(secure || local)
        || userinfo
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "model endpoint must be HTTPS or loopback HTTP, without credentials, query or fragment"
                .into(),
        );
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// The directory the store lives in: the operator's `~/.jeden`.
pub(super) fn store_directory() -> PathBuf {
    crate::dirs_home().join(".jeden")
}

/// The store itself.
pub(super) fn store_path() -> PathBuf {
    store_directory().join("model-tokens.sqlite3")
}

/// Whether `mode` lets anyone but the owner read, write or enter: any of the
/// group's or others' permission bits is set in it.
#[cfg(unix)]
fn shared(mode: u32) -> bool {
    [
        libc::S_IRGRP,
        libc::S_IWGRP,
        libc::S_IXGRP,
        libc::S_IROTH,
        libc::S_IWOTH,
        libc::S_IXOTH,
    ]
    .into_iter()
    .map(u32::from)
    .any(|bit| mode & bit == bit)
}

#[cfg(unix)]
pub(super) fn check_directory(dir: &Path, protect: bool) -> Result<(), String> {
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let meta = fs::symlink_metadata(dir)
        .map_err(|error| format!("inspect token directory {}: {error}", dir.display()))?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } {
        return Err(format!(
            "token directory {} is not owned by this user or is a symlink",
            dir.display()
        ));
    }
    if protect {
        fs::set_permissions(dir, fs::Permissions::from_mode(u32::from(libc::S_IRWXU)))
            .map_err(|error| format!("protect token directory {}: {error}", dir.display()))?;
    } else if shared(meta.permissions().mode()) {
        return Err(format!("token directory {} is not private", dir.display()));
    }
    Ok(())
}

pub(super) fn check_file(path: &Path, create: bool) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::fs::{self, OpenOptions};
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
        if create {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(u32::from(libc::S_IRUSR | libc::S_IWUSR))
                .open(path)
            {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(format!("create token store {}: {error}", path.display()))
                }
            }
        }
        let meta = fs::symlink_metadata(path)
            .map_err(|error| format!("inspect token store {}: {error}", path.display()))?;
        if !meta.is_file()
            || shared(meta.permissions().mode())
            || meta.uid() != unsafe { libc::geteuid() }
        {
            return Err(format!(
                "token store {} is not a private file owned by this user",
                path.display()
            ));
        }
    }
    #[cfg(not(unix))]
    {
        if create {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(format!("create token store {}: {error}", path.display()))
                }
            }
        }
    }
    Ok(())
}
