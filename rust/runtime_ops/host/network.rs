use super::super::security::{ExecutionGrant, GrantError};
use http::uri::Scheme;
use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
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
/// Special-purpose IPv4 blocks no public destination lives in that the
/// stable standard library does not name, each with its prefix length. Cloud
/// metadata endpoints fall inside them or inside link-local space, so none
/// is named alone.
// https://www.iana.org/assignments/iana-ipv4-special-registry/iana-ipv4-special-registry.xhtml
const NON_GLOBAL_V4: &[(Ipv4Addr, u32)] = &[
    (Ipv4Addr::new(0, 0, 0, 0), 8), // "this network": https://www.rfc-editor.org/rfc/rfc791#section-3.2
    (Ipv4Addr::new(100, 64, 0, 0), 10), // shared address space: https://www.rfc-editor.org/rfc/rfc6598
    (Ipv4Addr::new(192, 0, 0, 0), 24), // IETF protocol assignments: https://www.rfc-editor.org/rfc/rfc6890
    (Ipv4Addr::new(192, 88, 99, 0), 24), // 6to4 relay anycast: https://www.rfc-editor.org/rfc/rfc7526
    (Ipv4Addr::new(198, 18, 0, 0), 15), // benchmarking: https://www.rfc-editor.org/rfc/rfc2544
    (Ipv4Addr::new(240, 0, 0, 0), 4), // reserved: https://www.rfc-editor.org/rfc/rfc1112#section-4
];

/// Global unicast IPv6 space: an address outside it (loopback, unspecified,
/// unique-local, link-local, multicast, discard-only, translation) is no
/// public destination.
// https://www.rfc-editor.org/rfc/rfc4291#section-2.4
const GLOBAL_UNICAST_V6: (Ipv6Addr, u32) = (Ipv6Addr::new(0x2000, 0, 0, 0, 0, 0, 0, 0), 3);

/// Special-purpose blocks inside global unicast IPv6 space.
// https://www.iana.org/assignments/iana-ipv6-special-registry/iana-ipv6-special-registry.xhtml
const NON_GLOBAL_V6: &[(Ipv6Addr, u32)] = &[
    (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23), // IETF protocol assignments: https://www.rfc-editor.org/rfc/rfc2928
    (Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0), 32), // documentation: https://www.rfc-editor.org/rfc/rfc3849
    (Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16), // 6to4: https://www.rfc-editor.org/rfc/rfc3056
    (Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0), 20), // documentation: https://www.rfc-editor.org/rfc/rfc9637
];

/// The well-known prefix a NAT64 translator delivers to the IPv4 address in
/// an address's trailing octets.
// https://www.rfc-editor.org/rfc/rfc6052#section-2.1
const NAT64_WELL_KNOWN: (Ipv6Addr, u32) = (Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, 0, 0), 96);

/// Whether `ip` shares its first `prefix` bits with `network`.
fn within_v4(ip: Ipv4Addr, (network, prefix): (Ipv4Addr, u32)) -> bool {
    (u32::from(ip) ^ u32::from(network)).leading_zeros() >= prefix
}

/// `within_v4` for IPv6.
fn within_v6(ip: Ipv6Addr, (network, prefix): (Ipv6Addr, u32)) -> bool {
    (u128::from(ip) ^ u128::from(network)).leading_zeros() >= prefix
}

/// The IPv4 address an IPv6 address is delivered to: the one an IPv4-mapped
/// address carries, or the one in the trailing octets of an address under
/// the NAT64 well-known prefix. A 6to4 address is refused whole by
/// `NON_GLOBAL_V6` instead.
fn carried_v4(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    if let Some(mapped) = v6.to_ipv4_mapped() {
        return Some(mapped);
    }
    if within_v6(v6, NAT64_WELL_KNOWN) {
        let [.., a, b, c, d] = v6.octets();
        return Some(Ipv4Addr::new(a, b, c, d));
    }
    None
}

/// Whether a destination is a public address. IPv4: none the standard
/// library names private, loopback, link-local, broadcast, documentation,
/// unspecified or multicast, and none inside a block of `NON_GLOBAL_V4`.
/// IPv6: an address that carries an IPv4 address (`carried_v4`) is judged
/// as that IPv4 address, since that is where it is delivered; any other is
/// public when it is inside global unicast space and inside no block of
/// `NON_GLOBAL_V6`.
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
                || NON_GLOBAL_V4.iter().any(|&block| within_v4(v, block)))
        }
        IpAddr::V6(v) => match carried_v4(v) {
            Some(carried) => is_public(IpAddr::V4(carried)),
            None => {
                within_v6(v, GLOBAL_UNICAST_V6)
                    && !NON_GLOBAL_V6.iter().any(|&block| within_v6(v, block))
            }
        },
    }
}
