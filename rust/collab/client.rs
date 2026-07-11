use serde_json::Value;
use std::time::Duration;

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())
}

pub fn relay_post(base: &str, room: &str, blob: &str) -> Result<usize, String> {
    relay_post_authorized(base, room, blob, None)
}

pub fn relay_post_authorized(
    base: &str,
    room: &str,
    blob: &str,
    write_token: Option<&str>,
) -> Result<usize, String> {
    relay_post_role(base, room, blob, write_token, "full")
}

pub fn relay_post_role(
    base: &str,
    room: &str,
    blob: &str,
    write_token: Option<&str>,
    role: &str,
) -> Result<usize, String> {
    let url = format!("{}/room/{}", base.trim_end_matches('/'), room);
    let mut request = client()?
        .post(&url)
        .header("content-type", "text/plain")
        .header("x-jeden-role", role)
        .body(blob.to_string());
    if let Some(token) = write_token {
        request = request.header("x-jeden-write-token", token);
    }
    let response = request.send().map_err(|e| e.to_string())?;
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
        .ok_or_else(|| "relay POST returned no seq".into())
}

pub fn relay_rotate_write_token(
    base: &str,
    room: &str,
    old_token: &str,
    new_token: &str,
) -> Result<(), String> {
    let url = format!("{}/room/{}/token", base.trim_end_matches('/'), room);
    let response = client()?
        .put(&url)
        .header("content-type", "text/plain")
        .header("x-jeden-write-token", old_token)
        .body(new_token.to_string())
        .send()
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "relay token rotation {}: {}",
            status.as_u16(),
            text
        ));
    }
    Ok(())
}

pub fn relay_get(base: &str, room: &str, since: usize) -> Result<(Vec<String>, usize), String> {
    relay_get_role(base, room, since, None, None)
}

fn relay_get_role(
    base: &str,
    room: &str,
    since: usize,
    token: Option<&str>,
    role: Option<&str>,
) -> Result<(Vec<String>, usize), String> {
    let url = format!(
        "{}/room/{}?since={}",
        base.trim_end_matches('/'),
        room,
        since
    );
    let mut request = client()?.get(&url);
    if let Some(token) = token {
        request = request.header("x-jeden-write-token", token);
    }
    if let Some(role) = role {
        request = request.header("x-jeden-role", role);
    }
    let response = request.send().map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("relay GET {}: {}", status.as_u16(), text));
    }
    let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let events = value
        .get("events")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let next = value
        .get("next")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or_default();
    Ok((events, next))
}

#[derive(Debug, Clone)]
pub struct LiveClient {
    base: String,
    room: String,
    key: [u8; 32],
    write_token: Option<String>,
    pub role: super::CollabRole,
    pub client_id: String,
    cursor: usize,
}

impl LiveClient {
    pub fn new(
        base: impl Into<String>,
        room: impl Into<String>,
        key: [u8; 32],
        write_token: Option<String>,
        role: super::CollabRole,
        client_id: impl Into<String>,
    ) -> Self {
        Self {
            base: base.into(),
            room: room.into(),
            key,
            write_token,
            role,
            client_id: client_id.into(),
            cursor: 0,
        }
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    pub fn reconnect_from(&mut self, cursor: usize) {
        self.cursor = cursor;
    }
    pub fn publish(&mut self, kind: super::FrameKind) -> Result<usize, String> {
        let token = self
            .write_token
            .as_deref()
            .ok_or("this collaboration client has no write token")?;
        let frame = super::ProtocolFrame::new(self.client_id.clone(), self.role, kind)?;
        let blob = super::seal_frame(&self.key, &frame)?;
        let role = role_name(self.role);
        let seq = relay_post_role(&self.base, &self.room, &blob, Some(token), role)?;
        self.cursor = self.cursor.max(seq);
        Ok(seq)
    }
    pub fn replay(&mut self) -> Result<Vec<super::ProtocolFrame>, String> {
        let role = role_name(self.role);
        let (blobs, next) = relay_get_role(
            &self.base,
            &self.room,
            self.cursor,
            self.write_token.as_deref(),
            Some(role),
        )?;
        let frames = blobs
            .iter()
            .map(|blob| super::open_frame(&self.key, blob))
            .collect::<Result<Vec<_>, _>>()?;
        self.cursor = next;
        Ok(frames)
    }
    pub fn revoke_and_rotate(&mut self) -> Result<String, String> {
        if self.role == super::CollabRole::View {
            return Err("view role is read-only".into());
        }
        let old = self
            .write_token
            .as_deref()
            .ok_or("this collaboration client has no write token")?;
        let next = super::new_role_write_token(self.role);
        relay_rotate_write_token(&self.base, &self.room, old, &next)?;
        self.write_token = Some(next.clone());
        Ok(next)
    }
}

fn role_name(role: super::CollabRole) -> &'static str {
    match role {
        super::CollabRole::View => "view",
        super::CollabRole::Prompt => "prompt",
        super::CollabRole::Abort => "abort",
        super::CollabRole::Full => "full",
    }
}
