//! Network: nothing is reachable by default, DNS included; a destination off the allow list,
//! a redirect off it, and every address a sandbox must never reach are refused; a host that
//! relays is the accepted limit of an allow list, and the suite only measures it.
//!
//! The probes answer with their exit status: 0 when the destination was reached, anything else
//! when it was refused. The positive cases are the control: a backend that refuses everything
//! would otherwise pass every refusal.

use super::{case, CaseDef};
use crate::conformance::support::{ensure, Env, Lab, Must, Stop, Verdict};
use crate::conformance::target::{NetTarget, ProbeKind};
use crate::error::SandboxError;
use crate::journal::SandboxEventKind;
use crate::outcome::DeclaredExit;
use crate::policy::SandboxPolicy;
use crate::request::NetworkPolicy;

pub(super) fn cases() -> Vec<CaseDef> {
    vec![
        case!(
            Network,
            "network.allowed_host_reaches",
            RealBackend,
            "a host on the allow list can be connected to and resolved (the control of the refusals)",
            allowed_host_reaches
        ),
        case!(
            Network,
            "network.default_deny",
            RealBackend,
            "with no network rule nothing can be connected to",
            default_deny
        ),
        case!(
            Network,
            "network.dns_denied_by_default",
            RealBackend,
            "with no network rule no name can be resolved either",
            dns_denied_by_default
        ),
        case!(
            Network,
            "network.off_list_host",
            RealBackend,
            "a host that is not on the allow list is refused",
            off_list_host
        ),
        case!(
            Network,
            "network.redirect_off_list",
            RealBackend,
            "a redirect from an allowed host to one that is not allowed is refused",
            redirect_off_list
        ),
        case!(
            Network,
            "network.redirect_limit",
            RealBackend,
            "a chain of redirects longer than the limit is refused",
            redirect_limit
        ),
        case!(
            Network,
            "network.private_ipv4",
            RealBackend,
            "a name that resolves to a private IPv4 address is refused",
            private_ipv4
        ),
        case!(
            Network,
            "network.loopback",
            RealBackend,
            "a name that resolves to a loopback address is refused",
            loopback
        ),
        case!(
            Network,
            "network.link_local",
            RealBackend,
            "a name that resolves to a link-local IPv4 address is refused",
            link_local
        ),
        case!(
            Network,
            "network.dns_private_resolution",
            RealBackend,
            "a name that resolves to a private address is refused at DNS too",
            dns_private_resolution
        ),
        case!(
            Network,
            "network.ipv6_unique_local",
            RealBackend,
            "a unique-local IPv6 address is refused",
            ipv6_unique_local
        ),
        case!(
            Network,
            "network.ipv6_special_block",
            RealBackend,
            "a special block inside 2000::/3 (documentation) is refused: IPv6 is judged by an allow list",
            ipv6_special_block
        ),
        case!(
            Network,
            "network.ipv6_translated",
            RealBackend,
            "a translated (NAT64) IPv6 address is refused",
            ipv6_translated
        ),
        case!(
            Network,
            "network.ipv4_mapped_private",
            RealBackend,
            "an IPv4-mapped IPv6 address of a private IPv4 address is refused",
            ipv4_mapped_private
        ),
        case!(
            Network,
            "network.ipv4_mapped_public",
            RealBackend,
            "an IPv4-mapped IPv6 address is refused even when the IPv4 address it carries is public",
            ipv4_mapped_public
        ),
        case!(
            Network,
            "network.metadata_ipv4",
            RealBackend,
            "the IPv4 metadata endpoint is refused",
            metadata_ipv4
        ),
        case!(
            Network,
            "network.metadata_ipv6",
            RealBackend,
            "the IPv6 metadata endpoint is refused",
            metadata_ipv6
        ),
        case!(
            Network,
            "network.dns_rebinding",
            RealBackend,
            "a name whose second resolution differs from the first is refused",
            dns_rebinding
        ),
        case!(
            Network,
            "network.relay_measured",
            RealBackend,
            "an allowed host that relays is the accepted limit: the case only measures it",
            relay_measured
        ),
    ]
}

/// What the probe saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seen {
    Reached,
    Refused,
}

/// Run the probe in a sandbox with the network rules it asks for (or none, with `deny`).
async fn attempt(lab: &Lab<'_>, kind: &ProbeKind, deny: bool) -> Result<Seen, Stop> {
    let program = lab.probe(kind)?;
    let mut policy = SandboxPolicy::default();
    policy.network_allow = program.allow.clone();
    let env = lab.env_with(policy).await;
    let exec = Env::exec_of(&program);
    let ctx = env.ctx("tenant-a", "node-1");
    let mut create = env.create_req();
    if !deny && !program.allow.is_empty() {
        create.network = NetworkPolicy::Allow(program.allow.clone());
    }
    let (handle, claim) = env.ready(&ctx, create, &exec).await?;
    let result = env.run(&ctx, &handle, exec, &claim).await;
    let seen = match &result {
        Ok(outcome) => match outcome.declared_exit {
            DeclaredExit::Code { code: 0 } => Seen::Reached,
            _ => Seen::Refused,
        },
        // The backend may treat the attempt as a violation: a typed refusal too.
        Err(SandboxError::IsolationViolation { .. }) => Seen::Refused,
        Err(error) => {
            return Err(Stop::Fail(format!(
                "the probe ended with {} instead of an answer",
                error.full_code()
            )))
        }
    };
    ensure!(
        env.kinds().iter().any(|kind| matches!(
            kind,
            SandboxEventKind::SandboxExecCompleted | SandboxEventKind::SandboxExecFailed
        )),
        "the attempt is not in the journal"
    );
    env.guard.destroy(&ctx, &handle).await.must("destroy")?;
    Ok(seen)
}

/// The destination must be refused under the rules the probe asks for.
async fn refused(lab: &Lab<'_>, kind: ProbeKind) -> Verdict {
    let seen = attempt(lab, &kind, false).await?;
    ensure!(seen == Seen::Refused, "{kind:?} was reached");
    Ok(())
}

async fn allowed_host_reaches(lab: &Lab<'_>) -> Verdict {
    let seen = attempt(lab, &ProbeKind::Connect(NetTarget::Allowed), false).await?;
    ensure!(
        seen == Seen::Reached,
        "an allowed host could not be connected to"
    );
    let seen = attempt(lab, &ProbeKind::Resolve(NetTarget::Allowed), false).await?;
    ensure!(
        seen == Seen::Reached,
        "an allowed name could not be resolved"
    );
    Ok(())
}

async fn default_deny(lab: &Lab<'_>) -> Verdict {
    let kind = ProbeKind::Connect(NetTarget::Allowed);
    ensure!(
        attempt(lab, &kind, true).await? == Seen::Refused,
        "a host was reached with no network rule"
    );
    Ok(())
}

async fn dns_denied_by_default(lab: &Lab<'_>) -> Verdict {
    let kind = ProbeKind::Resolve(NetTarget::Allowed);
    ensure!(
        attempt(lab, &kind, true).await? == Seen::Refused,
        "a name was resolved with no network rule"
    );
    Ok(())
}

async fn off_list_host(lab: &Lab<'_>) -> Verdict {
    refused(lab, ProbeKind::Connect(NetTarget::OffList)).await
}

async fn redirect_off_list(lab: &Lab<'_>) -> Verdict {
    refused(lab, ProbeKind::Connect(NetTarget::RedirectOffList)).await
}

async fn redirect_limit(lab: &Lab<'_>) -> Verdict {
    refused(lab, ProbeKind::Connect(NetTarget::RedirectLoop)).await
}

async fn private_ipv4(lab: &Lab<'_>) -> Verdict {
    refused(lab, ProbeKind::Connect(NetTarget::PrivateV4)).await
}

async fn loopback(lab: &Lab<'_>) -> Verdict {
    refused(lab, ProbeKind::Connect(NetTarget::Loopback)).await
}

async fn link_local(lab: &Lab<'_>) -> Verdict {
    refused(lab, ProbeKind::Connect(NetTarget::LinkLocalV4)).await
}

async fn dns_private_resolution(lab: &Lab<'_>) -> Verdict {
    refused(lab, ProbeKind::Resolve(NetTarget::PrivateV4)).await
}

async fn ipv6_unique_local(lab: &Lab<'_>) -> Verdict {
    refused(lab, ProbeKind::Connect(NetTarget::UniqueLocalV6)).await
}

async fn ipv6_special_block(lab: &Lab<'_>) -> Verdict {
    refused(lab, ProbeKind::Connect(NetTarget::DocumentationV6)).await
}

async fn ipv6_translated(lab: &Lab<'_>) -> Verdict {
    refused(lab, ProbeKind::Connect(NetTarget::TranslatedV6)).await
}

async fn ipv4_mapped_private(lab: &Lab<'_>) -> Verdict {
    refused(lab, ProbeKind::Connect(NetTarget::MappedPrivateV6)).await
}

async fn ipv4_mapped_public(lab: &Lab<'_>) -> Verdict {
    refused(lab, ProbeKind::Connect(NetTarget::MappedPublicV6)).await
}

async fn metadata_ipv4(lab: &Lab<'_>) -> Verdict {
    refused(lab, ProbeKind::Connect(NetTarget::MetadataV4)).await
}

async fn metadata_ipv6(lab: &Lab<'_>) -> Verdict {
    refused(lab, ProbeKind::Connect(NetTarget::MetadataV6)).await
}

async fn dns_rebinding(lab: &Lab<'_>) -> Verdict {
    refused(lab, ProbeKind::Connect(NetTarget::Rebinding)).await
}

async fn relay_measured(lab: &Lab<'_>) -> Verdict {
    // An allowed host that relays is on the list: the contract cannot tell it from any other.
    // The case runs the probe to a typed answer and reports what the backend did with it.
    let seen = attempt(lab, &ProbeKind::Connect(NetTarget::Relay), false).await?;
    let what = match seen {
        Seen::Reached => "the connection to the relaying host was let through",
        Seen::Refused => "the connection to the relaying host was refused",
    };
    Err(Stop::Measured(what.to_owned()))
}
