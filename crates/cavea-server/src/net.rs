//! The client's address (08 §12): the socket's peer, or, when the peer is a
//! proxy the operator declared in `CAVEA_TRUSTED_PROXIES`, the address that
//! proxy forwarded in `X-Forwarded-For`. A header from any other peer is
//! ignored, so a client cannot choose its own address.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use axum::http::HeaderMap;

/// A CIDR range, held as IPv6 bits: an IPv4 range is its IPv4-mapped form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    net: u128,
    len: u8,
}

/// `ip` as IPv6 bits, an IPv4 address (or an IPv4-mapped one) as `::ffff:a.b.c.d`.
fn bits(ip: IpAddr) -> u128 {
    match ip.to_canonical() {
        IpAddr::V4(v4) => u128::from(v4.to_ipv6_mapped()),
        IpAddr::V6(v6) => u128::from(v6),
    }
}

fn mask(len: u8) -> u128 {
    u128::MAX.checked_shl(128 - u32::from(len)).unwrap_or(0)
}

impl Cidr {
    #[must_use]
    pub fn contains(self, ip: IpAddr) -> bool {
        bits(ip) & mask(self.len) == self.net
    }
}

impl FromStr for Cidr {
    type Err = &'static str;

    /// `10.0.0.0/8`, `2001:db8::/32`, or a single address. Host bits are
    /// ignored.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        const ERROR: &str =
            "must be a comma-separated list of addresses or CIDR ranges such as 10.0.0.0/8";
        let (ip, len) = s.split_once('/').map_or((s, None), |(ip, l)| (ip, Some(l)));
        let ip: IpAddr = ip.parse().map_err(|_| ERROR)?;
        let (max, offset) = if ip.is_ipv4() { (32, 96) } else { (128, 0) };
        let len = match len {
            None => max,
            Some(len) => len.parse().ok().filter(|&l| l <= max).ok_or(ERROR)?,
        };
        let len = len + offset;
        Ok(Self {
            net: bits(ip) & mask(len),
            len,
        })
    }
}

/// `CAVEA_TRUSTED_PROXIES`: the proxies whose `X-Forwarded-For` is believed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustedProxies(Vec<Cidr>);

impl TrustedProxies {
    fn contains(&self, ip: IpAddr) -> bool {
        self.0.iter().any(|c| c.contains(ip))
    }
}

impl FromStr for TrustedProxies {
    type Err = &'static str;

    /// A comma-separated list; empty for none.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.trim().is_empty() {
            return Ok(Self::default());
        }
        s.split(',')
            .map(|c| c.trim().parse())
            .collect::<Result<_, _>>()
            .map(Self)
    }
}

/// The client's address. Behind a trusted peer, `X-Forwarded-For` is read
/// from the right (every line of it, in order, as one list), skipping trusted
/// hops: the first untrusted address is the client. A malformed entry stops
/// the walk at the nearest trusted hop, which is then the client, so a broken
/// chain is counted against the proxy rather than believed. If every hop is
/// trusted, the leftmost is the client. `Forwarded` is not read.
#[must_use]
pub fn client_addr(peer: IpAddr, headers: &HeaderMap, trusted: &TrustedProxies) -> IpAddr {
    let mut client = peer.to_canonical();
    if !trusted.contains(client) {
        return client;
    }
    let hops = headers
        .get_all("x-forwarded-for")
        .iter()
        .rev()
        .flat_map(|line| line.to_str().unwrap_or("").rsplit(','));
    for hop in hops {
        let Some(ip) = parse_hop(hop.trim()) else {
            break;
        };
        client = ip;
        if !trusted.contains(ip) {
            break;
        }
    }
    client
}

/// An address as proxies write it: bare, `[v6]` or `[v6]:port`, or `v4:port`.
fn parse_hop(hop: &str) -> Option<IpAddr> {
    let ip = if let Some(rest) = hop.strip_prefix('[') {
        IpAddr::V6(rest.split_once(']')?.0.parse::<Ipv6Addr>().ok()?)
    } else if let Ok(ip) = hop.parse() {
        ip
    } else {
        IpAddr::V4(hop.rsplit_once(':')?.0.parse::<Ipv4Addr>().ok()?)
    };
    Some(ip.to_canonical())
}

/// The address a client is counted by: an IPv4 address, or the /64 of an
/// IPv6 one, since one subscriber is routinely given a whole /64.
#[must_use]
pub fn group(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & mask(64))),
        v4 => v4,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]

    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn the_client_address_is_believed_only_from_trusted_proxies() {
        let trusted: TrustedProxies = "10.0.0.0/8, 2001:db8:1::/48, 192.0.2.7".parse().unwrap();
        // (peer, X-Forwarded-For lines, the client).
        let cases: &[(&str, &[&str], &str)] = &[
            // Direct, or behind a proxy that is not declared: the header is
            // ignored, however it is written.
            ("203.0.113.5", &[], "203.0.113.5"),
            ("203.0.113.5", &["198.51.100.1"], "203.0.113.5"),
            ("203.0.113.5", &["10.0.0.1"], "203.0.113.5"),
            ("::ffff:203.0.113.5", &["198.51.100.1"], "203.0.113.5"),
            // One trusted hop; the client may have prepended anything.
            ("10.1.2.3", &["198.51.100.1"], "198.51.100.1"),
            ("10.1.2.3", &["6.6.6.6, 198.51.100.1"], "198.51.100.1"),
            ("::ffff:10.1.2.3", &["198.51.100.1"], "198.51.100.1"),
            ("10.1.2.3", &["::ffff:198.51.100.1"], "198.51.100.1"),
            ("10.1.2.3", &[" 198.51.100.1:5123 "], "198.51.100.1"),
            ("10.1.2.3", &["[2001:db8:2::9]:443"], "2001:db8:2::9"),
            // Two trusted hops, in one line or two.
            (
                "10.1.2.3",
                &["6.6.6.6, 198.51.100.1, 192.0.2.7"],
                "198.51.100.1",
            ),
            (
                "2001:db8:1::5",
                &["6.6.6.6, 198.51.100.1", "10.9.9.9"],
                "198.51.100.1",
            ),
            // Every hop trusted: the leftmost.
            ("10.1.2.3", &["10.4.4.4, 192.0.2.7"], "10.4.4.4"),
            // No header behind a trusted peer: the peer.
            ("10.1.2.3", &[], "10.1.2.3"),
            // Malformed: the walk stops at the nearest trusted hop.
            ("10.1.2.3", &["198.51.100.1, unknown"], "10.1.2.3"),
            (
                "10.1.2.3",
                &["198.51.100.1, 192.0.2.7, garbage"],
                "10.1.2.3",
            ),
            ("10.1.2.3", &["198.51.100.1, , 192.0.2.7"], "192.0.2.7"),
            ("10.1.2.3", &["999.1.1.1"], "10.1.2.3"),
            ("10.1.2.3", &[""], "10.1.2.3"),
        ];
        for (peer, lines, client) in cases {
            let mut headers = HeaderMap::new();
            for line in *lines {
                headers.append("x-forwarded-for", HeaderValue::from_str(line).unwrap());
            }
            let chosen = client_addr(peer.parse().unwrap(), &headers, &trusted);
            assert_eq!(
                chosen,
                client.parse::<IpAddr>().unwrap(),
                "{peer} {lines:?}"
            );
        }
        // Not UTF-8.
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_bytes(b"\xff").unwrap());
        let peer = "10.1.2.3".parse().unwrap();
        assert_eq!(client_addr(peer, &headers, &trusted), peer);
    }

    #[test]
    fn ranges_parse_and_match() {
        let range = |s: &str| s.parse::<Cidr>().unwrap();
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert!(range("10.0.0.0/8").contains(ip("10.255.0.1")));
        assert!(range("10.0.0.0/8").contains(ip("::ffff:10.0.0.1")));
        assert!(!range("10.0.0.0/8").contains(ip("11.0.0.1")));
        assert!(range("0.0.0.0/0").contains(ip("1.2.3.4")));
        assert!(!range("0.0.0.0/0").contains(ip("2001:db8::1")));
        assert!(range("::/0").contains(ip("1.2.3.4")));
        assert!(range("2001:db8::/32").contains(ip("2001:db8:ffff::1")));
        assert!(!range("2001:db8::/32").contains(ip("2001:db9::1")));
        assert_eq!(range("10.1.2.3/8"), range("10.0.0.0/8"));
        assert!(range("192.0.2.7").contains(ip("192.0.2.7")));
        assert!(!range("192.0.2.7").contains(ip("192.0.2.8")));
        for bad in ["10.0.0.0/33", "::/129", "10.0.0/8", "10.0.0.0/", "x", ""] {
            assert!(bad.parse::<Cidr>().is_err(), "{bad}");
        }
        assert_eq!("".parse::<TrustedProxies>(), Ok(TrustedProxies::default()));
        assert!("10.0.0.0/8,".parse::<TrustedProxies>().is_err());
    }

    #[test]
    fn an_ipv6_client_is_counted_by_its_slash_64() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert_eq!(group(ip("2001:db8:1:2:aaaa::1")), ip("2001:db8:1:2::"));
        assert_eq!(group(ip("2001:db8:1:2:ffff::9")), ip("2001:db8:1:2::"));
        assert_ne!(group(ip("2001:db8:1:3::1")), ip("2001:db8:1:2::"));
        assert_eq!(group(ip("198.51.100.1")), ip("198.51.100.1"));
        assert_eq!(group(ip("::ffff:198.51.100.1")), ip("198.51.100.1"));
    }
}
