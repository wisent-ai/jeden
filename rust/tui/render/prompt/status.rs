//! The live extras on the status line, read from the workspace state and the
//! quota service and cached briefly.
//!
//! Split out of `tui/render/mod.rs`, which had grown past the module line cap.

use crate::tui::PromptStatus;
use crate::tui::text::sanitize_terminal_text;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Live status-line extras read from project state files (`.jeden/usage.json`
/// counters and `.jeden/mode-state.json`). Reads are cached for a few seconds
/// so per-keystroke renders stay cheap.
#[derive(Clone)]
pub(super) struct StatusExtras {
    tokens: f64,
    cost: f64,
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

/// Total tokens and recorded cost across all events, summed exactly like the
/// `/usage` report does over the same `.jeden/usage.json` file.
fn usage_totals(cwd: &Path) -> (f64, f64) {
    let usage: serde_json::Value = std::fs::read_to_string(cwd.join(".jeden/usage.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null);
    let events = usage
        .get("events")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut tokens = 0.0;
    let mut cost = 0.0;
    for event in events {
        let number = |key: &str| event.get(key).and_then(serde_json::Value::as_f64);
        tokens += event
            .get("totalTokens")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_else(|| {
                let direct = ["inputTokens", "outputTokens"]
                    .iter()
                    .filter_map(|key| number(key))
                    .sum::<f64>();
                let cache_read = number("cacheReadTokens")
                    .or_else(|| number("cacheRead"))
                    .unwrap_or_default();
                let cache_write = number("cacheWriteTokens")
                    .or_else(|| number("cacheWrite"))
                    .unwrap_or_default();
                direct + cache_read + cache_write
            });
        cost += event
            .pointer("/cost/total")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_else(|| {
                ["input", "output", "cacheRead", "cacheWrite"]
                    .iter()
                    .map(|key| {
                        event
                            .pointer(&format!("/cost/{key}"))
                            .and_then(serde_json::Value::as_f64)
                            .unwrap_or_default()
                    })
                    .sum::<f64>()
            });
    }
    (tokens, cost)
}

pub(super) fn status_extras(cwd: &Path) -> Option<StatusExtras> {
    let mut guard = EXTRAS_CACHE.lock().ok()?;
    let fresh = guard
        .as_ref()
        .is_some_and(|cache| cache.cwd == cwd && cache.fetched_at.elapsed() < EXTRAS_TTL);
    if !fresh {
        let (tokens, cost) = usage_totals(cwd);
        let state = crate::slash::read_mode_state(cwd);
        *guard = Some(ExtrasCache {
            cwd: cwd.to_path_buf(),
            fetched_at: Instant::now(),
            extras: StatusExtras {
                tokens,
                cost,
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
/// (`~N tok`); nothing when no usage has been recorded yet.
pub(super) fn cost_segment(status: &PromptStatus, extras: &StatusExtras) -> Option<String> {
    if let Some(cost) = &status.cost {
        return Some(format!("cost {}", sanitize_terminal_text(cost)));
    }
    if extras.cost > 0.0 {
        Some(format!("${:.2}", extras.cost))
    } else if extras.tokens > 0.0 {
        Some(format!("~{} tok", extras.tokens.round() as u64))
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
    let mut guard = QUOTA_CACHE.lock().ok()?;
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
            if let Ok(mut guard) = QUOTA_CACHE.lock() {
                *guard = Some(QuotaCache {
                    fetched_at: Instant::now(),
                    percent_free,
                    refreshing: false,
                });
            }
        });
    }
    guard.as_ref().and_then(|cache| cache.percent_free)
}
