use serde_json::Value;

/// The relay client. A request runs until the relay answers or the
/// connection ends; `reqwest`'s own thirty-second default is turned off,
/// because a library's guess is still a guess.
fn client() -> Result<reqwest::blocking::Client, String> {
    crate::net::blocking_builder().build().map_err(|error| {
        format!(
            "the relay's HTTP client cannot be built: {}",
            crate::control_plane::transport::describe_reqwest(error)
        )
    })
}

/// One request to the relay, waited for through Stado's wait reporting; the
/// relay's answer text when it succeeded, its status and text when it did not.
fn answered(what: &str, request: reqwest::blocking::RequestBuilder) -> Result<Value, String> {
    let response =
        stado_wait::request_blocking(request).map_err(|e| format!("relay {what}: {e}"))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|e| format!("relay {what}: the answer could not be read: {e}"))?;
    if !status.is_success() {
        return Err(format!("relay {what} {}: {text}", status.as_u16()));
    }
    serde_json::from_str(&text).map_err(|e| format!("relay {what} answered no JSON ({e}): {text}"))
}

/// Posts one sealed blob to `room`, answering the sequence number the relay gave it.
pub fn relay_post_authorized(
    base: &str,
    room: &str,
    blob: &str,
    write_token: Option<&str>,
) -> Result<usize, String> {
    let url = format!("{}/room/{}", base.trim_end_matches('/'), room);
    let mut request = client()?
        .post(&url)
        .header("content-type", "text/plain")
        .header("x-jeden-role", super::CollabRole::Full.name())
        .body(blob.to_string());
    if let Some(token) = write_token {
        request = request.header("x-jeden-write-token", token);
    }
    answered("POST", request)?
        .get("seq")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .ok_or_else(|| "relay POST returned no seq".into())
}

/// Every event `room` holds, and the sequence of the last of them (none for
/// a room that holds no event yet).
pub fn relay_get(base: &str, room: &str) -> Result<(Vec<String>, Option<usize>), String> {
    let url = format!("{}/room/{}", base.trim_end_matches('/'), room);
    let value = answered("GET", client()?.get(&url))?;
    let events = value
        .get("events")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("relay GET answered no events list: {value}"))?
        .iter()
        .map(|event| {
            event
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("relay GET answered an event that is not text: {event}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let next = match value.get("next") {
        Some(serde_json::Value::Null) => None,
        Some(next) => Some(next.as_u64().ok_or_else(|| {
            format!("relay GET answered a next cursor that is not a sequence: {next}")
        })? as usize),
        None => return Err(format!("relay GET answered no next cursor: {value}")),
    };
    Ok((events, next))
}
