use super::security::{ExecutionGrant, GrantError};
use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use url::Url;
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
    if !matches!(url.scheme(), "http" | "https") {
        return Err(GrantError::NetworkDenied(
            "only http/https are permitted".into(),
        ));
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
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            !(v.is_private()
                || v.is_loopback()
                || v.is_link_local()
                || v.is_broadcast()
                || v.is_documentation()
                || v.is_unspecified()
                || v.is_multicast()
                || v.octets()[0] == 0
                || v.octets()[0] >= 224
                || v.octets() == [100, 100, 100, 200]
                || v.octets() == [169, 254, 169, 254])
        }
        IpAddr::V6(v) => {
            !(v.is_loopback()
                || v.is_unspecified()
                || v.is_multicast()
                || v.is_unique_local()
                || v.is_unicast_link_local()
                || v.to_ipv4_mapped()
                    .is_some_and(|x| !is_public(IpAddr::V4(x))))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_and_loopback_are_denied_after_resolution() {
        let root = std::env::current_dir().unwrap();
        let mut grant = ExecutionGrant::trusted_host("test", root);
        grant.network.hosts = ["169.254.169.254".into(), "127.0.0.1".into()]
            .into_iter()
            .collect();
        grant.network.ports = [80].into_iter().collect();
        for value in [
            "http://169.254.169.254/latest/meta-data/",
            "http://127.0.0.1/",
        ] {
            let url = Url::parse(value).unwrap();
            assert!(authorize_url(&grant, &url)
                .unwrap_err()
                .to_string()
                .contains("non-public address rejected"));
        }
    }
}
