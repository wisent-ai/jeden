//! POSIX permission bits as the `u32` that `PermissionsExt::mode`,
//! `OpenOptionsExt::mode`, `DirBuilderExt::mode` and a tar header take and
//! answer.
//!
//! The C library's `S_I*` constants are `u32` on Linux and `u16` on macOS, so
//! converting them to `u32` is required on one platform and refused by clippy
//! as a useless conversion on the other. The bits are the same on both, so
//! every mode Jeden writes or checks is stated once here.

/// Owner read and write (`S_IRUSR | S_IWUSR`): every private file.
pub const OWNER_READ_WRITE: u32 = 0o600;
/// Owner read, write and search (`S_IRWXU`): every private directory.
pub const OWNER_ALL: u32 = 0o700;
/// Every group and other bit (`S_IRWXG | S_IRWXO`): a private file or
/// directory holds none of them.
pub const GROUP_AND_OTHER: u32 = 0o077;
/// Every execute bit (`S_IXUSR | S_IXGRP | S_IXOTH`).
pub const ANY_EXECUTE: u32 = 0o111;
/// Owner read and write, group and others read (`S_IRUSR | S_IWUSR |
/// S_IRGRP | S_IROTH`): a file its owner may change and anyone may read.
pub const OWNER_WRITE_ANYONE_READ: u32 = 0o644;
/// Owner everything, group and others read and execute (`S_IRWXU | S_IRGRP |
/// S_IXGRP | S_IROTH | S_IXOTH`): a program anyone may run.
pub const OWNER_WRITE_ANYONE_RUN: u32 = 0o755;
