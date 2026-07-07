use serde_json::Value;
use std::time::Duration;

/// POST one opaque blob to `base/room/<room>`; returns the new sequence length.
pub fn relay_post(base: &str, room: &str, blob: &str) -> Result<usize, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("{}/room/{}", base.trim_end_matches('/'), room);
    let response = client
        .post(&url)
        .header("content-type", "text/plain")
        .body(blob.to_string())
        .send()
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("relay POST {}: {}", status.as_u16(), text));
    }
    let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    value
        .get("seq")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .ok_or_else(|| "relay POST returned no seq".to_string())
}

/// GET blobs from `base/room/<room>` at index >= `since`; returns `(blobs, next)`.
pub fn relay_get(base: &str, room: &str, since: usize) -> Result<(Vec<String>, usize), String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("{}/room/{}?since={}", base.trim_end_matches('/'), room, since);
    let response = client.get(&url).send().map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("relay GET {}: {}", status.as_u16(), text));
    }
    let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let events = value
        .get("events")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let next = value.get("next").and_then(Value::as_u64).map(|n| n as usize).unwrap_or(0);
    Ok((events, next))
}
