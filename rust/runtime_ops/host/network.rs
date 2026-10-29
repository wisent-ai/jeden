use super::super::security::{ExecutionGrant, GrantError};
use http::uri::Scheme;
use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use url::Url;
use wisent_public_destination::is_public;
#[derive(Clone, Debug)]
pub struct ResolvedTarget {
    pub url: Url,
    pub host: String,
    pub port: u16,
    pub addresses: BTreeSet<IpAddr>,
}
pub fn authorize_url(grant: &ExecutionGrant, url: &Url) -> Result<ResolvedTarget, GrantError> {
    if grant.is_expired() {
        return Err(GrantError::Expired);
    }
    let scheme = url.scheme();
    if scheme != Scheme::HTTP.as_str() && scheme != Scheme::HTTPS.as_str() {
        return Err(GrantError::NetworkDenied(format!(
            "scheme {scheme} is not permitted: only {} and {} are",
            Scheme::HTTP,
            Scheme::HTTPS
        )));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(GrantError::NetworkDenied("URL userinfo rejected".into()));
    }
    let host = url
        .host_str()
        .ok_or_else(|| GrantError::NetworkDenied("URL host required".into()))?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if !grant.network.hosts.contains(&host) {
        return Err(GrantError::NetworkDenied(format!(
            "host {host} is not granted"
        )));
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| GrantError::NetworkDenied("URL port required".into()))?;
    if !grant.network.ports.contains(&port) {
        return Err(GrantError::NetworkDenied(format!(
            "port {port} is not granted"
        )));
    }
    let addresses = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| GrantError::NetworkDenied(format!("DNS failed: {e}")))?
        .map(|v| v.ip())
        .collect::<BTreeSet<_>>();
    if addresses.is_empty() {
        return Err(GrantError::NetworkDenied(
            "DNS returned no addresses".into(),
        ));
    }
    if !grant.network.allow_private {
        if let Some(ip) = addresses.iter().find(|ip| !is_public(**ip)) {
            return Err(GrantError::NetworkDenied(format!(
                "non-public address rejected: {ip}"
            )));
        }
    }
    Ok(ResolvedTarget {
        url: url.clone(),
        host,
        port,
        addresses,
    })
}
pub fn authorize_endpoint(
    grant: &ExecutionGrant,
    host: &str,
    port: u16,
) -> Result<BTreeSet<IpAddr>, GrantError> {
    if grant.is_expired() {
        return Err(GrantError::Expired);
    }
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if !grant.network.hosts.contains(&host) {
        return Err(GrantError::NetworkDenied(format!(
            "host {host} is not granted"
        )));
    }
    if !grant.network.ports.contains(&port) {
        return Err(GrantError::NetworkDenied(format!(
            "port {port} is not granted"
        )));
    }
    let addresses = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| GrantError::NetworkDenied(format!("DNS failed: {e}")))?
        .map(|v| v.ip())
        .collect::<BTreeSet<_>>();
    if addresses.is_empty() {
        return Err(GrantError::NetworkDenied(
            "DNS returned no addresses".into(),
        ));
    }
    if !grant.network.allow_private {
        if let Some(ip) = addresses.iter().find(|ip| !is_public(**ip)) {
            return Err(GrantError::NetworkDenied(format!(
                "non-public address rejected: {ip}"
            )));
        }
    }
    Ok(addresses)
}
pub fn validate_redirect(
    grant: &ExecutionGrant,
    from: &ResolvedTarget,
    location: &str,
) -> Result<ResolvedTarget, GrantError> {
    let next = from
        .url
        .join(location)
        .map_err(|e| GrantError::NetworkDenied(format!("invalid redirect: {e}")))?;
    authorize_url(grant, &next)
}
pub fn pinned_socket(target: &ResolvedTarget) -> Result<SocketAddr, GrantError> {
    target
        .addresses
        .iter()
        .next()
        .copied()
        .map(|ip| SocketAddr::new(ip, target.port))
        .ok_or_else(|| GrantError::NetworkDenied("no pinned address".into()))
}
