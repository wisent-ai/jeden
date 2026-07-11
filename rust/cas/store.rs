use super::digest::Digest;
use sha2::{Digest as ShaDigest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub enum CasError {
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    DigestMismatch {
        expected: Digest,
        actual: Digest,
    },
    CorruptObject {
        expected: Digest,
        actual: Digest,
    },
    InvalidOffset {
        expected: u64,
        actual: u64,
    },
    InvalidPath(String),
    UnsupportedEntry(PathBuf),
    CaseCollision {
        directory: PathBuf,
        first: String,
        second: String,
    },
    InvalidSnapshot(String),
    Serialization(String),
}

impl CasError {
    pub(crate) fn io(operation: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }
}
impl fmt::Display for CasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(f, "{operation} {}: {source}", path.display()),
            Self::DigestMismatch { expected, actual } => {
                write!(f, "digest mismatch: expected {expected}, got {actual}")
            }
            Self::CorruptObject { expected, actual } => write!(
                f,
                "corrupt CAS object {expected}: content hashes to {actual}"
            ),
            Self::InvalidOffset { expected, actual } => write!(
                f,
                "invalid upload offset: expected {expected}, got {actual}"
            ),
            Self::InvalidPath(message) => write!(f, "invalid snapshot path: {message}"),
            Self::UnsupportedEntry(path) => write!(
                f,
                "snapshot entry is not a regular file or directory: {}",
                path.display()
            ),
            Self::CaseCollision {
                directory,
                first,
                second,
            } => write!(
                f,
                "case-folding collision in {}: {first:?} and {second:?}",
                directory.display()
            ),
            Self::InvalidSnapshot(message) => write!(f, "invalid snapshot: {message}"),
            Self::Serialization(message) => write!(f, "snapshot serialization failed: {message}"),
        }
    }
}
impl std::error::Error for CasError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LocalCas {
    root: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UploadStatus {
    Incomplete { offset: u64 },
    Complete { digest: Digest },
}

impl LocalCas {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, CasError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root).map_err(|error| CasError::io("create CAS root", &root, error))?;
        let root_metadata = fs::symlink_metadata(&root)
            .map_err(|error| CasError::io("inspect CAS root", &root, error))?;
        if !root_metadata.file_type().is_dir() {
            return Err(CasError::UnsupportedEntry(root));
        }
        for directory in [root.join("objects"), root.join("uploads"), root.join("tmp")] {
            fs::create_dir_all(&directory)
                .map_err(|error| CasError::io("create CAS directory", &directory, error))?;
            let metadata = fs::symlink_metadata(&directory)
                .map_err(|error| CasError::io("inspect CAS directory", &directory, error))?;
            if !metadata.file_type().is_dir() {
                return Err(CasError::UnsupportedEntry(directory));
            }
        }
        Ok(Self { root })
    }

    pub fn new(root: impl AsRef<Path>) -> Result<Self, CasError> {
        Self::open(root)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn objects_dir(&self) -> PathBuf {
        self.root.join("objects")
    }
    pub(crate) fn object_path(&self, digest: Digest) -> PathBuf {
        let encoded = digest.to_string();
        self.objects_dir().join(&encoded[..2]).join(&encoded[2..])
    }

    pub fn put(&self, bytes: &[u8]) -> Result<Digest, CasError> {
        let digest = Digest::of(bytes);
        self.put_verified(digest, bytes)?;
        Ok(digest)
    }

    pub fn put_verified(&self, expected: Digest, bytes: &[u8]) -> Result<(), CasError> {
        let actual = Digest::of(bytes);
        if actual != expected {
            return Err(CasError::DigestMismatch { expected, actual });
        }
        let temporary = self.temporary_path("put");
        let mut file = self.create_temporary(&temporary)?;
        file.write_all(bytes)
            .map_err(|error| CasError::io("write CAS temporary object", &temporary, error))?;
        file.sync_all()
            .map_err(|error| CasError::io("sync CAS temporary object", &temporary, error))?;
        drop(file);
        self.commit_temporary(&temporary, expected)
    }

    pub fn get(&self, digest: Digest) -> Result<Vec<u8>, CasError> {
        let path = self.object_path(digest);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| CasError::io("inspect CAS object", &path, error))?;
        if !metadata.file_type().is_file() {
            return Err(CasError::UnsupportedEntry(path));
        }
        let bytes =
            fs::read(&path).map_err(|error| CasError::io("read CAS object", &path, error))?;
        let actual = Digest::of(&bytes);
        if actual != digest {
            return Err(CasError::CorruptObject {
                expected: digest,
                actual,
            });
        }
        Ok(bytes)
    }

    /// Returns false only when the object is absent; an existing corrupt object is an error.
    pub fn contains(&self, digest: Digest) -> Result<bool, CasError> {
        let path = self.object_path(digest);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_file() => self.get(digest).map(|_| true),
            Ok(_) => Err(CasError::UnsupportedEntry(path)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(CasError::io("inspect CAS object", path, error)),
        }
    }

    pub fn upload_offset(&self, expected: Digest) -> Result<u64, CasError> {
        let path = self.upload_path(expected);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_file() => {
                // Reading the entire partial object detects unreadable/replaced upload state on resume.
                let mut file = File::open(&path)
                    .map_err(|error| CasError::io("open partial upload", &path, error))?;
                let mut hash = Sha256::new();
                io::copy(&mut file, &mut HashWriter(&mut hash))
                    .map_err(|error| CasError::io("verify partial upload", &path, error))?;
                Ok(metadata.len())
            }
            Ok(_) => Err(CasError::UnsupportedEntry(path)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(CasError::io("inspect partial upload", path, error)),
        }
    }

    /// Appends one resumable chunk. The caller-provided offset must exactly match durable state.
    /// When `finish` is true, the complete upload is hash-checked and atomically committed.
    pub fn upload_chunk(
        &self,
        expected: Digest,
        offset: u64,
        chunk: &[u8],
        finish: bool,
    ) -> Result<UploadStatus, CasError> {
        if self.contains(expected)? {
            if offset == 0 && chunk.is_empty() {
                return Ok(UploadStatus::Complete { digest: expected });
            }
            return Err(CasError::InvalidOffset {
                expected: 0,
                actual: offset,
            });
        }
        let path = self.upload_path(expected);
        let current = self.upload_offset(expected)?;
        if current != offset {
            return Err(CasError::InvalidOffset {
                expected: current,
                actual: offset,
            });
        }
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        let mut file = options
            .open(&path)
            .map_err(|error| CasError::io("open partial upload", &path, error))?;
        let metadata = file
            .metadata()
            .map_err(|error| CasError::io("inspect partial upload", &path, error))?;
        if !metadata.file_type().is_file() || metadata.len() != offset {
            return Err(CasError::InvalidOffset {
                expected: metadata.len(),
                actual: offset,
            });
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| CasError::io("seek partial upload", &path, error))?;
        file.write_all(chunk)
            .map_err(|error| CasError::io("append partial upload", &path, error))?;
        file.sync_all()
            .map_err(|error| CasError::io("sync partial upload", &path, error))?;
        let next = offset
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| CasError::InvalidPath("upload length overflow".into()))?;
        if !finish {
            return Ok(UploadStatus::Incomplete { offset: next });
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|error| CasError::io("rewind partial upload", &path, error))?;
        let mut hash = Sha256::new();
        io::copy(&mut file, &mut HashWriter(&mut hash))
            .map_err(|error| CasError::io("hash partial upload", &path, error))?;
        let mut actual_bytes = [0_u8; 32];
        actual_bytes.copy_from_slice(&hash.finalize());
        let actual = Digest::from_bytes(actual_bytes);
        drop(file);
        if actual != expected {
            return Err(CasError::DigestMismatch { expected, actual });
        }
        self.commit_temporary(&path, expected)?;
        Ok(UploadStatus::Complete { digest: expected })
    }

    pub fn abort_upload(&self, expected: Digest) -> Result<(), CasError> {
        let path = self.upload_path(expected);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(CasError::io("remove partial upload", path, error)),
        }
    }

    fn upload_path(&self, expected: Digest) -> PathBuf {
        self.root.join("uploads").join(format!("{expected}.part"))
    }
    fn temporary_path(&self, prefix: &str) -> PathBuf {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        self.root
            .join("tmp")
            .join(format!("{prefix}-{}-{sequence}", std::process::id()))
    }
    fn create_temporary(&self, path: &Path) -> Result<File, CasError> {
        OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| CasError::io("create CAS temporary object", path, error))
    }
    fn commit_temporary(&self, temporary: &Path, digest: Digest) -> Result<(), CasError> {
        // Re-hash the durable bytes, rather than trusting a hash computed before the write.
        let mut file = File::open(temporary)
            .map_err(|error| CasError::io("open CAS temporary object", temporary, error))?;
        let mut hash = Sha256::new();
        io::copy(&mut file, &mut HashWriter(&mut hash))
            .map_err(|error| CasError::io("verify CAS temporary object", temporary, error))?;
        let mut bytes = [0_u8; 32];
        bytes.copy_from_slice(&hash.finalize());
        let actual = Digest::from_bytes(bytes);
        if actual != digest {
            return Err(CasError::DigestMismatch {
                expected: digest,
                actual,
            });
        }
        let mut permissions = fs::metadata(temporary)
            .map_err(|error| CasError::io("inspect CAS temporary object", temporary, error))?
            .permissions();
        permissions.set_readonly(true);
        fs::set_permissions(temporary, permissions)
            .map_err(|error| CasError::io("make CAS object immutable", temporary, error))?;
        let destination = self.object_path(digest);
        let parent = destination
            .parent()
            .expect("object paths always have a parent");
        fs::create_dir_all(parent)
            .map_err(|error| CasError::io("create CAS object shard", parent, error))?;
        let parent_metadata = fs::symlink_metadata(parent)
            .map_err(|error| CasError::io("inspect CAS object shard", parent, error))?;
        if !parent_metadata.file_type().is_dir() {
            return Err(CasError::UnsupportedEntry(parent.to_path_buf()));
        }
        match fs::hard_link(temporary, &destination) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                self.get(digest)?;
            }
            Err(error) => return Err(CasError::io("commit CAS object", &destination, error)),
        }
        fs::remove_file(temporary)
            .map_err(|error| CasError::io("remove CAS temporary object", temporary, error))?;
        sync_directory(parent)?;
        if let Some(temporary_parent) = temporary.parent() {
            sync_directory(temporary_parent)?;
        }
        Ok(())
    }
}

struct HashWriter<'a>(&'a mut Sha256);
impl Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn sync_directory(path: &Path) -> Result<(), CasError> {
    let file =
        File::open(path).map_err(|error| CasError::io("open directory for sync", path, error))?;
    file.sync_all()
        .map_err(|error| CasError::io("sync directory", path, error))
}
