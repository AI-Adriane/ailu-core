//! The simulated network: a table of hosts, and the policy applied to it.
//!
//! Nothing here opens a socket. A host is an entry in a table (a name, the addresses it
//! resolves to, optionally a redirect or a second, different answer), and a program "connects"
//! by asking the table. What the simulator proves is that the network policy is applied the way
//! the contract says; what it cannot prove is that a production backend applies it.
//!
//! Every refusal is counted ([`NetStats`]) and nothing else is kept: no host name, no address,
//! no request content.
//!
//! **Accepted limit.** A host that is on the allow list and that *relays* (an open redirect, a
//! tunnel, a proxy) gets traffic to places the list does not name, and no backend can know it.
//! The simulator lets such a host pass and counts it ([`NetStats::relayed`]) so that a test can
//! measure what happened.

use crate::request::{HostRule, NetworkPolicy};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

/// How many redirects a connection follows before it is refused.
pub const MAX_REDIRECTS: usize = 5;

/// What kind of forbidden address a name resolved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddrClass {
    /// `0.0.0.0/8`, `::`.
    Unspecified,
    /// `127.0.0.0/8`, `::1`.
    Loopback,
    /// RFC 1918, shared address space (`100.64.0.0/10`), unique-local and site-local IPv6.
    Private,
    /// `169.254.0.0/16`, `fe80::/10`.
    LinkLocal,
    /// The cloud metadata endpoints: `169.254.169.254` and `fd00:ec2::254`.
    Metadata,
    /// Multicast.
    Multicast,
    /// Broadcast, the reserved block, documentation and benchmarking ranges, deprecated IPv6
    /// forms.
    Reserved,
}

/// Whether an address is one a sandbox must never reach, and which kind it is. IPv4-mapped
/// IPv6 (`::ffff:a.b.c.d`), NAT64 (`64:ff9b::/96`) and 6to4 (`2002::/16`) forms are judged by
/// the IPv4 address they carry, so a forbidden IPv4 address cannot be reached through its IPv6
/// spelling.
pub fn classify_address(ip: IpAddr) -> Option<AddrClass> {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => classify_v6(v6),
    }
}

fn classify_v4(a: Ipv4Addr) -> Option<AddrClass> {
    let o = a.octets();
    if a == Ipv4Addr::new(169, 254, 169, 254) {
        Some(AddrClass::Metadata)
    } else if o[0] == 0 {
        Some(AddrClass::Unspecified)
    } else if a.is_loopback() {
        Some(AddrClass::Loopback)
    } else if a.is_private() || (o[0] == 100 && o[1] & 0xc0 == 64) {
        Some(AddrClass::Private)
    } else if a.is_link_local() {
        Some(AddrClass::LinkLocal)
    } else if a.is_multicast() {
        Some(AddrClass::Multicast)
    } else if a.is_broadcast()
        || o[0] >= 240
        || a.is_documentation()
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 198 && o[1] & 0xfe == 18)
    {
        Some(AddrClass::Reserved)
    } else {
        None
    }
}

fn classify_v6(a: Ipv6Addr) -> Option<AddrClass> {
    let s = a.segments();
    if a == Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254) {
        return Some(AddrClass::Metadata);
    }
    if let Some(v4) = a.to_ipv4_mapped() {
        return classify_v4(v4);
    }
    // NAT64 well-known prefix: the IPv4 address is in the last 32 bits.
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2..6].iter().all(|x| *x == 0) {
        return classify_v4(embedded(s[6], s[7])).or(Some(AddrClass::Reserved));
    }
    // 6to4: the IPv4 address is in bits 16..48.
    if s[0] == 0x2002 {
        return classify_v4(embedded(s[1], s[2])).or(Some(AddrClass::Reserved));
    }
    if a.is_unspecified() {
        Some(AddrClass::Unspecified)
    } else if a.is_loopback() {
        Some(AddrClass::Loopback)
    } else if a.is_multicast() {
        Some(AddrClass::Multicast)
    } else if s[0] & 0xffc0 == 0xfe80 {
        Some(AddrClass::LinkLocal)
    } else if s[0] & 0xfe00 == 0xfc00 || s[0] & 0xffc0 == 0xfec0 {
        Some(AddrClass::Private)
    } else if s[..6].iter().all(|x| *x == 0) || (s[0] == 0x2001 && (s[1] == 0x0db8 || s[1] == 0)) {
        // IPv4-compatible (deprecated), documentation, Teredo.
        Some(AddrClass::Reserved)
    } else {
        None
    }
}

fn embedded(high: u16, low: u16) -> Ipv4Addr {
    let [a, b] = high.to_be_bytes();
    let [c, d] = low.to_be_bytes();
    Ipv4Addr::new(a, b, c, d)
}

/// A host of the simulated network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimHost {
    addrs: Vec<IpAddr>,
    rebind: Option<Vec<IpAddr>>,
    redirect: Option<HostRule>,
    relay: bool,
    body: Vec<u8>,
}

impl SimHost {
    /// A host that resolves to `addrs`.
    pub fn new(addrs: impl IntoIterator<Item = IpAddr>) -> Self {
        Self {
            addrs: addrs.into_iter().collect(),
            rebind: None,
            redirect: None,
            relay: false,
            body: Vec::new(),
        }
    }

    /// The host answers a *second* resolution of its name with `addrs` instead: DNS
    /// rebinding.
    pub fn rebinding_to(mut self, addrs: impl IntoIterator<Item = IpAddr>) -> Self {
        self.rebind = Some(addrs.into_iter().collect());
        self
    }

    /// Connecting to the host is answered by a redirect to `host:port`.
    pub fn redirecting_to(mut self, host: &str, port: u16) -> Self {
        self.redirect = Some(HostRule {
            host: host.to_ascii_lowercase(),
            port,
        });
        self
    }

    /// The host relays traffic to places the allow list does not name (an open redirect, a
    /// tunnel). See the module documentation: the simulator lets it pass and counts it.
    pub fn relaying(mut self) -> Self {
        self.relay = true;
        self
    }

    /// What a connection to the host returns.
    pub fn with_body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }
}

/// Why a network operation was refused. Carries no host, no address, no content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NetDenied {
    /// The sandbox has no network (`Deny`, the default), so not even a name can be resolved.
    #[error("network denied")]
    NetworkDenied,
    /// The host and port are not on the allow list.
    #[error("host not allowed")]
    HostNotAllowed,
    /// A redirect led to a host and port that are not on the allow list.
    #[error("redirect target not allowed")]
    RedirectNotAllowed,
    /// The name resolves to an address a sandbox must never reach.
    #[error("address blocked")]
    AddressBlocked(AddrClass),
    /// A second resolution of the name gave a different answer. The connection is refused
    /// and the first address stays the pinned one.
    #[error("dns rebinding")]
    Rebinding {
        /// The address of the first resolution, the only one the connection may use.
        pinned: IpAddr,
    },
    /// More than [`MAX_REDIRECTS`] redirects.
    #[error("too many redirects")]
    TooManyRedirects,
    /// The name does not resolve. Not a policy refusal.
    #[error("unknown host")]
    UnknownHost,
}

/// Counters of what the simulated network did. They hold numbers only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetStats {
    /// Connections that were let through (relaying hosts included).
    pub allowed: u64,
    /// Of those, connections to a host that relays: the accepted limit of the allow list.
    pub relayed: u64,
    /// Attempts in `Deny` mode.
    pub network_denied: u64,
    /// Hosts not on the allow list.
    pub host_not_allowed: u64,
    /// Redirects to hosts not on the allow list.
    pub redirect_not_allowed: u64,
    /// Resolutions to a forbidden address.
    pub address_blocked: u64,
    /// Of those, resolutions to a metadata endpoint.
    pub metadata_blocked: u64,
    /// Divergent second resolutions.
    pub rebinding: u64,
    /// Redirect chains that were too long.
    pub too_many_redirects: u64,
    /// Names that did not resolve.
    pub unresolved: u64,
}

impl NetStats {
    /// Every policy refusal together (unresolved names are not one).
    pub fn denied(&self) -> u64 {
        self.network_denied
            + self.host_not_allowed
            + self.redirect_not_allowed
            + self.address_blocked
            + self.rebinding
            + self.too_many_redirects
    }
}

#[derive(Default)]
struct Counters {
    allowed: AtomicU64,
    relayed: AtomicU64,
    network_denied: AtomicU64,
    host_not_allowed: AtomicU64,
    redirect_not_allowed: AtomicU64,
    address_blocked: AtomicU64,
    metadata_blocked: AtomicU64,
    rebinding: AtomicU64,
    too_many_redirects: AtomicU64,
    unresolved: AtomicU64,
}

fn bump(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

/// A connection the simulated network accepted.
#[derive(Clone, PartialEq, Eq)]
pub struct SimConnection {
    /// The address resolved once and pinned for the connection.
    pub addr: IpAddr,
    /// The port reached (after redirects).
    pub port: u16,
    /// What the host returned.
    pub body: Vec<u8>,
}

impl std::fmt::Debug for SimConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimConnection")
            .field("addr", &self.addr)
            .field("port", &self.port)
            .field("body", &self.body.len())
            .finish()
    }
}

/// The table of simulated hosts, and the policy applied to it.
#[derive(Default)]
pub struct SimNetwork {
    hosts: Mutex<HashMap<String, SimHost>>,
    counters: Counters,
}

impl SimNetwork {
    /// An empty network.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add (or replace) a host. Names are matched in lowercase.
    pub fn add_host(&self, name: &str, host: SimHost) {
        self.hosts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(name.to_ascii_lowercase(), host);
    }

    /// What the network has done so far.
    pub fn stats(&self) -> NetStats {
        let c = &self.counters;
        let get = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        NetStats {
            allowed: get(&c.allowed),
            relayed: get(&c.relayed),
            network_denied: get(&c.network_denied),
            host_not_allowed: get(&c.host_not_allowed),
            redirect_not_allowed: get(&c.redirect_not_allowed),
            address_blocked: get(&c.address_blocked),
            metadata_blocked: get(&c.metadata_blocked),
            rebinding: get(&c.rebinding),
            too_many_redirects: get(&c.too_many_redirects),
            unresolved: get(&c.unresolved),
        }
    }

    fn host(&self, name: &str) -> Option<SimHost> {
        self.hosts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned()
    }

    /// The policy gate shared by DNS and connections: `Deny` refuses everything, `Allow`
    /// refuses a host and port that are not on the list.
    fn gate(
        &self,
        policy: &NetworkPolicy,
        host: &str,
        port: u16,
        redirected: bool,
    ) -> Result<(), NetDenied> {
        match policy {
            NetworkPolicy::Deny => {
                bump(&self.counters.network_denied);
                Err(NetDenied::NetworkDenied)
            }
            NetworkPolicy::Allow(rules) => {
                if rules.iter().any(|r| r.host == host && r.port == port) {
                    Ok(())
                } else if redirected {
                    bump(&self.counters.redirect_not_allowed);
                    Err(NetDenied::RedirectNotAllowed)
                } else {
                    bump(&self.counters.host_not_allowed);
                    Err(NetDenied::HostNotAllowed)
                }
            }
        }
    }

    fn check_addresses(&self, addrs: &[IpAddr]) -> Result<(), NetDenied> {
        for addr in addrs {
            if let Some(class) = classify_address(*addr) {
                bump(&self.counters.address_blocked);
                if class == AddrClass::Metadata {
                    bump(&self.counters.metadata_blocked);
                }
                return Err(NetDenied::AddressBlocked(class));
            }
        }
        Ok(())
    }

    /// Resolve a name. `Deny` refuses (DNS included); with `Allow`, the name must be on the
    /// list (any port) and no address it resolves to may be forbidden.
    pub fn resolve(&self, policy: &NetworkPolicy, name: &str) -> Result<Vec<IpAddr>, NetDenied> {
        let name = name.to_ascii_lowercase();
        match policy {
            NetworkPolicy::Deny => {
                bump(&self.counters.network_denied);
                return Err(NetDenied::NetworkDenied);
            }
            NetworkPolicy::Allow(rules) => {
                if !rules.iter().any(|r| r.host == name) {
                    bump(&self.counters.host_not_allowed);
                    return Err(NetDenied::HostNotAllowed);
                }
            }
        }
        let Some(host) = self.host(&name).filter(|h| !h.addrs.is_empty()) else {
            bump(&self.counters.unresolved);
            return Err(NetDenied::UnknownHost);
        };
        self.check_addresses(&host.addrs)?;
        Ok(host.addrs)
    }

    /// Connect to `name:port`, following redirects. Every hop goes through the same checks:
    /// the allow list, the forbidden addresses, and a second resolution that must agree with
    /// the first.
    pub fn connect(
        &self,
        policy: &NetworkPolicy,
        name: &str,
        port: u16,
    ) -> Result<SimConnection, NetDenied> {
        let mut target = HostRule {
            host: name.to_ascii_lowercase(),
            port,
        };
        let mut redirected = false;
        for _ in 0..=MAX_REDIRECTS {
            self.gate(policy, &target.host, target.port, redirected)?;
            let Some(host) = self.host(&target.host).filter(|h| !h.addrs.is_empty()) else {
                bump(&self.counters.unresolved);
                return Err(NetDenied::UnknownHost);
            };
            self.check_addresses(&host.addrs)?;
            // The first answer is pinned. A client that resolved again would get the second
            // answer; the simulator refuses a connection whose two answers disagree.
            let second = host.rebind.as_ref().unwrap_or(&host.addrs);
            if *second != host.addrs {
                bump(&self.counters.rebinding);
                return Err(NetDenied::Rebinding {
                    pinned: host.addrs[0],
                });
            }
            if let Some(next) = host.redirect {
                target = next;
                redirected = true;
                continue;
            }
            bump(&self.counters.allowed);
            if host.relay {
                bump(&self.counters.relayed);
            }
            return Ok(SimConnection {
                addr: host.addrs[0],
                port: target.port,
                body: host.body,
            });
        }
        bump(&self.counters.too_many_redirects);
        Err(NetDenied::TooManyRedirects)
    }
}

impl std::fmt::Debug for SimNetwork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimNetwork")
            .field(
                "hosts",
                &self
                    .hosts
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .len(),
            )
            .field("stats", &self.stats())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn allow(rules: &[(&str, u16)]) -> NetworkPolicy {
        NetworkPolicy::Allow(
            rules
                .iter()
                .map(|(host, port)| HostRule {
                    host: (*host).to_owned(),
                    port: *port,
                })
                .collect(),
        )
    }

    fn net() -> SimNetwork {
        let net = SimNetwork::new();
        net.add_host(
            "api.example.com",
            SimHost::new([ip("93.184.216.34")]).with_body("ok"),
        );
        net.add_host("other.example.com", SimHost::new([ip("93.184.216.35")]));
        net
    }

    #[test]
    fn classification_covers_private_loopback_link_local_and_metadata() {
        use AddrClass::*;
        let cases = [
            ("10.1.2.3", Some(Private)),
            ("172.16.0.1", Some(Private)),
            ("172.32.0.1", None),
            ("192.168.1.1", Some(Private)),
            ("100.64.0.1", Some(Private)),
            ("127.0.0.1", Some(Loopback)),
            ("0.0.0.0", Some(Unspecified)),
            ("169.254.1.1", Some(LinkLocal)),
            ("169.254.169.254", Some(Metadata)),
            ("224.0.0.1", Some(Multicast)),
            ("255.255.255.255", Some(Reserved)),
            ("240.0.0.1", Some(Reserved)),
            ("93.184.216.34", None),
            ("::1", Some(Loopback)),
            ("::", Some(Unspecified)),
            ("fe80::1", Some(LinkLocal)),
            ("fd00::1", Some(Private)),
            ("fc00::1", Some(Private)),
            ("fd00:ec2::254", Some(Metadata)),
            ("ff02::1", Some(Multicast)),
            ("2001:db8::1", Some(Reserved)),
            ("2606:4700::1111", None),
            // IPv4-mapped IPv6 obeys the IPv4 rules.
            ("::ffff:10.0.0.1", Some(Private)),
            ("::ffff:127.0.0.1", Some(Loopback)),
            ("::ffff:169.254.169.254", Some(Metadata)),
            ("::ffff:93.184.216.34", None),
            // NAT64 and 6to4 carry an IPv4 address too.
            ("64:ff9b::a00:1", Some(Private)),
            ("2002:7f00:1::", Some(Loopback)),
            // IPv4-compatible, deprecated.
            ("::10.0.0.1", Some(Reserved)),
        ];
        for (addr, expected) in cases {
            assert_eq!(classify_address(ip(addr)), expected, "{addr}");
        }
    }

    #[test]
    fn deny_is_the_default_and_covers_dns() {
        let net = net();
        assert_eq!(
            net.resolve(&NetworkPolicy::Deny, "api.example.com"),
            Err(NetDenied::NetworkDenied)
        );
        assert_eq!(
            net.connect(&NetworkPolicy::default(), "api.example.com", 443)
                .unwrap_err(),
            NetDenied::NetworkDenied
        );
        assert_eq!(net.stats().network_denied, 2);
    }

    #[test]
    fn an_allowed_host_connects_and_others_are_refused() {
        let net = net();
        let policy = allow(&[("api.example.com", 443)]);
        let conn = net.connect(&policy, "API.example.com", 443).unwrap();
        assert_eq!(conn.addr, ip("93.184.216.34"));
        assert_eq!(conn.body, b"ok");
        assert_eq!(
            net.connect(&policy, "other.example.com", 443).unwrap_err(),
            NetDenied::HostNotAllowed
        );
        assert_eq!(
            net.connect(&policy, "api.example.com", 80).unwrap_err(),
            NetDenied::HostNotAllowed,
            "the port is part of the rule"
        );
        assert_eq!(
            net.resolve(&policy, "other.example.com"),
            Err(NetDenied::HostNotAllowed)
        );
        assert_eq!(
            net.resolve(&policy, "api.example.com").unwrap(),
            vec![ip("93.184.216.34")]
        );
        let stats = net.stats();
        assert_eq!((stats.allowed, stats.host_not_allowed), (1, 3));
    }

    #[test]
    fn a_name_on_the_list_that_resolves_to_a_forbidden_address_is_refused() {
        let net = SimNetwork::new();
        for (name, addr) in [
            ("private.test", "10.0.0.7"),
            ("loop.test", "127.0.0.1"),
            ("link.test", "169.254.10.10"),
            ("v6.test", "fd12::1"),
            ("mapped.test", "::ffff:192.168.0.1"),
            ("meta4.test", "169.254.169.254"),
            ("meta6.test", "fd00:ec2::254"),
        ] {
            net.add_host(name, SimHost::new([ip(addr)]));
            let policy = allow(&[(name, 443)]);
            assert!(
                matches!(
                    net.connect(&policy, name, 443),
                    Err(NetDenied::AddressBlocked(_))
                ),
                "{name}"
            );
            assert!(matches!(
                net.resolve(&policy, name),
                Err(NetDenied::AddressBlocked(_))
            ));
        }
        // One bad address among good ones is enough.
        net.add_host(
            "mixed.test",
            SimHost::new([ip("93.184.216.34"), ip("10.0.0.1")]),
        );
        assert!(net
            .connect(&allow(&[("mixed.test", 443)]), "mixed.test", 443)
            .is_err());
        let stats = net.stats();
        assert_eq!(stats.metadata_blocked, 4, "two connects and two resolves");
        assert_eq!(stats.allowed, 0);
    }

    #[test]
    fn a_redirect_is_followed_only_to_a_listed_host() {
        let net = net();
        net.add_host(
            "hop.example.com",
            SimHost::new([ip("93.184.216.40")]).redirecting_to("api.example.com", 443),
        );
        net.add_host(
            "bad-hop.example.com",
            SimHost::new([ip("93.184.216.41")]).redirecting_to("other.example.com", 443),
        );
        net.add_host(
            "evil-hop.example.com",
            SimHost::new([ip("93.184.216.42")]).redirecting_to("meta.test", 80),
        );
        net.add_host("meta.test", SimHost::new([ip("169.254.169.254")]));
        let policy = allow(&[
            ("hop.example.com", 80),
            ("bad-hop.example.com", 80),
            ("evil-hop.example.com", 80),
            ("api.example.com", 443),
            ("meta.test", 80),
        ]);
        let conn = net.connect(&policy, "hop.example.com", 80).unwrap();
        assert_eq!((conn.addr, conn.port), (ip("93.184.216.34"), 443));
        assert_eq!(
            net.connect(&policy, "bad-hop.example.com", 80).unwrap_err(),
            NetDenied::RedirectNotAllowed
        );
        assert_eq!(
            net.connect(&policy, "evil-hop.example.com", 80)
                .unwrap_err(),
            NetDenied::AddressBlocked(AddrClass::Metadata),
            "a listed redirect target still resolves to a forbidden address"
        );
        assert_eq!(net.stats().redirect_not_allowed, 1);
    }

    #[test]
    fn a_redirect_loop_is_cut() {
        let net = SimNetwork::new();
        net.add_host(
            "a.test",
            SimHost::new([ip("93.184.216.1")]).redirecting_to("b.test", 80),
        );
        net.add_host(
            "b.test",
            SimHost::new([ip("93.184.216.2")]).redirecting_to("a.test", 80),
        );
        let policy = allow(&[("a.test", 80), ("b.test", 80)]);
        assert_eq!(
            net.connect(&policy, "a.test", 80).unwrap_err(),
            NetDenied::TooManyRedirects
        );
        assert_eq!(net.stats().too_many_redirects, 1);
    }

    #[test]
    fn dns_rebinding_is_refused_and_the_first_address_stays_pinned() {
        let net = SimNetwork::new();
        net.add_host(
            "rebind.test",
            SimHost::new([ip("93.184.216.34")]).rebinding_to([ip("127.0.0.1")]),
        );
        let policy = allow(&[("rebind.test", 443)]);
        assert_eq!(
            net.connect(&policy, "rebind.test", 443).unwrap_err(),
            NetDenied::Rebinding {
                pinned: ip("93.184.216.34")
            }
        );
        // Even a rebind to a harmless-looking address is a divergence.
        net.add_host(
            "swap.test",
            SimHost::new([ip("93.184.216.34")]).rebinding_to([ip("93.184.216.99")]),
        );
        assert_eq!(
            net.connect(&allow(&[("swap.test", 443)]), "swap.test", 443)
                .unwrap_err(),
            NetDenied::Rebinding {
                pinned: ip("93.184.216.34")
            }
        );
        assert_eq!(net.stats().rebinding, 2);
    }

    #[test]
    fn a_relaying_host_passes_and_is_counted() {
        let net = SimNetwork::new();
        net.add_host("relay.test", SimHost::new([ip("93.184.216.50")]).relaying());
        let conn = net.connect(&allow(&[("relay.test", 443)]), "relay.test", 443);
        assert!(conn.is_ok(), "the accepted limit of an allow list");
        let stats = net.stats();
        assert_eq!((stats.allowed, stats.relayed, stats.denied()), (1, 1, 0));
    }

    #[test]
    fn unknown_names_are_not_policy_refusals() {
        let net = net();
        let policy = allow(&[("ghost.example.com", 443)]);
        assert_eq!(
            net.connect(&policy, "ghost.example.com", 443).unwrap_err(),
            NetDenied::UnknownHost
        );
        assert_eq!(
            net.resolve(&policy, "ghost.example.com"),
            Err(NetDenied::UnknownHost)
        );
        let stats = net.stats();
        assert_eq!((stats.unresolved, stats.denied()), (2, 0));
    }

    #[test]
    fn debug_holds_no_host_name_address_or_content() {
        let net = net();
        let _ = net.connect(&allow(&[("api.example.com", 443)]), "api.example.com", 443);
        let shown = format!("{net:?}");
        assert!(
            !shown.contains("example.com") && !shown.contains("93.184"),
            "{shown}"
        );
        let conn = net
            .connect(&allow(&[("api.example.com", 443)]), "api.example.com", 443)
            .unwrap();
        assert!(!format!("{conn:?}").contains("ok\""));
    }
}
