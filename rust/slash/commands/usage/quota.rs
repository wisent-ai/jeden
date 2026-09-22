//! Showing what is left of each subscription quota, read live rather than
//! from a local copy that may be stale.
//!
//! Split out of `slash/commands/usage.rs`, which had grown past the module
//! line cap.

use crate::control_plane::billing::QuotaBucket;
use crate::control_plane::billing::QuotaState;
use crate::control_plane::now_ms;
use crate::control_plane::quota::{
    fetch_subscription_quotas, percent_free, QuotaEntry, SubscriptionQuotas,
};
use crate::tui::PickerItem;
use std::collections::BTreeMap;

/// One display row derived from a Weles quota bucket.
struct QuotaRow {
    label: String,
    badge: Option<String>,
    detail: String,
    remaining: Option<u64>,
    exhausted: bool,
}

/// Non-action display row; the picker skips disabled, command-less rows.
pub(super) fn display_row(label: impl Into<String>) -> PickerItem {
    let mut item = PickerItem::action(label, "");
    item.command = None;
    item.disabled = true;
    item
}

/// Compact 20-cell bar: `█` cells for the used share, `░` for the free share.
fn quota_bar(remaining: u64, limit: u64) -> String {
    const CELLS: u128 = 20;
    let free =
        ((u128::from(remaining) * CELLS + u128::from(limit) / 2) / u128::from(limit)) as usize;
    let free = free.min(CELLS as usize);
    format!("{}{}", "█".repeat(CELLS as usize - free), "░".repeat(free))
}

/// ` · resets in <duration>` when the bucket reports a reset timestamp.
fn reset_suffix(now: u64, resets_at_ms: Option<u64>) -> String {
    let Some(reset) = resets_at_ms else {
        return String::new();
    };
    let delta = reset.saturating_sub(now);
    if delta == 0 {
        return " · resets soon".into();
    }
    let minutes = (delta / 60_000).max(1);
    let text = if minutes >= 1_440 {
        format!("{}d{}h", minutes / 1_440, minutes % 1_440 / 60)
    } else if minutes >= 60 {
        format!("{}h", minutes / 60)
    } else {
        format!("{minutes}m")
    };
    format!(" · resets in {text}")
}

/// Badge (percent free) and amount text for one bucket; buckets without a
/// limit render `unmetered` instead of a bar.
fn bucket_amount(bucket: &QuotaBucket) -> (Option<String>, String) {
    match (bucket.remaining, bucket.limit) {
        (Some(remaining), Some(limit)) if limit > 0 => (
            Some(format!("{}%", percent_free(remaining, limit))),
            format!(
                "{} {remaining}/{limit} remaining",
                quota_bar(remaining, limit)
            ),
        ),
        (Some(remaining), _) => (None, format!("unmetered · {remaining} remaining")),
        (None, Some(limit)) => (None, format!("limit {limit}")),
        (None, None) if bucket.state == QuotaState::Unmetered => (None, "unmetered".into()),
        (None, None) => (None, "amount not reported".into()),
    }
}

fn unavailable_row(label: String, error: impl std::fmt::Display) -> QuotaRow {
    QuotaRow {
        label,
        badge: None,
        detail: format!("quota unavailable: {error}"),
        remaining: None,
        exhausted: false,
    }
}

/// Live quota rows from the shared Weles subscription quota fetch (same
/// client path as `/login`). Any health/account-level failure collapses into
/// a single disabled info row so the local accounting actions below always
/// render.
pub(super) fn quota_rows() -> Vec<PickerItem> {
    let accounts = match fetch_subscription_quotas() {
        SubscriptionQuotas::Unavailable(reason) => {
            return vec![display_row(format!("quota unavailable: {reason}"))];
        }
        SubscriptionQuotas::Accounts(accounts) => accounts,
    };
    let now = now_ms();
    let mut groups: BTreeMap<String, Vec<QuotaRow>> = BTreeMap::new();
    for account in accounts {
        for entry in account.entries {
            let row = match entry {
                QuotaEntry::Bucket(labeled) => {
                    let (badge, amount) = bucket_amount(&labeled.bucket);
                    QuotaRow {
                        label: labeled.label,
                        badge,
                        detail: format!(
                            "{amount}{}",
                            reset_suffix(now, labeled.bucket.resets_at_ms)
                        ),
                        remaining: labeled.bucket.remaining,
                        exhausted: labeled.bucket.state == QuotaState::Exhausted,
                    }
                }
                QuotaEntry::Unavailable { label, error } => unavailable_row(label, error),
            };
            groups
                .entry(account.provider.clone())
                .or_default()
                .push(row);
        }
    }
    let mut items = Vec::new();
    for (provider, mut rows) in groups {
        // Most constrained subscription first; unreported amounts sort last.
        rows.sort_by(|left, right| {
            left.remaining
                .unwrap_or(u64::MAX)
                .cmp(&right.remaining.unwrap_or(u64::MAX))
                .then_with(|| left.label.cmp(&right.label))
        });
        let exhausted = rows.iter().any(|row| row.exhausted);
        let mut header = display_row(format!("── {provider} ({}) ──", rows.len()));
        header.badge = Some(if exhausted {
            "✖".into()
        } else {
            "✔".into()
        });
        items.push(header);
        items.extend(rows.into_iter().map(|row| {
            let mut item = display_row(row.label).detail(row.detail);
            if let Some(badge) = row.badge {
                item = item.badge(badge);
            }
            item
        }));
    }
    if items.is_empty() {
        items.push(display_row("quota: no subscription quota reported"));
    }
    items
}
