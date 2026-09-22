//! Getting the bytes of a release and its evidence from where the operator
//! said they are, over https or from a local path, without letting a download
//! run away.
//!
//! Split out of `update/mod.rs`, which had grown past the module line cap.

use serde_json::Value;
use std::path::Path;

fn github_auth_token(location: &str) -> Option<String> {
    let url = reqwest::Url::parse(location).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let path = url.path().to_ascii_lowercase();
    let authorized_path = match url.host_str() {
        Some("github.com") => path == "/wisent-ai/jeden" || path.starts_with("/wisent-ai/jeden/"),
        Some("api.github.com") => {
            path == "/repos/wisent-ai/jeden" || path.starts_with("/repos/wisent-ai/jeden/")
        }
        _ => false,
    };
    if !authorized_path {
        return None;
    }
    ["JEDEN_UPDATE_GITHUB_TOKEN", "GH_TOKEN"]
        .into_iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
}

fn safe_github_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn github_release_asset_coordinates(location: &str) -> Option<(String, String)> {
    let url = reqwest::Url::parse(location).ok()?;
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        return None;
    }
    let segments = url.path_segments()?.collect::<Vec<_>>();
    if segments.len() != 6
        || !segments[0].eq_ignore_ascii_case("wisent-ai")
        || !segments[1].eq_ignore_ascii_case("jeden")
        || segments[2] != "releases"
        || segments[3] != "download"
        || !safe_github_segment(segments[4])
        || !safe_github_segment(segments[5])
    {
        return None;
    }
    Some((segments[4].to_owned(), segments[5].to_owned()))
}

fn github_asset_api_url(metadata: &Value, asset_name: &str) -> Option<String> {
    let raw = metadata
        .get("assets")?
        .as_array()?
        .iter()
        .find(|asset| asset.get("name").and_then(Value::as_str) == Some(asset_name))?
        .get("url")?
        .as_str()?;
    let url = reqwest::Url::parse(raw).ok()?;
    let segments = url.path_segments()?.collect::<Vec<_>>();
    if url.scheme() != "https"
        || url.host_str() != Some("api.github.com")
        || segments.len() != 6
        || segments[0] != "repos"
        || !segments[1].eq_ignore_ascii_case("wisent-ai")
        || !segments[2].eq_ignore_ascii_case("jeden")
        || segments[3] != "releases"
        || segments[4] != "assets"
        || segments[5].is_empty()
        || !segments[5].bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some(url.to_string())
}

fn github_release_asset_request(
    client: &reqwest::blocking::Client,
    location: &str,
    token: &str,
) -> Result<Option<reqwest::blocking::RequestBuilder>, String> {
    let Some((tag, asset_name)) = github_release_asset_coordinates(location) else {
        return Ok(None);
    };
    let endpoint = format!("https://api.github.com/repos/wisent-ai/jeden/releases/tags/{tag}");
    let response = client
        .get(endpoint)
        .bearer_auth(token)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header(reqwest::header::USER_AGENT, "jeden-updater")
        .send()
        .map_err(|error| format!("resolve private GitHub release asset: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "resolve private GitHub release asset returned {}",
            response.status()
        ));
    }
    if response
        .content_length()
        .is_some_and(|size| size > 1024 * 1024)
    {
        return Err("private GitHub release metadata exceeds size limit".into());
    }
    let bytes = response.bytes().map_err(|error| error.to_string())?;
    if bytes.len() > 1024 * 1024 {
        return Err("private GitHub release metadata exceeds size limit".into());
    }
    let metadata: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid private GitHub release metadata: {error}"))?;
    let asset_url = github_asset_api_url(&metadata, &asset_name)
        .ok_or_else(|| format!("private GitHub release asset is unavailable: {asset_name}"))?;
    Ok(Some(
        client
            .get(asset_url)
            .bearer_auth(token)
            .header(reqwest::header::ACCEPT, "application/octet-stream")
            .header(reqwest::header::USER_AGENT, "jeden-updater"),
    ))
}

pub(super) fn fetch(location: &str, limit: usize) -> Result<Vec<u8>, String> {
    if location.starts_with("https://") {
        let client = crate::net::blocking_builder()
            .build()
            .map_err(|error| error.to_string())?;
        let mut request = client.get(location);
        if let Some(token) = github_auth_token(location) {
            request = github_release_asset_request(&client, location, &token)?
                .unwrap_or_else(|| client.get(location).bearer_auth(token));
        }
        let response = request
            .send()
            .map_err(|error| format!("download {location}: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "download {location} returned {}",
                response.status()
            ));
        }
        if response
            .content_length()
            .is_some_and(|size| size > limit as u64)
        {
            return Err(format!("download {location} exceeds size limit"));
        }
        let bytes = response.bytes().map_err(|error| error.to_string())?;
        if bytes.len() > limit {
            return Err(format!("download {location} exceeds size limit"));
        }
        return Ok(bytes.to_vec());
    }
    let path = location.strip_prefix("file://").unwrap_or(location);
    let metadata = std::fs::metadata(path).map_err(|error| format!("read {path}: {error}"))?;
    if metadata.len() > limit as u64 {
        return Err(format!("read {path}: file exceeds size limit"));
    }
    std::fs::read(path).map_err(|error| format!("read {path}: {error}"))
}

pub(super) fn resolve(base: &str, reference: &str) -> String {
    if reference.contains("://") || Path::new(reference).is_absolute() {
        return reference.into();
    }
    if base.starts_with("https://") {
        return reqwest::Url::parse(base)
            .ok()
            .and_then(|url| url.join(reference).ok())
            .map(|url| url.to_string())
            .unwrap_or_else(|| reference.into());
    }
    Path::new(base.strip_prefix("file://").unwrap_or(base))
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(reference)
        .display()
        .to_string()
}
