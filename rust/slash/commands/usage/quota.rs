//! Showing what is left of each subscription quota, read live rather than
//! from a local copy that may be stale.
//!
//! Split out of `slash/commands/usage.rs`, which had grown past the module
//! line cap.

use crate::control_plane::billing::QuotaBucket;
use crate::control_plane::billing::QuotaState;
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

/// ` · resets in <duration>` when the bucket reports a reset timestamp,
/// counted to the minute.
fn reset_suffix(now: u64, resets_at_ms: Option<u64>) -> String {
    let Some(reset) = resets_at_ms else {
        return String::new();
    };
    let Some(ahead) = std::num::NonZeroU64::new(reset.saturating_sub(now)) else {
        return " · resets soon".into();
    };
    let counted = i64::try_from(ahead.get())
        .map_err(|error| error.to_string())
        .and_then(|millis| {
            jiff::SignedDuration::from_millis(millis)
                .round(jiff::Unit::Minute)
                .map_err(|error| error.to_string())
        });
    match counted {
        Ok(duration) if duration.is_zero() => " · resets soon".into(),
        Ok(duration) => format!(" · resets in {duration:#}"),
        Err(error) => format!(" · reset time cannot be counted: {error}"),
    }
}

/// Badge (percent free) and amount text for one bucket; buckets without a
/// limit render `unmetered`.
fn bucket_amount(bucket: &QuotaBucket) -> (Option<String>, String) {
    match (
        bucket.remaining,
        bucket.limit.and_then(std::num::NonZeroU64::new),
    ) {
        (Some(remaining), Some(limit)) => (
            Some(format!("{}%", percent_free(remaining, limit.get()))),
            format!("{remaining}/{limit} remaining"),
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
    // Reset times are counted from now; a clock that cannot be read says so
    // in the one row, as an unavailable quota does.
    let now = match crate::clock::unix_millis() {
        Ok(now) => now,
        Err(error) => {
            return vec![display_row(format!(
                "quota reset times cannot be counted: {error}"
            ))];
        }
    };
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
            (left.remaining.is_none(), left.remaining, &left.label).cmp(&(
                right.remaining.is_none(),
                right.remaining,
                &right.label,
            ))
        });
        let exhausted = rows.iter().any(|row| row.exhausted);
        let mut header = display_row(format!("── {provider} ({}) ──", rows.len()));
        header.badge = Some(if exhausted { "✖" } else { "✔" }.into());
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
