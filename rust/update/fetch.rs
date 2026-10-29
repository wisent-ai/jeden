//! Getting the bytes of a release and its evidence from where the operator
//! said they are, over https or from a local path, without letting a download
//! run away.
//!
//! Split out of `update/mod.rs`, which had grown past the module line cap.

use serde_json::Value;
use std::path::Path;

/// The variables a GitHub token for Jeden's own releases is read from, in order.
const TOKEN_VARIABLES: &[&str] = &["JEDEN_UPDATE_GITHUB_TOKEN", "GH_TOKEN"];

/// The GitHub token to send to `location`: only to Jeden's own repository on
/// github.com or its API, and only when a variable states one. A variable that
/// is set but cannot be read is refused rather than passed over.
fn github_auth_token(location: &str) -> Result<Option<String>, String> {
    // A location that is not a URL is not GitHub's, so it is sent no token.
    let url = match reqwest::Url::parse(location) {
        Ok(url) => url,
        Err(_not_github) => return Ok(None),
    };
    if url.scheme() != "https" {
        return Ok(None);
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
        return Ok(None);
    }
    for name in TOKEN_VARIABLES {
        match std::env::var(name) {
            Ok(value) if !value.trim().is_empty() => return Ok(Some(value.trim().to_owned())),
            Ok(_) | Err(std::env::VarError::NotPresent) => {}
            Err(error) => return Err(format!("{name} cannot be read: {error}")),
        }
    }
    Ok(None)
}

fn safe_github_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// The tag and asset name of a `github.com/wisent-ai/jeden/releases/download/<tag>/<asset>`
/// location; `None` for any other location.
fn github_release_asset_coordinates(location: &str) -> Option<(String, String)> {
    let url = match reqwest::Url::parse(location) {
        Ok(url) => url,
        Err(_not_a_release_url) => return None,
    };
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        return None;
    }
    let segments = url.path_segments()?.collect::<Vec<_>>();
    match segments.as_slice() {
        [owner, repository, "releases", "download", tag, asset]
            if owner.eq_ignore_ascii_case("wisent-ai")
                && repository.eq_ignore_ascii_case("jeden")
                && safe_github_segment(tag)
                && safe_github_segment(asset) =>
        {
            Some(((*tag).to_owned(), (*asset).to_owned()))
        }
        _ => None,
    }
}

/// The API address of `asset_name` in a release's metadata, when the metadata
/// lists it under Jeden's own repository.
fn github_asset_api_url(metadata: &Value, asset_name: &str) -> Option<String> {
    let raw = metadata
        .get("assets")?
        .as_array()?
        .iter()
        .find(|asset| asset.get("name").and_then(Value::as_str) == Some(asset_name))?
        .get("url")?
        .as_str()?;
    let url = match reqwest::Url::parse(raw) {
        Ok(url) => url,
        Err(_not_an_asset_url) => return None,
    };
    if url.scheme() != "https" || url.host_str() != Some("api.github.com") {
        return None;
    }
    let segments = url.path_segments()?.collect::<Vec<_>>();
    match segments.as_slice() {
        ["repos", owner, repository, "releases", "assets", id]
            if owner.eq_ignore_ascii_case("wisent-ai")
                && repository.eq_ignore_ascii_case("jeden")
                && !id.is_empty()
                && id.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            Some(url.to_string())
        }
        _ => None,
    }
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
    let response = stado_wait::request_blocking(
        client
            .get(endpoint)
            .bearer_auth(token)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header(reqwest::header::USER_AGENT, "jeden-updater"),
    )
    .map_err(|error| format!("resolve private GitHub release asset: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "resolve private GitHub release asset returned {}",
            response.status()
        ));
    }
    let bytes = response
        .bytes()
        .map_err(|error| format!("read private GitHub release metadata: {error}"))?;
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

/// The bytes at `location`. With `expected`, the signed manifest's size, an
/// answer that announces another length is refused before it is read; the
/// caller checks the length and digest of what arrived either way.
pub(super) fn fetch(location: &str, expected: Option<u64>) -> Result<Vec<u8>, String> {
    if location.starts_with("https://") {
        let client = crate::net::blocking_builder()
            .build()
            .map_err(|error| format!("build the download client: {error}"))?;
        // A token goes with the request only to Jeden's own repository; a
        // release asset there is fetched through the API that serves private ones.
        let request = match github_auth_token(location)? {
            Some(token) => match github_release_asset_request(&client, location, &token)? {
                Some(asset) => asset,
                None => client.get(location).bearer_auth(token),
            },
            None => client.get(location),
        };
        let response = stado_wait::request_blocking(request)
            .map_err(|error| format!("download {location}: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "download {location} returned {}",
                response.status()
            ));
        }
        if let Some((announced, expected)) = response.content_length().zip(expected) {
            if announced != expected {
                return Err(format!(
                    "download {location} announces {announced} bytes; the signed manifest names {expected}"
                ));
            }
        }
        let bytes = response
            .bytes()
            .map_err(|error| format!("read {location}: {error}"))?;
        return Ok(bytes.to_vec());
    }
    let path = local_path(location);
    std::fs::read(path).map_err(|error| format!("read {path}: {error}"))
}

/// A local location as a path: a `file://` URL's path, or the location itself.
fn local_path(location: &str) -> &str {
    match location.strip_prefix("file://") {
        Some(path) => path,
        None => location,
    }
}

/// `reference` read against `base`, the location it was found in: an absolute
/// reference stands as it is, a relative one is joined onto the base's URL or
/// directory. A reference an https base cannot join is refused naming both.
pub(super) fn resolve(base: &str, reference: &str) -> Result<String, String> {
    if reference.contains("://") || Path::new(reference).is_absolute() {
        return Ok(reference.into());
    }
    if base.starts_with("https://") {
        let url = reqwest::Url::parse(base).map_err(|error| {
            format!("{base} is not a URL to resolve {reference} against: {error}")
        })?;
        return url
            .join(reference)
            .map(|joined| joined.to_string())
            .map_err(|error| format!("{reference} cannot be joined onto {base}: {error}"));
    }
    let directory = match Path::new(local_path(base)).parent() {
        Some(directory) => directory,
        None => {
            return Err(format!(
                "{base} names no directory to resolve {reference} against"
            ))
        }
    };
    Ok(directory.join(reference).display().to_string())
}
