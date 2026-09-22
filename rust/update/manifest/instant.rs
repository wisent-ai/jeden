//! Reading the moment a release says it was issued, without trusting a date
//! library to be lenient about it.
//!
//! Split out of `update/manifest.rs`, which had grown past the module line cap.

pub(super) fn parse_rfc3339(value: &str) -> Result<u64, String> {
    if value.len() < 20
        || value.as_bytes().get(4) != Some(&b'-')
        || value.as_bytes().get(7) != Some(&b'-')
        || value.as_bytes().get(10) != Some(&b'T')
        || value.as_bytes().get(13) != Some(&b':')
        || value.as_bytes().get(16) != Some(&b':')
    {
        return Err(format!("timestamp is not RFC3339: {value}"));
    }
    let number = |range: std::ops::Range<usize>| {
        value
            .get(range)
            .and_then(|part| part.parse::<i64>().ok())
            .ok_or_else(|| format!("timestamp is not RFC3339: {value}"))
    };
    let year = number(0..4)?;
    let month = number(5..7)?;
    let day = number(8..10)?;
    let hour = number(11..13)?;
    let minute = number(14..16)?;
    let second = number(17..19)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(format!("timestamp is not RFC3339: {value}"));
    }
    let suffix = &value[19..];
    let offset = if suffix == "Z" {
        0
    } else if suffix.len() == 6
        && matches!(suffix.as_bytes()[0], b'+' | b'-')
        && suffix.as_bytes()[3] == b':'
    {
        let hours = suffix[1..3]
            .parse::<i64>()
            .map_err(|_| format!("timestamp is not RFC3339: {value}"))?;
        let minutes = suffix[4..6]
            .parse::<i64>()
            .map_err(|_| format!("timestamp is not RFC3339: {value}"))?;
        if hours > 23 || minutes > 59 {
            return Err(format!("timestamp is not RFC3339: {value}"));
        }
        let seconds = hours * 3600 + minutes * 60;
        if suffix.starts_with('-') {
            -seconds
        } else {
            seconds
        }
    } else {
        return Err(format!("timestamp is not RFC3339: {value}"));
    };
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let timestamp = days * 86_400 + hour * 3600 + minute * 60 + second - offset;
    u64::try_from(timestamp).map_err(|_| format!("timestamp predates Unix epoch: {value}"))
}
