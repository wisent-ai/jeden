//! The live extras on the status line, read from the workspace state and the
//! quota service and cached briefly.
//!
//! Split out of `tui/render/mod.rs`, which had grown past the module line cap.

use crate::tui::text::sanitize_terminal_text;
use crate::tui::PromptStatus;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Live status-line extras read from project state files (`.jeden/usage.json`
/// counters and `.jeden/mode-state.json`). Reads are cached for a few seconds
/// so per-keystroke renders stay cheap.
#[derive(Clone)]
pub(super) struct StatusExtras {
    /// Total tokens and recorded cost, or why the ledger cannot be read.
    usage: Result<(f64, f64), String>,
    plan: bool,
    goal: bool,
    loop_mode: bool,
    advisor: bool,
}

struct ExtrasCache {
    cwd: PathBuf,
    fetched_at: Instant,
    extras: StatusExtras,
}

static EXTRAS_CACHE: Mutex<Option<ExtrasCache>> = Mutex::new(None);
const EXTRAS_TTL: Duration = Duration::from_secs(3);

/// Total tokens and recorded cost across all events, added up by the reader
/// `/usage` uses over the same `.jeden/usage.json` file.
fn usage_totals(cwd: &Path) -> Result<(f64, f64), String> {
    let ledger = crate::slash::usage_ledger::read_ledger(&cwd.join(".jeden/usage.json"))?;
    let totals = crate::slash::usage_ledger::Agg::of(&ledger.events);
    Ok((totals.total, totals.cost.total))
}

/// A status cache is only ever replaced whole, so a panic while one was held
/// leaves nothing half-written: its lock is recovered instead of hiding the
/// segment for the rest of the session.
fn recovered<T>(cache: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match cache.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub(super) fn status_extras(cwd: &Path) -> Option<StatusExtras> {
    let mut guard = recovered(&EXTRAS_CACHE);
    let fresh = guard
        .as_ref()
        .is_some_and(|cache| cache.cwd == cwd && cache.fetched_at.elapsed() < EXTRAS_TTL);
    if !fresh {
        let usage = usage_totals(cwd);
        let state = crate::slash::read_mode_state(cwd);
        *guard = Some(ExtrasCache {
            cwd: cwd.to_path_buf(),
            fetched_at: Instant::now(),
            extras: StatusExtras {
                usage,
                plan: state.plan.enabled,
                goal: state.goal.enabled || state.guided_goal.active,
                loop_mode: state.loop_mode.enabled,
                advisor: state.advisor.enabled,
            },
        });
    }
    guard.as_ref().map(|cache| cache.extras.clone())
}

/// Live session spend: recorded cost when nonzero, otherwise the token form
/// (`~N tok`); nothing when no usage has been recorded yet, and `usage
/// unreadable` when the ledger cannot be read (`/usage` names its cause).
pub(super) fn cost_segment(status: &PromptStatus, extras: &StatusExtras) -> Option<String> {
    if let Some(cost) = &status.cost {
        return Some(format!("cost {}", sanitize_terminal_text(cost)));
    }
    let Ok((tokens, cost)) = extras.usage else {
        return Some("usage unreadable".into());
    };
    if cost > 0.0 {
        Some(format!("${cost:.2}"))
    } else if tokens > 0.0 {
        Some(format!("~{} tok", tokens.round() as u64))
    } else {
        None
    }
}

/// Compact badges for the active modes, ordered after the route segment.
pub(super) fn mode_badges(extras: &StatusExtras) -> Vec<String> {
    let mut badges = Vec::new();
    if extras.plan {
        badges.push("plan".to_string());
    }
    if extras.goal {
        badges.push("goal".to_string());
    }
    if extras.loop_mode {
        badges.push("loop".to_string());
    }
    if extras.advisor {
        badges.push("adv".to_string());
    }
    badges
}

struct QuotaCache {
    fetched_at: Instant,
    percent_free: Option<u64>,
    refreshing: bool,
}

static QUOTA_CACHE: Mutex<Option<QuotaCache>> = Mutex::new(None);
const QUOTA_TTL: Duration = Duration::from_secs(60);

/// Most-constrained Weles subscription quota (percent free), refreshed on a
/// detached thread so renders never block on the network. `None` until the
/// first refresh lands and whenever Weles is unconfigured or unreachable, so
/// the status line just skips the segment.
pub(super) fn quota_percent_free_cached() -> Option<u64> {
    let mut guard = recovered(&QUOTA_CACHE);
    let refresh = match guard.as_ref() {
        Some(cache) => cache.fetched_at.elapsed() >= QUOTA_TTL && !cache.refreshing,
        None => true,
    };
    if refresh {
        if let Some(cache) = guard.as_mut() {
            cache.refreshing = true;
        } else {
            *guard = Some(QuotaCache {
                fetched_at: Instant::now(),
                percent_free: None,
                refreshing: true,
            });
        }
        std::thread::spawn(|| {
            let percent_free =
                crate::control_plane::quota::fetch_subscription_quotas().min_percent_free();
            let mut guard = recovered(&QUOTA_CACHE);
            *guard = Some(QuotaCache {
                fetched_at: Instant::now(),
                percent_free,
                refreshing: false,
            });
        });
    }
    guard.as_ref().and_then(|cache| cache.percent_free)
}
