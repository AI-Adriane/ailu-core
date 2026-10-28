//! The reference table of simulated hosts the network tests and the conformance target share:
//! one public host, one that is public but off the allow list, redirects, and a name for each
//! kind of address a sandbox must never reach.

use super::net::{SimHost, SimNetwork};
use crate::request::HostRule;
use std::net::IpAddr;

fn ip(text: &str) -> IpAddr {
    text.parse()
        .expect("the reference table holds valid addresses")
}

/// Every host the allow list of the reference table names, with its port. `other.example.com`
/// is deliberately absent: it exists but is off the list.
const LISTED: [(&str, u16); 16] = [
    ("api.example.com", 443),
    ("hop.example.com", 80),
    ("badhop.example.com", 80),
    ("loop.example.com", 443),
    ("private.example.com", 443),
    ("loopback.example.com", 443),
    ("linklocal.example.com", 443),
    ("ula.example.com", 443),
    ("mapped.example.com", 443),
    ("mappedpub.example.com", 443),
    ("nat64.example.com", 443),
    ("doc6.example.com", 443),
    ("meta4.example.com", 443),
    ("meta6.example.com", 443),
    ("rebind.example.com", 443),
    ("relay.example.com", 443),
];

/// The allow-list rules of the reference table.
pub(crate) fn rules() -> Vec<HostRule> {
    LISTED
        .iter()
        .map(|(host, port)| HostRule {
            host: (*host).to_owned(),
            port: *port,
        })
        .collect()
}

/// Add the reference hosts to `net`.
pub(crate) fn install(net: &SimNetwork) {
    net.add_host("api.example.com", SimHost::new([ip("93.184.216.34")]));
    net.add_host("other.example.com", SimHost::new([ip("93.184.216.35")]));
    net.add_host(
        "hop.example.com",
        SimHost::new([ip("93.184.216.36")]).redirecting_to("api.example.com", 443),
    );
    net.add_host(
        "badhop.example.com",
        SimHost::new([ip("93.184.216.37")]).redirecting_to("other.example.com", 443),
    );
    // A host that redirects to itself: the chain never ends.
    net.add_host(
        "loop.example.com",
        SimHost::new([ip("93.184.216.40")]).redirecting_to("loop.example.com", 443),
    );
    net.add_host("private.example.com", SimHost::new([ip("10.1.2.3")]));
    net.add_host("loopback.example.com", SimHost::new([ip("127.0.0.1")]));
    net.add_host("linklocal.example.com", SimHost::new([ip("169.254.10.10")]));
    net.add_host("ula.example.com", SimHost::new([ip("fd12:3456::1")]));
    net.add_host(
        "mapped.example.com",
        SimHost::new([ip("::ffff:192.168.1.1")]),
    );
    net.add_host(
        "mappedpub.example.com",
        SimHost::new([ip("::ffff:93.184.216.34")]),
    );
    net.add_host(
        "nat64.example.com",
        SimHost::new([ip("64:ff9b::5db8:d822")]),
    );
    net.add_host("doc6.example.com", SimHost::new([ip("2001:db8::1")]));
    net.add_host("meta4.example.com", SimHost::new([ip("169.254.169.254")]));
    net.add_host("meta6.example.com", SimHost::new([ip("fd00:ec2::254")]));
    net.add_host(
        "rebind.example.com",
        SimHost::new([ip("93.184.216.38")]).rebinding_to([ip("127.0.0.1")]),
    );
    net.add_host(
        "relay.example.com",
        SimHost::new([ip("93.184.216.39")]).relaying(),
    );
}
