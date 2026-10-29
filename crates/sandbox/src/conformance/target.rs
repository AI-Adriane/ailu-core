//! What a backend offers the suite: a fresh instance per case, the commands that provoke each
//! behavior, and optional hooks to look inside.

use crate::backend::SandboxBackend;
use crate::clock::SandboxClock;
use crate::ids::{SandboxContext, SandboxStep, TenantId};
use crate::proof::{ProofSigner, TrustedKeys};
use crate::request::HostRule;
use ailu_fs_backend::ArtifactRef;
use async_trait::async_trait;
use std::sync::Arc;

/// A backend under test, as the suite sees it.
///
/// The suite drives the backend through the guard ([`crate::GovernedSandbox`]) and, for the
/// obligations only a backend can show, directly. It never starts a process: whatever has to
/// run in the sandbox is named by a [`ProbeProgram`] that the target supplies, so a real
/// backend decides what each behavior looks like on its side.
///
/// A target that cannot provoke a behavior answers `None` from [`ConformanceTarget::probe`], and
/// the cases that need it are reported as not applicable. They are never reported as passed.
/// The same goes for the optional [`Hooks`].
#[async_trait]
pub trait ConformanceTarget: Send + Sync {
    /// The backend the guard is put in front of.
    type Backend: SandboxBackend + 'static;

    /// A short name for the report. It must not hold anything sensitive.
    fn name(&self) -> String;

    /// The isolation profile every `create` asks for. The deployment resolves it.
    fn profile(&self) -> String {
        "default".to_owned()
    }

    /// A fresh, isolated instance: nothing created by an earlier case may be visible in it.
    /// Called once per case.
    async fn fresh(&self) -> Instance<Self::Backend>;

    /// The command that provokes `kind` on this backend, or `None` when it cannot.
    fn probe(&self, kind: &ProbeKind) -> Option<ProbeProgram>;
}

/// One fresh instance of a target.
pub struct Instance<B> {
    /// The backend. The guard owns it for the duration of the case.
    pub backend: B,
    /// The clock the guard and the backend share. A backend with its own sweeps and time
    /// reads must read this one: the cases that move time (`lifecycle.ttl_sweep`,
    /// `lifecycle.slot_returned_once_after_ttl`, `idempotence.retention_covers_the_ceiling`)
    /// assume it does, and a backend on another clock fails them for that reason.
    pub clock: Arc<dyn SandboxClock>,
    /// The optional hooks.
    pub hooks: Arc<dyn Hooks>,
    /// The proof signer the cases of the `proof` group put in front of the guard, with the
    /// keys that verify what it signs. `None`: those cases are not applicable.
    pub proof: Option<ProofKit>,
}

/// A proof signer and the keys a verifier pins for it: what the `proof` group of the suite
/// needs from a target. The signer is the deployment's (or the crate's in-process one); the
/// keys are those a verifier would be configured with, never read from a record.
///
/// A target gives a **fresh** kit for each instance: the chain a signer holds must not carry
/// records from an earlier case.
#[derive(Clone)]
pub struct ProofKit {
    pub signer: Arc<dyn ProofSigner>,
    pub trusted: TrustedKeys,
}

impl<B> Instance<B> {
    /// An instance with no hook.
    pub fn new(backend: B, clock: Arc<dyn SandboxClock>) -> Self {
        Self {
            backend,
            clock,
            hooks: Arc::new(NoHooks),
            proof: None,
        }
    }

    /// Offer a proof signer to the suite.
    #[must_use]
    pub fn with_proof_kit(mut self, kit: ProofKit) -> Self {
        self.proof = Some(kit);
        self
    }

    /// Offer hooks to the suite.
    #[must_use]
    pub fn with_hooks(mut self, hooks: Arc<dyn Hooks>) -> Self {
        self.hooks = hooks;
        self
    }
}

/// What the backend keeps under an operation's idempotency key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerState {
    /// Nothing is recorded under the key.
    Absent,
    /// The call began and no result was kept: the program may or may not have run.
    MarkerOnly,
    /// A result is kept.
    ResultStored,
}

/// Optional ways to look inside an instance, or to push it into a state a case needs. Every
/// method defaults to "not offered"; a case that needs a hook the target does not offer is
/// reported as not applicable.
#[async_trait]
pub trait Hooks: Send + Sync {
    /// Move the instance's clock forward by `ms` (or wait that long, for a backend on real
    /// time). `false` when the target cannot.
    async fn advance_clock(&self, _ms: u64) -> bool {
        false
    }

    /// Let the backend sweep what is past its `ttl` or its retention, then count the
    /// resources that are still there although they should be gone. `None` when the target
    /// cannot tell.
    async fn residue_after_sweep(&self) -> Option<usize> {
        None
    }

    /// How many sandboxes (execution resources, environments, files) the backend holds right
    /// now. `None` when the target cannot tell.
    async fn live_resources(&self) -> Option<usize> {
        None
    }

    /// How many times a program was really run, all sandboxes together. `None` when the
    /// target cannot tell.
    async fn executions(&self) -> Option<u64> {
        None
    }

    /// Arm a host crash for the next `exec` that gets to run: the backend writes its execution
    /// marker, then dies before keeping a result. `false` when the target cannot.
    async fn arm_crash_after_marker(&self) -> bool {
        false
    }

    /// What the backend keeps under the key of `step` of `ctx`. `None` when the target
    /// cannot tell.
    async fn marker(&self, _ctx: &SandboxContext, _step: SandboxStep) -> Option<MarkerState> {
        None
    }

    /// How long the backend keeps `(key -> request digest, result)`, in milliseconds. `None`
    /// when the target cannot tell.
    async fn retention_ms(&self) -> Option<u64> {
        None
    }

    /// Make a secret resolvable for `tenant` under `reference`. `false` when the target
    /// cannot.
    async fn provide_secret(&self, _tenant: &TenantId, _reference: &str, _value: &str) -> bool {
        false
    }

    /// Make an artifact of `tenant` available as an input. `false` when the target cannot.
    async fn provide_artifact(
        &self,
        _tenant: &TenantId,
        _artifact: &ArtifactRef,
        _bytes: &[u8],
    ) -> bool {
        false
    }

    /// The names of the environment variables every program starts with when nothing was
    /// granted (the set the profile declares). `None` when the target cannot say.
    async fn base_environment(&self) -> Option<Vec<String>> {
        None
    }
}

/// The hooks of a target that offers none.
pub struct NoHooks;

#[async_trait]
impl Hooks for NoHooks {}

/// A destination a probe tries to reach. The target decides which name and address stand for
/// it, and arranges for the destination to behave as described.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NetTarget {
    /// A public host on the allow list.
    Allowed,
    /// A public host that is *not* on the allow list.
    OffList,
    /// A host on the allow list that redirects to a host that is not.
    RedirectOffList,
    /// A host on the allow list that starts a chain of redirects longer than the backend
    /// follows (five in the simulator).
    RedirectLoop,
    /// A name on the allow list that resolves to a private IPv4 address.
    PrivateV4,
    /// A name on the allow list that resolves to a loopback address.
    Loopback,
    /// A name on the allow list that resolves to a link-local IPv4 address.
    LinkLocalV4,
    /// A name on the allow list that resolves to a unique-local IPv6 address.
    UniqueLocalV6,
    /// A name on the allow list that resolves to an IPv4-mapped IPv6 address of a private
    /// IPv4 address.
    MappedPrivateV6,
    /// A name on the allow list that resolves to an IPv4-mapped IPv6 address of a *public*
    /// IPv4 address: refused too.
    MappedPublicV6,
    /// A name on the allow list that resolves to a translated (NAT64) IPv6 address.
    TranslatedV6,
    /// A name on the allow list that resolves to an IPv6 address of the documentation block
    /// (`2001:db8::/32`), inside `2000::/3` but special.
    DocumentationV6,
    /// A name on the allow list that resolves to the IPv4 metadata endpoint.
    MetadataV4,
    /// A name on the allow list that resolves to the IPv6 metadata endpoint.
    MetadataV6,
    /// A name on the allow list that answers a first resolution with a public address and a
    /// second one with a loopback address.
    Rebinding,
    /// A name on the allow list that answers the first two resolutions with a public address
    /// and a third one with a loopback address.
    RebindingLate,
    /// A public host that is on the allow list, but on a port that is not: the list names
    /// host and port together.
    AllowedOtherPort,
    /// A host on the allow list that relays traffic to places the list does not name. The
    /// contract cannot forbid it: the suite only measures what the backend does.
    Relay,
}

/// A behavior the suite asks the target to provoke.
///
/// Unless said otherwise, the command exits with status 0 when the behavior was observed and
/// with another status otherwise, and it runs under [`probe_limits`](crate::conformance::probe_limits)
/// (or under the limits the case states).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeKind {
    /// A plain command that finishes at once: it writes the file `out/result.txt`, prints
    /// something on its standard output and on its standard error, and exits 0. The file and
    /// each stream hold **more than one byte**: the cases check that the delivery is not empty
    /// and that it is cut when the bounds are one byte. Every case that runs something uses
    /// it, so a target that offers none makes them all not applicable.
    Success,
    /// Runs until the wall-time limit is passed.
    WallTime,
    /// Stays running for about a second, well inside the wall-time limit of
    /// [`probe_limits`](crate::conformance::probe_limits), then exits 0. A target whose
    /// programs cannot be caught while they run (a simulator that runs them in one step) offers
    /// none.
    Blocking,
    /// Uses more memory than [`probe_limits`](crate::conformance::probe_limits) allows.
    Memory,
    /// Starts more processes than [`probe_limits`](crate::conformance::probe_limits) allows.
    Processes,
    /// Writes more than the ephemeral disk of [`probe_limits`](crate::conformance::probe_limits).
    Disk,
    /// Writes far more on its standard output than the output bound of
    /// [`probe_limits`](crate::conformance::probe_limits).
    HugeOutput,
    /// Prints the names of all its environment variables on its standard output, one per
    /// line, and exits 0.
    ListEnvironment,
    /// Prints the value of the environment variable `name` on its standard output and on its
    /// standard error, writes it to `out/secret.txt`, and exits 0.
    ReadSecret {
        /// The variable to read.
        name: String,
    },
    /// Copies its standard input to its standard output and exits 0.
    EchoStdin,
    /// Prints its working directory, relative to the working root, on its standard output and
    /// exits 0.
    PrintWorkingDirectory,
    /// Prints the content of the file at `path`, relative to the working root, on its standard
    /// output and exits 0.
    ReadInput {
        /// The file to read.
        path: String,
    },
    /// Creates `out/link`, a symbolic link whose target is outside the working root, and
    /// exits 0 (or exits non-zero when the sandbox does not let it make the link).
    OutgoingLink,
    /// Creates `out/a/link`, a symbolic link two directories down whose target is outside the
    /// working root, and exits 0 (or non-zero when the sandbox does not let it make the
    /// link).
    OutgoingLinkDeep,
    /// Connects to the destination, following redirects. **Exit status 0** when the
    /// connection was established, **1** when the sandbox refused it (a network rule, an
    /// address it must not reach, too many redirects...). Any other status, or a kill, is a
    /// failure of the probe itself and fails the case. The probe must give up quickly: a
    /// refusal that takes the whole wall-time limit to come is a failure too.
    Connect(NetTarget),
    /// Resolves the name of the destination. **Exit status 0** when it obtained an address,
    /// **1** when the sandbox refused; anything else is a failure of the probe.
    Resolve(NetTarget),
}

/// The command a target gives for a [`ProbeKind`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeProgram {
    /// Program followed by its arguments, as put in the `exec` request.
    pub argv: Vec<String>,
    /// The network rules the sandbox must be created with for the probe to be meaningful:
    /// the destinations the probe may legitimately reach. Empty for a probe that needs no
    /// network.
    pub allow: Vec<HostRule>,
}

impl ProbeProgram {
    /// A command with no network rule.
    pub fn new<I, S>(argv: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            argv: argv.into_iter().map(Into::into).collect(),
            allow: Vec::new(),
        }
    }

    /// The rules the sandbox is created with.
    #[must_use]
    pub fn allowing(mut self, rules: impl IntoIterator<Item = HostRule>) -> Self {
        self.allow = rules.into_iter().collect();
        self
    }
}
