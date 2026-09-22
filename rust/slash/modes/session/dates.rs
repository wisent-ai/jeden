//! Reading the moment a session started, and saying how long ago that was.
//!
//! Split out of `slash/modes/session.rs`, which had grown past the module line
//! cap.

pub(crate) fn started_epoch(started: &str) -> Option<u64> {
    let trimmed = started.trim();
    if let Ok(epoch) = trimmed.parse::<u64>() {
        return Some(epoch);
    }
    parse_rfc3339_epoch(trimmed)
}

/// Howard Hinnant's days-from-civil; no date crate is available here.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (i64::from(month) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Epoch seconds from `YYYY-MM-DDTHH:MM:SS[.frac](Z|±HH:MM)`; anything else
/// is unparseable and yields no age.
fn parse_rfc3339_epoch(value: &str) -> Option<u64> {
    let bytes = value.as_bytes();
    if bytes.len() < 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year: i64 = value.get(0..4)?.parse().ok()?;
    let month: u32 = value.get(5..7)?.parse().ok()?;
    let day: u32 = value.get(8..10)?.parse().ok()?;
    let hour: i64 = value.get(11..13)?.parse().ok()?;
    let minute: i64 = value.get(14..16)?.parse().ok()?;
    let second: i64 = value.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let rest = value[19..].trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
    let zone = rest.as_bytes();
    let offset: i64 = if rest.eq_ignore_ascii_case("z") || rest.is_empty() {
        0
    } else if zone.len() == 6 && (zone[0] == b'+' || zone[0] == b'-') && zone[3] == b':' {
        let sign = if zone[0] == b'-' { -1 } else { 1 };
        let hours: i64 = rest.get(1..3)?.parse().ok()?;
        let minutes: i64 = rest.get(4..6)?.parse().ok()?;
        sign * (hours * 3_600 + minutes * 60)
    } else {
        return None;
    };
    let epoch =
        days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second - offset;
    u64::try_from(epoch).ok()
}

pub(super) fn relative_age(started: &str) -> Option<String> {
    let epoch = started_epoch(started)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let elapsed = now.saturating_sub(epoch);
    Some(if elapsed < 60 {
        format!("{elapsed}s")
    } else if elapsed < 3_600 {
        format!("{}m", elapsed / 60)
    } else if elapsed < 48 * 3_600 {
        format!("{}h", elapsed / 3_600)
    } else {
        format!("{}d", elapsed / 86_400)
    })
}
