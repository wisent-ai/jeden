//! A network grant that does not allow private destinations refuses every
//! address that is not a public one, an IPv6 address that delivers to a
//! private IPv4 address included, and admits a public host.

use jeden::tool_runtime::runtime_ops::network::{authorize_endpoint, authorize_url};
use jeden::tool_runtime::runtime_ops::security::{ExecutionGrant, GrantError};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;
use url::Url;

/// A host under the reserved example domain, answered from real public
/// addresses: https://www.rfc-editor.org/rfc/rfc2606#section-3
const PUBLIC_HOST: &str = "example.com";

/// A grant for `hosts` on the port an `http` URL names by default, private
/// destinations refused.
fn grant(hosts: &[String]) -> (ExecutionGrant, u16) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut grant = ExecutionGrant::trusted_host("egress-journey", root);
    let port = Url::parse(&format!("http://{PUBLIC_HOST}/"))
        .expect("the example host forms a URL")
        .port_or_known_default()
        .expect("an http URL names its default port");
    grant.network.hosts = hosts.iter().cloned().collect();
    grant.network.ports = [port].into_iter().collect();
    grant.network.allow_private = false;
    (grant, port)
}

/// Asserts that `address` is refused as a non-public destination.
fn refused(address: IpAddr) {
    let host = address.to_string();
    let (grant, port) = grant(&[host.clone()]);
    match authorize_endpoint(&grant, &host, port) {
        Err(GrantError::NetworkDenied(reason)) => assert!(
            reason.contains("non-public address rejected"),
            "{host} was refused for another reason: {reason}"
        ),
        Err(other) => panic!("{host} was refused for another reason: {other}"),
        Ok(addresses) => panic!("{host} was admitted as {addresses:?}"),
    }
}

/// A private IPv4 address.
fn private_v4() -> Ipv4Addr {
    Ipv4Addr::new(10, 0, 0, 1) // https://www.rfc-editor.org/rfc/rfc1918#section-3
}

#[test]
fn nat64_address_carrying_a_private_ipv4_is_refused() {
    let [a, b, c, d] = private_v4().octets();
    // https://www.rfc-editor.org/rfc/rfc6052#section-2.1 : the well-known prefix, then the IPv4 address in the last 32 bits.
    let nat64 = Ipv6Addr::new(
        0x64,
        0xff9b,
        0,
        0,
        0,
        0,
        u16::from_be_bytes([a, b]),
        u16::from_be_bytes([c, d]),
    );
    refused(IpAddr::V6(nat64));
}

#[test]
fn ipv4_mapped_private_address_is_refused() {
    refused(IpAddr::V6(private_v4().to_ipv6_mapped()));
}

#[test]
fn six_to_four_address_is_refused() {
    let [a, b, c, d] = private_v4().octets();
    // https://www.rfc-editor.org/rfc/rfc3056#section-2 : the 6to4 prefix followed by the IPv4 address of the site.
    let six_to_four = Ipv6Addr::new(
        0x2002,
        u16::from_be_bytes([a, b]),
        u16::from_be_bytes([c, d]),
        0,
        0,
        0,
        0,
        1,
    );
    refused(IpAddr::V6(six_to_four));
}

#[test]
fn shared_address_space_is_refused_through_a_url() {
    // https://www.rfc-editor.org/rfc/rfc6598#section-7 : the first address of the shared address space block.
    let shared = Ipv4Addr::new(100, 64, 0, 1);
    let host = shared.to_string();
    let (grant, _) = grant(&[host.clone()]);
    let url = Url::parse(&format!("http://{host}/")).expect("an IPv4 host forms a URL");
    match authorize_url(&grant, &url) {
        Err(GrantError::NetworkDenied(reason)) => assert!(
            reason.contains("non-public address rejected"),
            "{url} was refused for another reason: {reason}"
        ),
        Err(other) => panic!("{url} was refused for another reason: {other}"),
        Ok(target) => panic!("{url} was admitted as {:?}", target.addresses),
    }
}

#[test]
fn public_host_is_admitted() {
    let (grant, _) = grant(&[PUBLIC_HOST.to_string()]);
    let url = Url::parse(&format!("http://{PUBLIC_HOST}/")).expect("the example host forms a URL");
    match authorize_url(&grant, &url) {
        Ok(target) => assert!(!target.addresses.is_empty(), "{url} resolved to no address"),
        Err(error) => panic!("{url} was refused: {error}"),
    }
}
