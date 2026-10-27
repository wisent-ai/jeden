//! Reading the moment a release says it was issued: an RFC 3339 instant with
//! its offset, as Unix seconds.
//!
//! Split out of `update/manifest.rs`, which had grown past the module line cap.

pub(super) fn parse_rfc3339(value: &str) -> Result<u64, String> {
    let instant = value
        .parse::<jiff::Timestamp>()
        .map_err(|error| format!("timestamp is not RFC3339: {value}: {error}"))?;
    u64::try_from(instant.as_second())
        .map_err(|error| format!("timestamp predates Unix epoch: {value}: {error}"))
}
