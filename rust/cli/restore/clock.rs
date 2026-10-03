//! The instants `jeden restore` compares: the operator's `--since` and the
//! timestamp OMP writes on every transcript record, both as milliseconds
//! since the Unix epoch. The calendar arithmetic is the C library's
//! (`mktime` for the local day, `timegm` for UTC), not a copy of it here.

use crate::cli::invocation::refusal;

const MILLIS_PER_SECOND: i64 = 1000;

/// `today` is local midnight of the current day, `YYYY-MM-DD` local midnight
/// of that day, and `YYYY-MM-DDTHH:MM:SS[.fff]Z` that UTC instant. Anything
/// else is the caller's mistake, refused with the forms it may use.
pub(super) fn since(text: &str) -> Result<i64, String> {
    if text == "today" {
        let now = unsafe { libc::time(std::ptr::null_mut()) };
        let mut local: libc::tm = unsafe { std::mem::zeroed() };
        if unsafe { libc::localtime_r(&now, &mut local) }.is_null() {
            return Err("cannot read the local date for --since today".into());
        }
        return local_midnight(local.tm_year, local.tm_mon, local.tm_mday);
    }
    if let Some((year, month, day)) = date(text) {
        return local_midnight(year - TM_YEAR_BASE, month - 1, day);
    }
    utc(text).ok_or_else(|| {
        refusal::usage(format!(
            "--since takes today, a local date YYYY-MM-DD or a UTC instant YYYY-MM-DDTHH:MM:SSZ, not {text:?}"
        ))
    })
}

/// A record's `timestamp`, which OMP writes as `2026-10-02T21:08:21.909Z`.
pub(super) fn utc(text: &str) -> Option<i64> {
    let (day, time) = text.split_once('T')?;
    let (year, month, date) = date(day)?;
    let time = time.strip_suffix('Z')?;
    let (clock, fraction) = match time.split_once('.') {
        Some((clock, fraction)) => (clock, Some(fraction)),
        None => (time, None),
    };
    let mut parts = clock.split(':');
    let hour: i32 = parts.next()?.parse().ok()?;
    let minute: i32 = parts.next()?.parse().ok()?;
    let second: i32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let millis = match fraction {
        Some(digits) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => {
            // Milliseconds are the first three digits; a shorter fraction is
            // padded, so `.9` is 900 ms.
            let padded = format!("{digits:0<3}");
            padded[..3].parse::<i64>().ok()?
        }
        Some(_) => return None,
        None => 0,
    };
    let mut broken: libc::tm = unsafe { std::mem::zeroed() };
    broken.tm_year = year - TM_YEAR_BASE;
    broken.tm_mon = month - 1;
    broken.tm_mday = date;
    broken.tm_hour = hour;
    broken.tm_min = minute;
    broken.tm_sec = second;
    let seconds = unsafe { libc::timegm(&mut broken) };
    if seconds == -1 {
        return None;
    }
    Some(seconds as i64 * MILLIS_PER_SECOND + millis)
}

/// The same instant written back the way OMP writes it, for reports.
pub(crate) fn format_utc(millis: i64) -> String {
    let seconds = millis.div_euclid(MILLIS_PER_SECOND) as libc::time_t;
    let mut broken: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::gmtime_r(&seconds, &mut broken) };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        broken.tm_year + TM_YEAR_BASE,
        broken.tm_mon + 1,
        broken.tm_mday,
        broken.tm_hour,
        broken.tm_min,
        broken.tm_sec,
        millis.rem_euclid(MILLIS_PER_SECOND)
    )
}

/// `struct tm` counts years from 1900 (C standard, <time.h>).
const TM_YEAR_BASE: i32 = 1900;

fn date(text: &str) -> Option<(i32, i32, i32)> {
    let mut parts = text.split('-');
    let year: i32 = parts.next().filter(|part| part.len() == 4)?.parse().ok()?;
    let month: i32 = parts.next().filter(|part| part.len() == 2)?.parse().ok()?;
    let day: i32 = parts.next().filter(|part| part.len() == 2)?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((year, month, day))
}

fn local_midnight(tm_year: i32, tm_mon: i32, tm_mday: i32) -> Result<i64, String> {
    let mut broken: libc::tm = unsafe { std::mem::zeroed() };
    broken.tm_year = tm_year;
    broken.tm_mon = tm_mon;
    broken.tm_mday = tm_mday;
    // The C library decides whether daylight saving applies at that midnight.
    broken.tm_isdst = -1;
    let seconds = unsafe { libc::mktime(&mut broken) };
    if seconds == -1 {
        return Err("the local midnight for --since cannot be represented".into());
    }
    Ok(seconds as i64 * MILLIS_PER_SECOND)
}
