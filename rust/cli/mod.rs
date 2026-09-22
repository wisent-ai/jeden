//! CLI subtree: shared helpers and submodule declarations extracted from main.rs.

pub(crate) mod auth;
pub(crate) mod clipboard;
pub(crate) mod commands;
pub(crate) mod config;
pub(crate) mod context;
pub(crate) mod i18n;
pub(crate) mod invocation;
mod reports;
pub(crate) mod run;
pub(crate) mod tooling;
pub(crate) mod workspace;
pub(crate) mod worktree;

pub(crate) use reports::{billing, contracts, sessions, stats};
pub(crate) use tooling::{completions, gallery, token};
