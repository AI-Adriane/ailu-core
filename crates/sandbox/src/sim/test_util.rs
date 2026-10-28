//! Shared fixtures of the simulator's tests.

use super::*;
use crate::approval::{GrantVerifier, InMemoryGrantVerifier, VerifiedGrant};
use crate::clock::SandboxClock;
use crate::error::LimitKind;
use crate::guard::GovernedSandbox;
use crate::journal::{InMemoryJournal, SandboxJournal};
use crate::limits::Limits;
use crate::path::LogicalPath;
use crate::policy::SandboxPolicy;
use crate::request::{
    CollectRequest, CreateRequest, EnvGrant, ExecRequest, NetworkPolicy, SecretRef,
};
use crate::secret::SecretResolver;
use crate::{ApprovalClaim, SandboxContext, TenantId};
use ailu_graph_core::{NodeId, RunId};
use std::sync::Arc;

/// Virtual "now" of every test: far enough from zero that the retention window fits behind it.
pub(crate) const NOW: u64 = 30 * 24 * 3600 * 1000;
pub(crate) const RUN_STARTED: u64 = NOW - 1000;
/// The value of the secret the tests grant. It must never show up anywhere.
pub(crate) const SECRET: &str = "s3cr3t-value-42";
pub(crate) const STDIN: &[u8] = b"s3cr3t-stdin";

pub(crate) struct Fx {
    pub guard: Arc<GovernedSandbox<SimBackend>>,
    pub sim: SimBackend,
    pub clock: Arc<SimClock>,
    pub secrets: Arc<SimSecrets>,
    pub net: Arc<SimNetwork>,
    pub journal: Arc<InMemoryJournal>,
    pub verifier: Arc<InMemoryGrantVerifier>,
}

/// A backend on its own, with the test programs registered, and its clock.
pub(crate) fn bare() -> (SimBackend, Arc<SimClock>) {
    bare_with(SimConfig::default())
}

pub(crate) fn bare_with(config: SimConfig) -> (SimBackend, Arc<SimClock>) {
    let clock = Arc::new(SimClock::new(NOW));
    let sim = SimBackend::with_parts(
        Arc::clone(&clock) as Arc<dyn SandboxClock>,
        Arc::new(SimSecrets::new()),
        Arc::new(SimNetwork::new()),
        config,
    );
    register_programs(&sim);
    (sim, clock)
}

pub(crate) fn fx() -> Fx {
    fx_with(SandboxPolicy::default())
}

pub(crate) fn fx_with(policy: SandboxPolicy) -> Fx {
    let clock = Arc::new(SimClock::new(NOW));
    let secrets = Arc::new(SimSecrets::new());
    let net = Arc::new(SimNetwork::new());
    let sim = SimBackend::with_parts(
        Arc::clone(&clock) as Arc<dyn SandboxClock>,
        Arc::clone(&secrets) as Arc<dyn SecretResolver>,
        Arc::clone(&net),
        SimConfig::default(),
    );
    register_programs(&sim);
    let journal = Arc::new(InMemoryJournal::new());
    let verifier = Arc::new(InMemoryGrantVerifier::new());
    let guard = Arc::new(
        GovernedSandbox::new(
            sim.clone(),
            policy,
            Arc::clone(&clock) as Arc<dyn SandboxClock>,
            Arc::clone(&journal) as Arc<dyn SandboxJournal>,
            Arc::clone(&verifier) as Arc<dyn GrantVerifier>,
        )
        .unwrap(),
    );
    Fx {
        guard,
        sim,
        clock,
        secrets,
        net,
        journal,
        verifier,
    }
}

/// A second guard over the same backend, journal and clock: the host restarted.
pub(crate) fn restarted(fx: &Fx) -> GovernedSandbox<SimBackend> {
    GovernedSandbox::new(
        fx.sim.clone(),
        SandboxPolicy::default(),
        Arc::clone(&fx.clock) as Arc<dyn SandboxClock>,
        Arc::clone(&fx.journal) as Arc<dyn SandboxJournal>,
        Arc::clone(&fx.verifier) as Arc<dyn GrantVerifier>,
    )
    .unwrap()
}

/// The programs every test can call.
pub(crate) fn register_programs(sim: &SimBackend) {
    sim.register("echo", |inv| {
        inv.fs().write("out/result.txt", "data").unwrap();
        Ok(SimOutput::success()
            .with_stdout("hello")
            .with_stderr("warn")
            .with_usage(SimUsage {
                duration_ms: 5,
                ..SimUsage::default()
            }))
    });
    // Reports a running time past the wall-time limit: an infinite loop.
    sim.register("sleep", |_| {
        Ok(SimOutput::success().with_usage(SimUsage {
            duration_ms: 3_600_000,
            ..SimUsage::default()
        }))
    });
    // The program itself says it was killed.
    sim.register("spin", |_| Err(SimFault::Limit(LimitKind::WallTime)));
    sim.register("alloc", |_| {
        Ok(SimOutput::success()
            .with_stdout("partial")
            .with_usage(SimUsage {
                memory_bytes: 1 << 40,
                ..SimUsage::default()
            }))
    });
    sim.register("fork", |_| {
        Ok(SimOutput::success().with_usage(SimUsage {
            processes: 100_000,
            ..SimUsage::default()
        }))
    });
    // Fills the disk and ignores the refusals.
    sim.register("fill", |inv| {
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
    sim.register("flood", |_| {
        Ok(SimOutput::success().with_stdout(SimStream::Repeated {
            byte: b'x',
            len: 1 << 40,
        }))
    });
    // Prints the secret in several forms, to every place an output can go.
    sim.register("leak", |inv| {
        let value = inv.env("API_KEY").unwrap_or("none").to_owned();
        inv.fs().write("out/leak.txt", value.clone()).unwrap();
        Ok(SimOutput::success()
            .with_stdout(format!("key={value}"))
            .with_stderr(value))
    });
    sim.register("cat", |inv| {
        let data = inv.fs().read("in/data.txt").unwrap_or_default();
        Ok(SimOutput::success().with_stdout(data))
    });
    sim.register("names", |inv| {
        Ok(SimOutput::success().with_stdout(inv.env_names().join(",")))
    });
    sim.register("fetch", |inv| {
        let port: u16 = inv.args()[1].parse().unwrap();
        let said = match inv.connect(&inv.args()[0], port) {
            Ok(conn) => format!("ok:{}", conn.addr),
            Err(error) => error.to_string(),
        };
        Ok(SimOutput::success().with_stdout(said))
    });
    sim.register("resolve", |inv| {
        let said = match inv.resolve(&inv.args()[0]) {
            Ok(addrs) => format!("ok:{}", addrs.len()),
            Err(error) => error.to_string(),
        };
        Ok(SimOutput::success().with_stdout(said))
    });
    sim.register("debug-self", |inv| {
        Ok(SimOutput::success().with_stdout(format!("{inv:?}")))
    });
    sim.register("panic", |_| panic!("the program failed"));
}

pub(crate) fn ctx_for(tenant: &str, node: &str) -> SandboxContext {
    SandboxContext {
        tenant_id: TenantId::new(tenant),
        run_id: RunId::from("run-1"),
        node_id: NodeId::from(node),
        attempt: 1,
    }
}

pub(crate) fn ctx() -> SandboxContext {
    ctx_for("tenant-a", "node-1")
}

pub(crate) fn create_req() -> CreateRequest {
    CreateRequest {
        profile: "default".to_owned(),
        limits: Limits::default(),
        network: NetworkPolicy::Deny,
        inputs: vec![],
        env: vec![],
        ttl_ms: 60_000,
    }
}

pub(crate) fn create_with_secret() -> CreateRequest {
    CreateRequest {
        env: vec![EnvGrant {
            name: "API_KEY".to_owned(),
            secret_ref: SecretRef("api-key".to_owned()),
        }],
        ..create_req()
    }
}

pub(crate) fn exec_of(program: &str, args: &[&str]) -> ExecRequest {
    let mut argv = vec![program.to_owned()];
    argv.extend(args.iter().map(|a| (*a).to_owned()));
    ExecRequest {
        argv,
        cwd: None,
        stdin: Some(STDIN.to_vec()),
    }
}

pub(crate) fn exec_req() -> ExecRequest {
    exec_of("echo", &["--token=s3cr3t-arg"])
}

pub(crate) fn collect_req() -> CollectRequest {
    CollectRequest {
        patterns: vec!["out/*".to_owned()],
        max_files: 8,
        max_bytes: 4096,
    }
}

pub(crate) fn path(p: &str) -> LogicalPath {
    LogicalPath::new(p).unwrap()
}

/// Record an approval by "alice" for exactly this call, and return the claim for it.
pub(crate) fn approve(
    fx: &Fx,
    ctx: &SandboxContext,
    create: &CreateRequest,
    exec: &ExecRequest,
) -> ApprovalClaim {
    let request = fx.guard.request_approval(ctx, create, exec).unwrap();
    let approval_id = format!("apr-{}", &request.call_key[13..29]);
    fx.verifier.approve(VerifiedGrant {
        approval_id: approval_id.clone(),
        call_key: request.call_key.clone(),
        tenant_id: ctx.tenant_id.clone(),
        run_id: ctx.run_id.clone(),
        requested_by: "agent".to_owned(),
        approved_by: "alice".to_owned(),
        approval_payload_hash: "payload-hash".to_owned(),
    });
    ApprovalClaim {
        approval_id,
        call_key: request.call_key,
    }
}
