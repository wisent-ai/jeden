//! What an operator asks of a session once it exists: reading it out, sharing
//! it, starting background work from it, and the rows shown when one of those
//! is opened without arguments.

mod export;
mod jobs;
mod pickers;
mod share;

pub(crate) use export::{slash_session_export, slash_session_text};
pub(crate) use jobs::jobs_picker;
pub(crate) use jobs::{handle_jobs, handle_tan};
pub(crate) use pickers::{dump_picker, export_picker, omfg_picker, share_picker, tan_picker};
pub(crate) use share::{handle_omfg, handle_share};
