//! The simulated backend as a conformance target.

use super::target::{
    ConformanceTarget, Hooks, Instance, MarkerState, NetTarget, ProbeKind, ProbeProgram,
};
use crate::clock::SandboxClock;
use crate::ids::{IdempotencyKey, SandboxContext, SandboxStep, TenantId};
use crate::outcome::DeclaredExit;
use crate::request::HostRule;
use crate::secret::SecretResolver;
use crate::sim::reference;
use crate::sim::{
    SimBackend, SimClock, SimConfig, SimFault, SimInvocation, SimNetwork, SimOutput, SimResult,
    SimSecrets, SimStream, SimUsage,
};
use ailu_fs_backend::ArtifactRef;
use async_trait::async_trait;
use std::sync::Arc;

/// Where the virtual clock of every instance starts: thirty days after the epoch, far enough
/// from zero for any retention to fit behind it.
const START_MS: u64 = 30 * 24 * 3600 * 1000;

/// The simulated backend ([`SimBackend`]) as a [`ConformanceTarget`]. It passes every case
/// that applies to it; see the tests of the crate for the cases it cannot take part in.
///
/// Each instance has its own virtual clock, secret store and table of hosts (the reference
/// table: one public host, one off the allow list, redirects, and a name for each kind of
/// address a sandbox must never reach).
#[derive(Clone, Debug)]
pub struct SimTarget {
    config: SimConfig,
}

impl Default for SimTarget {
    fn default() -> Self {
        Self::new()
    }
}

impl SimTarget {
    /// The simulator with its default settings.
    pub fn new() -> Self {
        Self::with_config(SimConfig::default())
    }

    /// The simulator with other settings.
    pub fn with_config(config: SimConfig) -> Self {
        Self { config }
    }

    /// The allow-list rules the network probes of this target stand on: every host of the
    /// reference table that a sandbox may be allowed to name.
    pub fn reference_rules() -> Vec<HostRule> {
        reference::rules()
    }
}

#[async_trait]
impl ConformanceTarget for SimTarget {
    type Backend = SimBackend;

    fn name(&self) -> String {
        "ailu-sandbox simulator".to_owned()
    }

    async fn fresh(&self) -> Instance<SimBackend> {
        let clock = Arc::new(SimClock::new(START_MS));
        let secrets = Arc::new(SimSecrets::new());
        let network = Arc::new(SimNetwork::new());
        reference::install(&network);
        let sim = SimBackend::with_parts(
            Arc::clone(&clock) as Arc<dyn SandboxClock>,
            Arc::clone(&secrets) as Arc<dyn SecretResolver>,
            network,
            self.config.clone(),
        );
        register_programs(&sim);
        let hooks = SimHooks {
            sim: sim.clone(),
            clock: Arc::clone(&clock),
            secrets,
        };
        Instance::new(sim, clock).with_hooks(Arc::new(hooks))
    }

    fn probe(&self, kind: &ProbeKind) -> Option<ProbeProgram> {
        Some(match kind {
            ProbeKind::Success => ProbeProgram::new(["conf-ok"]),
            ProbeKind::WallTime => ProbeProgram::new(["conf-wall"]),
            // The simulator runs a program in one step: there is no moment to catch it running.
            ProbeKind::Blocking => return None,
            ProbeKind::Memory => ProbeProgram::new(["conf-memory"]),
            ProbeKind::Processes => ProbeProgram::new(["conf-processes"]),
            ProbeKind::Disk => ProbeProgram::new(["conf-disk"]),
            ProbeKind::HugeOutput => ProbeProgram::new(["conf-flood"]),
            ProbeKind::ListEnvironment => ProbeProgram::new(["conf-env"]),
            ProbeKind::ReadSecret { name } => ProbeProgram::new(["conf-secret", name.as_str()]),
            ProbeKind::EchoStdin => ProbeProgram::new(["conf-stdin"]),
            ProbeKind::PrintWorkingDirectory => ProbeProgram::new(["conf-pwd"]),
            ProbeKind::ReadInput { path } => ProbeProgram::new(["conf-read", path.as_str()]),
            ProbeKind::OutgoingLink => ProbeProgram::new(["conf-link"]),
            ProbeKind::OutgoingLinkDeep => ProbeProgram::new(["conf-link-deep"]),
            ProbeKind::Connect(target) => {
                let (host, port, allow) = destination(*target);
                ProbeProgram::new(["conf-connect", host, &port.to_string()]).allowing(allow)
            }
            ProbeKind::Resolve(target) => {
                let (host, _, allow) = destination(*target);
                ProbeProgram::new(["conf-resolve", host]).allowing(allow)
            }
        })
    }
}

fn rule(host: &str, port: u16) -> HostRule {
    HostRule {
        host: host.to_owned(),
        port,
    }
}

/// The host and port a destination stands for in the reference table, and the rules a sandbox
/// is created with to reach it.
fn destination(target: NetTarget) -> (&'static str, u16, Vec<HostRule>) {
    let (host, port) = match target {
        NetTarget::Allowed | NetTarget::OffList => ("api.example.com", 443),
        NetTarget::RedirectOffList => ("badhop.example.com", 80),
        NetTarget::RedirectLoop => ("loop.example.com", 443),
        NetTarget::PrivateV4 => ("private.example.com", 443),
        NetTarget::Loopback => ("loopback.example.com", 443),
        NetTarget::LinkLocalV4 => ("linklocal.example.com", 443),
        NetTarget::UniqueLocalV6 => ("ula.example.com", 443),
        NetTarget::MappedPrivateV6 => ("mapped.example.com", 443),
        NetTarget::MappedPublicV6 => ("mappedpub.example.com", 443),
        NetTarget::TranslatedV6 => ("nat64.example.com", 443),
        NetTarget::DocumentationV6 => ("doc6.example.com", 443),
        NetTarget::MetadataV4 => ("meta4.example.com", 443),
        NetTarget::MetadataV6 => ("meta6.example.com", 443),
        NetTarget::Rebinding => ("rebind.example.com", 443),
        NetTarget::RebindingLate => ("rebind3.example.com", 443),
        NetTarget::AllowedOtherPort => {
            // The list names the host on 443; the probe asks for another port.
            return ("api.example.com", 8443, vec![rule("api.example.com", 443)]);
        }
        NetTarget::Relay => ("relay.example.com", 443),
    };
    if target == NetTarget::OffList {
        // The sandbox may reach the listed host only; the probe asks for `other.example.com`.
        return ("other.example.com", 443, vec![rule("api.example.com", 443)]);
    }
    (host, port, vec![rule(host, port)])
}

struct SimHooks {
    sim: SimBackend,
    clock: Arc<SimClock>,
    secrets: Arc<SimSecrets>,
}

#[async_trait]
impl Hooks for SimHooks {
    async fn advance_clock(&self, ms: u64) -> bool {
        self.clock.advance(ms);
        true
    }

    async fn residue_after_sweep(&self) -> Option<usize> {
        self.sim.sweep();
        Some(self.sim.residue())
    }

    async fn live_resources(&self) -> Option<usize> {
        Some(self.sim.live_sandboxes())
    }

    async fn executions(&self) -> Option<u64> {
        Some(self.sim.program_calls())
    }

    async fn arm_crash_after_marker(&self) -> bool {
        self.sim
            .inject(SandboxStep::Exec, SimFault::CrashAfterMarker)
            .is_ok()
    }

    async fn marker(&self, ctx: &SandboxContext, step: SandboxStep) -> Option<MarkerState> {
        let key = IdempotencyKey::new(ctx, step);
        Some(match self.sim.stored_state(&ctx.tenant_id, key.as_str()) {
            None => MarkerState::Absent,
            Some(false) => MarkerState::MarkerOnly,
            Some(true) => MarkerState::ResultStored,
        })
    }

    async fn retention_ms(&self) -> Option<u64> {
        Some(self.sim.retention_ms())
    }

    async fn provide_secret(&self, tenant: &TenantId, reference: &str, value: &str) -> bool {
        self.secrets.set(tenant, reference, value);
        true
    }

    async fn provide_artifact(
        &self,
        tenant: &TenantId,
        artifact: &ArtifactRef,
        bytes: &[u8],
    ) -> bool {
        self.sim.provide_artifact(tenant, artifact.clone(), bytes);
        true
    }

    async fn base_environment(&self) -> Option<Vec<String>> {
        Some(self.sim.base_env().keys().cloned().collect())
    }
}

/// The programs the probes stand for. They run no process: each is a Rust function over the
/// virtual filesystem.
fn register_programs(sim: &SimBackend) {
    sim.register("conf-ok", |inv| {
        inv.fs().write("out/result.txt", "data").ok();
        Ok(SimOutput::success()
            .with_stdout("hello")
            .with_stderr("warn")
            .with_usage(SimUsage {
                duration_ms: 5,
                ..SimUsage::default()
            }))
    });
    // Reports a running time far past any wall-time limit: an infinite loop.
    sim.register("conf-wall", |_| {
        Ok(SimOutput::success().with_usage(SimUsage {
            duration_ms: 3_600_000,
            ..SimUsage::default()
        }))
    });
    sim.register("conf-memory", |_| {
        Ok(SimOutput::success().with_usage(SimUsage {
            memory_bytes: 1 << 40,
            ..SimUsage::default()
        }))
    });
    sim.register("conf-processes", |_| {
        Ok(SimOutput::success().with_usage(SimUsage {
            processes: 100_000,
            ..SimUsage::default()
        }))
    });
    // Fills the disk and ignores the refusals.
    sim.register("conf-disk", |inv| {
        for i in 0..10_000 {
            if inv
                .fs()
                .write(&format!("out/{i}"), vec![0u8; 1024])
                .is_err()
            {
                break;
            }
        }
        Ok(SimOutput::success())
    });
    // A virtual terabyte on standard output.
    sim.register("conf-flood", |_| {
        Ok(SimOutput::success().with_stdout(SimStream::Repeated {
            byte: b'x',
            len: 1 << 40,
        }))
    });
    sim.register("conf-env", |inv| {
        Ok(SimOutput::success().with_stdout(inv.env_names().join("\n")))
    });
    sim.register("conf-secret", |inv| {
        let value = inv
            .args()
            .first()
            .and_then(|name| inv.env(name))
            .unwrap_or("none");
        inv.fs().write("out/secret.txt", value).ok();
        Ok(SimOutput::success().with_stdout(value).with_stderr(value))
    });
    sim.register("conf-stdin", |inv| {
        Ok(SimOutput::success().with_stdout(inv.stdin().to_vec()))
    });
    sim.register("conf-pwd", |inv| {
        Ok(SimOutput::success().with_stdout(inv.cwd().unwrap_or(".")))
    });
    sim.register("conf-read", |inv| {
        let content = inv
            .args()
            .first()
            .and_then(|path| inv.fs().read(path))
            .unwrap_or_default();
        Ok(SimOutput::success().with_stdout(content))
    });
    sim.register("conf-link", |inv| {
        inv.fs().symlink("out/link", "../outside").ok();
        Ok(SimOutput::success())
    });
    sim.register("conf-link-deep", |inv| {
        inv.fs().symlink("out/a/link", "../../../outside").ok();
        Ok(SimOutput::success())
    });
    sim.register("conf-connect", connected);
    sim.register("conf-resolve", |inv| {
        let reached = inv
            .args()
            .first()
            .is_some_and(|host| inv.resolve(host).is_ok());
        Ok(exit_of(reached))
    });
}

fn connected(inv: &SimInvocation) -> SimResult {
    let port = inv.args().get(1).and_then(|port| port.parse::<u16>().ok());
    let reached = match (inv.args().first(), port) {
        (Some(host), Some(port)) => inv.connect(host, port).is_ok(),
        _ => false,
    };
    Ok(exit_of(reached))
}

/// Exit 0 when the destination was reached, 1 when it was not.
fn exit_of(reached: bool) -> SimOutput {
    SimOutput::success().with_exit(DeclaredExit::Code {
        code: if reached { 0 } else { 1 },
    })
}
