//! # Trusted-proxy client identity
//!
//! Behind a reverse proxy every request arrives from the proxy, so the transport peer names the
//! proxy rather than the caller. The forwarding headers are the only record of who called, and a
//! client can write them too — so a header is believed only when the hop that delivered it is a
//! proxy the operator named.
//!
//! The rule is applied right to left: the peer is the last hop, and each entry read is believed
//! only while the address to its right is a trusted proxy. The first address that is not one is
//! the client, and the walk stops there. A client that writes its own entry at the left of the
//! chain therefore cannot name a bucket: the honest proxy's entry to its right decides first.
//!
//! An entry that is not an address — a bare port, an obfuscated `for=_hidden`, `unknown`, an
//! unterminated quote — stops the walk rather than being skipped. Skipping would let an attacker
//! put one malformed element in the middle of the chain and have everything to its left believed.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The most hops examined in one chain. Far more than any real deployment, and the bound that
/// keeps a pathological header from turning one request into an unbounded walk.
const MAX_HOPS: usize = 32;

/// The caller's address, resolved through the configured trusted proxies.
///
/// Inserted into the request's extensions when a proxy list is configured; without one there is
/// nothing to resolve, and [`crate::RequestContext::client_addr`] reports the transport peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientAddr(pub IpAddr);

/// A list of networks whose forwarding headers are believed.
#[derive(Debug, Clone)]
pub struct TrustedProxies {
    nets: Vec<Cidr>,
}

impl TrustedProxies {
    /// Build the list from CIDR blocks or single addresses (`"10.0.0.0/8"`, `"::1"`,
    /// `"2001:db8::/32"`).
    ///
    /// An empty list and a `/0` block are refused: both describe "believe everyone", which is
    /// exactly the configuration that makes a forwarded identity spoofable. Leaving the feature
    /// unconfigured is how an app says it has no proxy in front of it.
    pub fn new<I, S>(cidrs: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut nets = Vec::new();
        for text in cidrs {
            nets.push(Cidr::parse(text.as_ref())?);
        }
        if nets.is_empty() {
            return Err(
                "the trusted-proxy list is empty; leave the feature unconfigured instead".into(),
            );
        }
        Ok(Self { nets })
    }

    /// Whether `addr` is one of the configured proxies.
    pub fn trusts(&self, addr: IpAddr) -> bool {
        self.nets.iter().any(|net| net.contains(addr))
    }

    /// The caller's address, given the transport peer and the two forwarding fields.
    ///
    /// `Forwarded` (RFC 7239) wins when both are present: the two fields do not share a
    /// semantics for "what came before this hop", and reading one while the other disagrees is
    /// how a proxy chain ends up describing two different clients.
    pub fn client_ip(
        &self,
        peer: IpAddr,
        forwarded: Option<&str>,
        x_forwarded_for: Option<&str>,
    ) -> IpAddr {
        if !self.trusts(peer) {
            return peer;
        }
        let chain = match forwarded.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => parse_forwarded(value),
            None => match x_forwarded_for
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                Some(value) => parse_x_forwarded_for(value),
                None => Vec::new(),
            },
        };
        let mut current = peer;
        for entry in chain.iter().rev() {
            // `None` is a malformed hop: everything to its left is unverified.
            let Some(addr) = entry else { break };
            current = *addr;
            if !self.trusts(*addr) {
                break;
            }
        }
        current
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cidr {
    base: IpAddr,
    prefix: u8,
}

impl Cidr {
    fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (addr_text, prefix_text) = match text.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (text, None),
        };
        if addr_text.is_empty() {
            return Err(format!("{text:?} names no address"));
        }
        let raw: IpAddr = addr_text
            .parse()
            .map_err(|_| format!("{addr_text:?} is not an IP address"))?;
        let mapped = matches!(raw, IpAddr::V6(v6) if v6.to_ipv4_mapped().is_some());
        let addr = canonical(raw);
        let max = match addr {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        let prefix = match prefix_text {
            Some(prefix) => {
                let prefix = prefix
                    .trim()
                    .parse::<u8>()
                    .map_err(|_| format!("{text:?} has no valid prefix length"))?;
                // A mapped address is an IPv4 block written in IPv6 punctuation: `::ffff:a/108`
                // is `a/12`, and a prefix shorter than the 96-bit mapping covers more than IPv4.
                if mapped {
                    if prefix < 96 {
                        return Err(format!(
                            "{text:?} maps IPv4 into IPv6, so its prefix must be /96 or longer"
                        ));
                    }
                    prefix - 96
                } else {
                    prefix
                }
            }
            None => max,
        };
        if prefix == 0 {
            return Err(format!(
                "{text:?} is a /0 block, which trusts every peer: a forwarded identity is then \
                 nothing but the client's own claim"
            ));
        }
        if prefix > max {
            return Err(format!("{text:?} has a prefix longer than {max}"));
        }
        Ok(Self { base: addr, prefix })
    }

    fn contains(&self, addr: IpAddr) -> bool {
        match (self.base, canonical(addr)) {
            (IpAddr::V4(base), IpAddr::V4(addr)) => {
                let mask = u32::MAX << (32 - u32::from(self.prefix));
                (u32::from(base) & mask) == (u32::from(addr) & mask)
            }
            (IpAddr::V6(base), IpAddr::V6(addr)) => {
                let mask = u128::MAX << (128 - u32::from(self.prefix));
                (u128::from(base) & mask) == (u128::from(addr) & mask)
            }
            _ => false,
        }
    }
}

/// An IPv4-mapped IPv6 address names the IPv4 peer, so it must compare as one: a dual-stack
/// socket reports `::ffff:127.0.0.1` for a loopback client, and a `127.0.0.1/32` entry would
/// otherwise not trust it.
fn canonical(addr: IpAddr) -> IpAddr {
    match addr {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        IpAddr::V4(_) => addr,
    }
}

/// `Forwarded` elements, in the order written, capped at the last [`MAX_HOPS`].
fn parse_forwarded(value: &str) -> Vec<Option<IpAddr>> {
    let mut chain: Vec<Option<IpAddr>> = Vec::new();
    scan_unquoted(value, b',', &mut |element| {
        if chain.len() == MAX_HOPS {
            chain.remove(0);
        }
        chain.push(for_parameter(element).and_then(parse_hop));
    });
    chain
}

/// `X-Forwarded-For` elements, in the order written, capped at the last [`MAX_HOPS`].
fn parse_x_forwarded_for(value: &str) -> Vec<Option<IpAddr>> {
    // Scanned from the right and reversed: only the last `MAX_HOPS` entries are ever read, so
    // the cap is applied before the vector exists rather than after.
    let mut chain: Vec<Option<IpAddr>> = value
        .rsplit(',')
        .take(MAX_HOPS)
        .map(|element| parse_hop(element.trim()))
        .collect();
    chain.reverse();
    chain
}

/// Call `f` with each `element` of `value` split on `sep`, ignoring separators inside a
/// quoted string.
fn scan_unquoted<'a>(value: &'a str, sep: u8, f: &mut dyn FnMut(&'a str)) {
    let bytes = value.as_bytes();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, &byte) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if quoted => escaped = true,
            b'"' => quoted = !quoted,
            byte if byte == sep && !quoted => {
                f(&value[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    f(&value[start..]);
}

/// The `for` parameter of one `Forwarded` element, without its quotes.
fn for_parameter(element: &str) -> Option<&str> {
    let mut found = None;
    scan_unquoted(element, b';', &mut |parameter| {
        let Some((name, value)) = parameter.split_once('=') else {
            return;
        };
        if name.trim().eq_ignore_ascii_case("for") {
            found = unquote(value.trim());
        }
    });
    found
}

/// Strip the quotes RFC 7239 allows around a parameter value; `None` when a quote is unbalanced.
fn unquote(raw: &str) -> Option<&str> {
    match raw.strip_prefix('"') {
        Some(rest) => rest.strip_suffix('"'),
        None if raw.contains('"') => None,
        None => Some(raw),
    }
}

/// One hop as an address: a bare address, `addr:port`, or `[v6]:port`.
fn parse_hop(raw: &str) -> Option<IpAddr> {
    let value = unquote(raw.trim())?.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(addr) = value.parse::<IpAddr>() {
        return Some(canonical(addr));
    }
    if let Some(rest) = value.strip_prefix('[') {
        let (inside, after) = rest.split_once(']')?;
        if !(after.is_empty() || after.starts_with(':')) {
            return None;
        }
        return inside.parse::<Ipv6Addr>().ok().map(IpAddr::V6);
    }
    let (host, port) = value.rsplit_once(':')?;
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if let Ok(v4) = host.parse::<Ipv4Addr>() {
        return Some(IpAddr::V4(v4));
    }
    host.parse::<Ipv6Addr>()
        .ok()
        .map(|v6| canonical(IpAddr::V6(v6)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxies(cidrs: &[&str]) -> TrustedProxies {
        TrustedProxies::new(cidrs).expect("valid proxy list")
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().expect("valid address")
    }

    #[test]
    fn a_cidr_block_and_a_bare_address_both_parse() {
        let list = proxies(&[
            "10.0.0.0/8",
            "192.168.1.5",
            "2001:db8::/32",
            "::1",
            "::ffff:172.16.0.0/108",
        ]);
        assert!(list.trusts(ip("10.1.2.3")));
        assert!(!list.trusts(ip("11.0.0.1")));
        assert!(list.trusts(ip("192.168.1.5")));
        assert!(!list.trusts(ip("192.168.1.6")));
        assert!(list.trusts(ip("2001:db8::99")));
        assert!(!list.trusts(ip("2001:db9::1")));
        assert!(list.trusts(ip("::1")));
        // A mapped address names the IPv4 peer and compares as one.
        assert!(list.trusts(ip("::ffff:172.16.5.5")));
        assert!(list.trusts(ip("172.16.5.5")));
    }

    #[test]
    fn a_list_that_trusts_everyone_is_refused() {
        assert!(TrustedProxies::new(["0.0.0.0/0"]).is_err());
        assert!(TrustedProxies::new(std::iter::empty::<&str>()).is_err());
        assert!(TrustedProxies::new(["10.0.0.0/33"]).is_err());
        assert!(TrustedProxies::new(["not-an-address"]).is_err());
        assert!(TrustedProxies::new(["10.0.0.0/x"]).is_err());
        assert!(TrustedProxies::new(["/8"]).is_err());
    }

    #[test]
    fn an_untrusted_peer_ignores_both_header_forms() {
        let list = proxies(&["10.0.0.0/8"]);
        let peer = ip("203.0.113.9");
        assert_eq!(
            list.client_ip(peer, Some("for=1.2.3.4"), Some("1.2.3.4")),
            peer
        );
    }

    #[test]
    fn a_trusted_peer_resolves_the_rightmost_untrusted_hop() {
        let list = proxies(&["10.0.0.0/8"]);
        let peer = ip("10.0.0.2");
        assert_eq!(
            list.client_ip(peer, None, Some("198.51.100.7, 10.0.0.3, 10.0.0.1")),
            ip("198.51.100.7")
        );
        // One honest proxy, no further proxies named: the rightmost entry is the client.
        assert_eq!(
            list.client_ip(peer, None, Some("198.51.100.7")),
            ip("198.51.100.7")
        );
    }

    #[test]
    fn a_forged_left_entry_is_ignored() {
        let list = proxies(&["10.0.0.0/8"]);
        let peer = ip("10.0.0.1");
        // The client wrote `6.6.6.6`; the honest proxy appended the address it saw.
        assert_eq!(
            list.client_ip(peer, None, Some("6.6.6.6, 198.51.100.7")),
            ip("198.51.100.7")
        );
    }

    #[test]
    fn a_malformed_entry_stops_the_walk() {
        let list = proxies(&["10.0.0.0/8"]);
        let peer = ip("10.0.0.1");
        // Nothing to the left of `garbage` is believed, so the forged left entry cannot win.
        assert_eq!(
            list.client_ip(peer, None, Some("6.6.6.6, garbage, 10.0.0.5")),
            ip("10.0.0.5")
        );
        // A malformed rightmost entry leaves only the transport peer.
        assert_eq!(list.client_ip(peer, None, Some("unknown")), peer);
        assert_eq!(list.client_ip(peer, None, Some("")), peer);
    }

    #[test]
    fn ports_and_brackets_are_parsed() {
        let list = proxies(&["10.0.0.0/8"]);
        let peer = ip("10.0.0.1");
        assert_eq!(
            list.client_ip(peer, None, Some("198.51.100.7:47011")),
            ip("198.51.100.7")
        );
        assert_eq!(
            list.client_ip(peer, None, Some("[2001:db8::7]:443")),
            ip("2001:db8::7")
        );
        assert_eq!(
            list.client_ip(peer, Some("for=192.0.2.60:47011"), None),
            ip("192.0.2.60")
        );
        assert_eq!(
            list.client_ip(peer, Some("For=\"[2001:db8::1]:4711\""), None),
            ip("2001:db8::1")
        );
        assert_eq!(
            list.client_ip(peer, Some("for=192.0.2.60"), None),
            ip("192.0.2.60")
        );
    }

    #[test]
    fn a_quoted_comma_does_not_split_a_forwarded_element() {
        let list = proxies(&["10.0.0.0/8"]);
        let peer = ip("10.0.0.1");
        // The quoted value is not an address, so it stops the walk rather than being split into
        // two elements that would name `192.0.2.1` as a hop.
        assert_eq!(
            list.client_ip(peer, Some("for=\"a,b\", for=192.0.2.1"), None),
            ip("192.0.2.1")
        );
        assert_eq!(list.client_ip(peer, Some("for=\"a,b\""), None), peer);
    }

    #[test]
    fn forwarded_wins_over_x_forwarded_for() {
        let list = proxies(&["10.0.0.0/8"]);
        let peer = ip("10.0.0.1");
        assert_eq!(
            list.client_ip(peer, Some("for=198.51.100.7"), Some("203.0.113.9")),
            ip("198.51.100.7")
        );
        // An empty `Forwarded` is not a disagreement: the other field is then the record.
        assert_eq!(
            list.client_ip(peer, Some("  "), Some("203.0.113.9")),
            ip("203.0.113.9")
        );
    }

    #[test]
    fn the_hop_cap_bounds_the_walk() {
        let list = proxies(&["10.0.0.0/8"]);
        let peer = ip("10.0.0.1");
        // 40 trusted hops, and an attacker-chosen address at the far left.
        let mut entries: Vec<String> = vec!["6.6.6.6".into()];
        entries.extend((0..40).map(|_| "10.0.0.9".to_string()));
        let header = entries.join(", ");
        let resolved = list.client_ip(peer, None, Some(&header));
        assert_ne!(resolved, ip("6.6.6.6"));
        assert!(list.trusts(resolved));
    }
}
