# ADR 0054 — A sandbox runs the command behind a port

- Status: **Accepted** — 2026-10-28, by the owner. One point remains open for the owner: how the
  sandbox events reach the run event stream (see Consequences and Open points). Nothing here is
  implemented yet; the code lands in small batches, one per pull request.
- Date: 2026-10-28
- Deciders: the owner
- Replaces (partially): [0024](./0024-governed-virtual-filesystem-seam.md) §5 (the `execute` seam).
  Of that section, the future `CommandExecutor` reached by a URL is replaced by a trait. The other
  guarantees of §5 stay: no process in the engine, and every execution is approval-gated.
- Relates to: [0032](./0032-secrets-redaction-and-no-log.md) (redaction), [0038](./0038-replay-as-evidence.md) /
  [0041](./0041-host-tools-on-catalog-runs-replayed-and-attested.md) (the replay journal: a host tool is never re-run on replay),
  [0044](./0044-cooperative-run-cancellation.md) (cooperative cancellation),
  [0051](./0051-the-signer-sees-and-signs-the-call.md) (the signature covers the exact call)

## Context

ADR 0024 §5 kept shell execution out of the engine and foresaw a future `CommandExecutor`, reached
through a configured URL. A URL is a weak boundary: it carries no types, no lifecycle, no
idempotence, no limits, and nothing a second implementation could be tested against.

Agents need to run commands (build a file, run a script on collected inputs). Doing so is the most
sensitive action the engine can offer. The engine must keep its guarantees around it: deterministic
by default, every run resumable, a checkpoint after every node, an event for every transition,
human gates that suspend cleanly, a node that is re-run on resume, a host tool that is never
re-run on replay, and tenants that never meet.

## Decision

Add a crate `ailu-sandbox` that defines the execution boundary as a **trait**, behind a
**guard** that lives in the engine. The engine never spawns a process itself: it contains no
`std::process::Command`. The port knows no orchestrator, no provider and no isolation technology;
it speaks of an opaque *isolation profile* that the deployment resolves.

Principles:

1. Every execution is a **sensitive action**. It passes through an approval gate bound to the exact
   call (ADR 0051). An agent never approves its own output nor its own execution.
2. **Fail closed.** When in doubt (policy, tenant, path, state), the answer is a typed refusal.
3. The contract is proved by a **conformance suite** that runs against any backend.

### 1. Shape: one crate, one trait, one guard

- `ailu-sandbox` depends on `ailu-graph-core`, `ailu-artifact-store`, `ailu-approval-engine`,
  `async-trait`, `serde`, `thiserror` and `sha2`. It has no runtime dependency: no `tokio` outside
  dev, no HTTP client.
- `trait SandboxBackend` (async, `Send + Sync`, in the style of `FilesystemBackend`) has four
  operations: `create`, `exec`, `collect`, `destroy`. The backend executes; it does **not** decide
  policy.
- `GovernedSandbox<B: SandboxBackend>` is the **guard**, in the engine. It validates the request,
  applies the deployment policy, requires the approval, normalizes paths, recomputes digests, emits
  events and signs the proof.
- `SandboxHandle` and `IdempotencyKey` can only be constructed by the guard (crate-private
  constructors). A backend never builds a handle: `create` returns a raw `RawCreated { sandbox_id,
  isolation_domain }`, and the guard wraps it into a `SandboxHandle` carrying the tenant of the
  `SandboxContext`. A backend forges neither a key nor a handle, so no later operation can be
  addressed to it except through the guard; this non-constructibility, not the fact that the trait is
  exported, is what keeps calls from going around the guard. A key or a handle presented by another
  tenant yields `NotFound`.
- `TenantId` is a new opaque type defined in the `ailu-sandbox` crate.
- Features `sim` (the simulated backend, section 9) and `conformance` (the suite, section 10).
  Neither is on by default.

### 2. Identity, context, idempotence

```rust
pub struct SandboxContext {      // supplied by the host, NEVER by the agent
    pub tenant_id: TenantId,     // new opaque type of ailu-sandbox
    pub run_id: RunId,
    pub node_id: NodeId,
    pub attempt: u32,            // node attempt, 1-based (as in NodeFailed)
}
```

An `IdempotencyKey` is made of `(tenant, run, node, attempt)` plus a `step`. Its canonical form
prefixes each segment with its length (`"6:tenant1|3:run|…"`) and then takes a sha256 hex digest;
never a bare concatenation, so `("a|b", "c")` and `("a", "b|c")` give distinct keys.

- `step` is fixed by the contract and the key is passed the **same way** to every operation:
  `create` = 0, `exec` = 1, `collect` = 2, `destroy` = 3. `step` is stable only if the node
  handler is deterministic (the same sequence of calls on every replay). A non-deterministic handler
  breaks idempotence, and the guard detects it as `IdempotencyConflict`.
- Per call, in the backend and in the guard: the same key with the same request digest returns the
  result already obtained (or joins the execution in progress) **without re-running**; the same key
  with a different digest is `IdempotencyConflict`; `destroy` is idempotent (destroying an absent or
  already destroyed sandbox of that tenant succeeds).
- **Persistence.** Idempotence **beyond `destroy`** (late replay, at-least-once delivery) requires
  the backend to keep, in durable storage isolated per tenant, at least
  `(key → request digest, result, proof)` for a declared retention (at least the resume window of a
  run). Without it the backend must refuse (`Unavailable { retriable: false }`) rather than
  re-execute. A backend without durable storage declares the capability absent and may only serve
  tests.
- **Resume.** Resuming after an approval gate re-runs the node with the **same** `attempt`, hence
  the same keys, hence no double effect. A **retry** has a new `attempt`, hence a new sandbox, which
  is intended.
- **Replay** (ADR 0038/0041). The result of an `exec` is recorded in the run journal under
  `(name = "sandbox.exec", input digest)`. On replay the port is **never** called: the journal
  answers, and an entry with no match is a divergence that fails loudly.

### 3. Lifecycle

**v1: one sandbox runs one `exec`, and a node runs one `exec` per attempt**, so a node needs a
single approval and its resume is coherent. Several `exec` in a node may be allowed later only if
**one approval covers the whole execution plan of the node** (an ordered list of `argv` inside the
approved digest). States: `Created → Executing → Executed → Collected → Destroyed`; a second `exec`
is `InvalidState`. Files are therefore not promised from one `exec` to another. `create` only
**reserves** (quota, isolation domain, `ttl`); a backend may create the execution resource at
`exec`. Several `exec` sharing a filesystem would be an extension announced by a backend capability,
never implicit. A `Poisoned` state (isolation violation, section 8) accepts only `destroy`.

| Operation                    | Input                                                         | Output                                          |
| ---------------------------- | ------------------------------------------------------------- | ----------------------------------------------- |
| `create(ctx, CreateRequest)` | profile, limits, network, input artifact refs, secrets, `ttl` | `SandboxHandle`                                 |
| `exec(handle, ExecRequest)`  | program and arguments, logical working dir, stdin             | `ExecOutcome`                                   |
| `collect(handle, request)`   | output patterns to retrieve                                   | `Collected` + `ExecutionProof` (signed by guard) |
| `destroy(handle)`            | none                                                          | `()`                                            |

- `ExecRequest { argv: Vec<String>, cwd: LogicalPath, stdin: Option<Vec<u8>> }`. `argv` is non-empty
  and `argv[0]` is the program. It is **never a string interpreted by a shell**. Launching a shell
  is a program like any other and needs the same call-bound approval.
- **A sandbox lives at most one node attempt.** The guard offers `with_sandbox(ctx, req, |s| …)`,
  which destroys on exit (success, failure, cancellation, suspension). `ttl` is mandatory at
  creation and capped by the deployment; it is a **sweep** if the host crashes before `destroy`, so
  nothing is left behind.
- A sandbox is **not** in the checkpoint. It is rebuilt by replaying the node (idempotently).
  No sandbox is held while a node is suspended.

### 4. Limits and request bounds

```rust
pub struct Limits { cpu_millicores: u32, memory_bytes: u64, wall_time_ms: u64,
                    output_bytes: u64 /* per stream */, max_processes: u32,
                    ephemeral_disk_bytes: u64 }
pub struct RequestBounds {  // bounds on the REQUEST itself, deployment ceilings
    max_argv_count: u32, max_argv_bytes: u32, max_stdin_bytes: u32,
    max_exec_per_sandbox: u32 /* v1: 1 */, max_collect_patterns: u32, max_collect_files: u32,
    max_collect_bytes: u64, max_ttl_ms: u64, max_input_bytes: u64, max_input_files: u32,
}
```

- `Limits::default()` is **low** (proposed: 250 millicores, 256 MiB, 30 s, 1 MiB per output stream,
  32 processes, 64 MiB of disk).
- The **ceiling** comes from the deployment policy (`SandboxPolicy::ceiling`), never from the agent.
  A request above the ceiling is **refused** (`PolicyDenied::LimitAboveCeiling`), not silently
  clamped. The **effective** limits appear in the proof.
- A limit exceeded during execution: the backend kills and returns
  `Err(LimitExceeded { kind, partial })`, `kind` in `{WallTime, Memory, Processes, Disk}`, with the
  partial `ExecOutcome` so a proof can still be produced. Output past its bound is **truncated**:
  `Ok` with a typed `Truncation` marker, not an error, and never unbounded memory on the host.
- Proposed `RequestBounds` values (to be tuned): 64 arguments, 32 KiB of `argv`, 64 KiB of stdin,
  1 `exec` per sandbox, 8 collect patterns, 256 files, 16 MiB collected, 15 min maximum `ttl`.
  A request outside the bounds is refused by the guard (`PolicyDenied::RequestTooLarge`) before it
  reaches the backend.

### 5. Network

- `NetworkPolicy::Deny` is the default. `NetworkPolicy::Allow(Vec<HostRule>)` with
  `HostRule { host, port }`: exact names, no IP wildcard, no range.
- The requested list is **intersected** with the deployment's; a rule outside the policy is
  `PolicyDenied::NetworkRule`. The guard treats `Allow` as sensitive: the list is part of the
  approved digest.
- Backend obligations, checked by the suite: a host off the list is refused; a redirect to a host
  off the list is refused; a DNS resolution to a private, loopback or metadata address is refused;
  DNS is refused in `Deny` mode; **DNS rebinding** is refused (resolve once, **pin** the address for
  the connection, refuse a divergent second resolution); **IPv6** and **IPv4-mapped IPv6**
  (`::ffff:a.b.c.d`) obey the same prohibitions as IPv4; the metadata endpoint in IPv4 and IPv6 and
  link-local and unique-local ranges are refused.
- Accepted limit: an allowed host that acts as a **proxy or relay** (open redirect, CONNECT, tunnel)
  bypasses the list, and the contract cannot know it. Mitigation: the list is approved by name with
  the call, and the suite includes a simulated relaying host to measure what a backend does with it.

### 6. Files and secrets

Files:

- Inputs are **artifact references** (`ArtifactRef`) of the current run. Destination paths are
  normalized by the **same rule** as the virtual filesystem of ADR 0024 (reject `..`, absolute
  paths, backslashes, NUL bytes; an outgoing symlink is refused), giving `InvalidPath`. A reference
  to another run or tenant is `NotFound`, with no disclosure.
- Inputs are **materialized by the backend** (the contract does not say how) and **bounded** by
  `max_input_bytes` (proposed default 4 MiB in total) and `max_input_files`; beyond them,
  `PolicyDenied::InputTooLarge`, before any execution.
- `CreateRequest` has **no host volume mount variant**: the prohibition is in the type, not only in
  the policy.
- Outputs are retrieved by `collect` through patterns under the working directory, **bounded** in
  count and size (over the bound: truncated and marked), written to the run's artifact store and
  returned as `ArtifactRef` plus `sha256`. The sandbox disk is ephemeral and under quota.

Secrets:

- The initial environment is **empty** except for a fixed set declared by the profile (for example
  `LANG`, a minimal `PATH`), and that set is recorded in the proof. No secret of the host is
  inherited, and the suite tests it.
- A secret enters only through `EnvGrant { name, secret_ref }` declared in `CreateRequest` and
  **covered by the approval** (the list of names is in the approved digest). The value is resolved
  by a host-provided `SecretResolver`, on the backend side, which **receives the tenant**
  (`resolve(&SandboxContext, &SecretRef)`) and refuses another tenant's reference. The value never
  crosses the request, the proof, the events or the journal (ADR 0032).
- The `subject` of `ApprovalRequired` never contains `stdin` in clear (only its size and digest),
  nor a secret value; `argv` appears redacted.

### 7. Tenant isolation and violations

- **Two tenants never share a sandbox or an isolation domain.** `create` returns an opaque
  `isolation_domain`, the proof records it, and the suite checks that two tenants never share one.
  A domain is never reused between tenants, even after `destroy` (recycling within one tenant is
  allowed).
- The tenant comes from `SandboxContext`. A `tenant` field supplied by the agent is ignored or
  rejected by the type.
- Per-tenant admission: ceilings on concurrent sandboxes and on creation rate (`SandboxPolicy`).
  Beyond them, `Err(QuotaExceeded { kind })` returns **immediately**, never a silent wait, and other
  tenants stay served.
- A violation detected by the backend (escape attempt, repeated denied access to a forbidden
  resource) is `Err(IsolationViolation { signal })`: the sandbox is `Poisoned`, the proof is marked
  `violation`, and `destroy` is required. The contract does **not** claim that a given backend
  detects everything; it fixes what must happen when something is detected.

### 8. Typed errors and events

```rust
pub enum SandboxError {
    PolicyDenied(PolicyReason),
    ApprovalRequired { subject: Value },    // the caller files the request and suspends (Interrupt)
    QuotaExceeded { kind: QuotaKind },
    LimitExceeded { kind: LimitKind, partial: Box<ExecOutcome> },
    IsolationViolation { signal: String },
    IdempotencyConflict,
    InvalidPath { reason: String },
    NotFound,                               // includes a different tenant
    InvalidState { from: SandboxState },
    Cancelled,                              // ADR 0044, at node and exec boundaries
    Busy { retry_after_ms: Option<u64> },   // a wait: retriable, not an error
    Unavailable { retriable: bool },
    Backend(String),                        // last resort, redacted message
}
```

- **Stable code.** `SandboxError::code(&self) -> &'static str` returns frozen values
  (`policy_denied`, `approval_required`, `quota_exceeded`, `limit_exceeded`, `isolation_violation`,
  `idempotency_conflict`, `invalid_path`, `not_found`, `invalid_state`, `cancelled`, `busy`,
  `unavailable`, `backend_error`); subtypes carry a code too (`limit_exceeded.wall_time`,
  `quota_exceeded.tenant_concurrency`). Adding a code is compatible; changing or removing one
  breaks. No consumer parses a message, and the suite checks the code, not the text.
- Error (final), wait (`Busy`) and unavailability (`Unavailable`) are distinct; the caller retries
  only the last two.
- **Redaction.** The guard applies ADR 0032 redaction to `stdout`, `stderr` and collected artifacts
  **before** storing or exposing them. The raw bytes are kept only if the deployment policy asks,
  and stay out of logs and telemetry.
- **Trait sketch** (to be frozen at the first batch). `CancelToken` is a new type wrapping the
  cooperative check of ADR 0044 (`CancelCheck`); a backend consults it during `create`, `exec` and
  `collect` and returns `Cancelled`; `destroy` is not cancellable. `SandboxClock` is a new trait,
  distinct from the `Clock` trait that already exists in `graph-runtime`; it is injected into the
  guard and the backend, and the engine does not read the clock.

```rust
#[async_trait]
pub trait SandboxBackend: Send + Sync {
    async fn create(&self, ctx: &SandboxContext, key: &IdempotencyKey, req: CreateRequest,
                    cancel: &CancelToken) -> Result<RawCreated, SandboxError>;
    async fn exec(&self, h: &SandboxHandle, // built by the guard
                   key: &IdempotencyKey, req: ExecRequest,
                  cancel: &CancelToken) -> Result<ExecOutcome, SandboxError>;
    async fn collect(&self, h: &SandboxHandle, key: &IdempotencyKey, req: CollectRequest,
                     cancel: &CancelToken) -> Result<RawCollected, SandboxError>;
    async fn destroy(&self, h: &SandboxHandle, key: &IdempotencyKey) -> Result<(), SandboxError>;
}
```

  `RawCreated` and `RawCollected` are raw types a backend can build. `RawCollected` carries the bounded bytes and the values the backend declares. The guard redacts
  them, stores them in the artifact store, recomputes the digests, and derives `Collected` and the
  `ExecutionProof`. A backend signs nothing and approves nothing.
- **Events.** One transition, one event, durably journaled with the node events:
  `sandbox_created`, `sandbox_exec_started`, `sandbox_exec_completed`, `sandbox_exec_failed`,
  `sandbox_collected`, `sandbox_destroyed`, `sandbox_denied`. They carry digests and identifiers,
  **never content**. They are nested inside `node_started … node_completed`. The events of a
  replayed node are deduplicated (same key, same event) or, if re-emitted, marked `replayed: true`.
  The semantics of existing node events do not change.

### 9. The simulated backend (`sim`)

ADR 0024 forbids launching any process in the engine, so the reference backend is **entirely
in-memory and deterministic**, and runs **no** real program:

- "programs" are registered Rust functions (`SimProgram`) over a virtual filesystem and an injected
  virtual `SandboxClock`;
- injectable faults: infinite loop (wall time), unbounded allocation, huge output, process
  explosion, disk fill, outage and unavailability, isolation violation;
- network: a simulated host table, with redirects and private resolutions, to test the policy.

It proves the **semantics of the contract** (states, idempotence, policy, quotas, errors, proof,
events). It does **not** prove the real isolation of a production backend; that is covered by the
same suite run against that backend.

### 10. The conformance suite (`conformance`)

`run_conformance(target)` runs every case against a `ConformanceTarget`: a backend factory plus
`probe(kind)`, which supplies, for a real backend, the command that provokes each behavior; `None`
marks the case *not applicable*, never *passed*. A case is *passed* when the attack **fails** in a
typed and journaled way. Groups:

- **Lifecycle**: state order; a second `exec` and a `collect` before `exec` are `InvalidState`;
  `destroy` is idempotent; `ttl` is swept; no residue after a simulated host crash.
- **Idempotence**: 20 parallel `exec` with one key give one execution and one result; digest
  conflict; resume after interruption; replay without any backend call.
- **Limits**: time, memory, processes, disk, truncated output; a request above the ceiling is
  refused.
- **Network**: default deny, DNS, off-list host, redirect, private IP, metadata; DNS rebinding,
  IPv6, IPv4-mapped, IPv6 metadata, relaying host.
- **Secrets and privileges**: empty environment, nothing inherited, an uncovered secret refused, the
  value absent from events, proof and logs.
- **Paths**: `..`, absolute, backslash, NUL, outgoing link; a reference to another run or tenant is
  `NotFound`.
- **Multi-tenant**: two tenants in parallel never share an isolation domain; per-tenant quota with
  immediate refusal and fairness.
- **Governance**: execution without approval is `ApprovalRequired` and nothing runs; the approval is
  bound to the exact call (changing one argument invalidates the grant); approver differs from
  requester (`SelfApproval`).
- **Proof**: valid chained signature; forging an output changes its digest and invalidates the
  proof.
- **Events**: one event per transition, in order, without content; a replayed node is deduplicated
  or marked `replayed`.
- **Keys and handles**: a key or handle of another tenant is `NotFound`; keys whose naive
  concatenation would collide stay distinct; idempotence still holds after `destroy`; cancellation
  through `CancelToken`.
- **Request bounds**: `argv`, stdin, inputs, collect patterns and `ttl` out of bounds are
  `PolicyDenied`.
- **Engine invariants**: checkpoint after the node (the sandbox is not in the state), resume from
  the last checkpoint, clean suspension on the gate (`run_suspended`) with no sandbox held,
  cancellation at boundaries.

The simulated backend must pass **100 %** of the suite. A production backend must pass every
applicable case and list the not-applicable ones in its report.

### 11. Execution proof and attestation chain

`ExecutionProof` is a canonical record holding digests only: `run_id`, `node_id`, `tenant_id`,
`sandbox_id`, `idempotency_key`, `isolation_domain`, `request_digest` (argv, cwd, stdin, network,
named secrets, requested limits), `profile_digest`, `limits_effective`, `inputs_digest` (sha256 over
sorted path/sha256 pairs), `outputs_digest`, `stdout_digest`, `stderr_digest`, `truncated`, `exit`
(code, signal or kill cause), `duration_ms`, `started_at` / `finished_at`, `violation`, and
`approval` (call key and approver).

- **Two chains, not one.** The approval attestation has a frozen shape, and its verifier would
  reject a mixed chain. The proof keeps the **same primitives** (JavaScript-style canonical JSON,
  Ed25519, `prev_hash`) but forms a **separate sandbox chain per run**, with a distinct record type
  whose `kind: "sandbox_execution"` field is **signed**. Each proof carries the `payload_hash` of the
  approval that authorized it (`approval_payload_hash`), which links the two chains without mixing
  them.
- **Keys.** A proof is verified against **verifier-pinned keys**, identified by a `kid`, with
  rotation (several valid `kid`s, revocation), never against a key embedded in the record.
- **Declared values.** `exit`, `duration_ms`, `started_at` and `finished_at` are **declared by the
  backend**; the proof names them `declared_*` and does not guarantee them.
- **Who signs.** The **guard**, in the engine, with the host key, never the backend. The guard
  **recomputes** the digests of the outputs it stores, so a backend that forges an output is
  detected. It cannot verify what the backend claims about its own isolation profile: the proof
  attests *what was declared*, not the strength of the isolation.
- The replay journal references the proof; a proof holds no content and no secret value.

### 12. Runtime invariants

1. **Checkpoint after every node.** The node state holds only artifact references and the proof
   digest; the sandbox is disposable.
2. **One event per transition** (section 8).
3. **Human gates.** The approval precedes `create`. Without a valid grant the node returns
   `NodeOutput::interrupt` and the run emits `run_suspended`. On resume the node re-runs, the keys
   are identical, and the call-bound approval unblocks. v1 has one `exec` per node, so a single
   approval to resume.
4. **Determinism and replay.** A replay reads the journal and does not call the port.
5. **Cancellation.** Observed at boundaries; `destroy` in every case.

Implementation plan (small batches, one concern per pull request): (1) types, errors, idempotency
key, paths; (2) trait, guard, policy; (3) simulated backend; (4) conformance suite; (5) proof and
attestation; (6) events.

## Consequences

- The engine gains an execution boundary without gaining a process. Any isolation technology can
  sit behind `SandboxBackend`; none is named or favored here.
- Safety rests on the guard and on the suite, not on trust in a backend. A backend that fails an
  applicable case is not conformant.
- **Events.** `RunEvent` is not `#[non_exhaustive]`, and consumers match on it exhaustively, as do
  the typed event schemas of the SDKs. Adding a `RunEvent::Sandbox` variant therefore breaks them.
  The sandbox events are **not exposed on the run event stream** until `RunEvent` is marked
  `#[non_exhaustive]`, which is planned for the next major version; the variant then follows in
  that same version. Until then the guard keeps its transitions as its own durable record. The
  addition is not made in a minor version.
- No binding and no TypeScript or Python layer is part of this ADR. Any exposure to another language
  is a separate decision, and parity is proved on the same conformance suite.
- Idempotence beyond `destroy` puts a durable, tenant-isolated store on the backend's critical path;
  a backend without it is limited to tests.
- v1 is deliberately narrow (one `exec` per sandbox and per node). It trades flexibility for a
  single approval and a coherent resume.
- The proof attests declarations, not isolation strength. Reviewers must not read it as a
  guarantee about the backend.
- The two attestation chains double the verification surface by one, in exchange for leaving the
  approval chain and its verifiers untouched.

## Alternatives considered

- **Keep the URL seam of ADR 0024 §5.** Rejected: untyped, no lifecycle or idempotence contract,
  and no way to test two implementations against one definition.
- **Run processes inside the engine.** Rejected: it breaks the rule that the engine spawns nothing
  and puts isolation in the same trust domain as the runtime.
- **Several `exec` per sandbox from v1.** Rejected for now: it needs an approval over a whole plan
  and a shared-filesystem capability; the key format already leaves room (`step`).
- **One attestation chain for approvals and executions.** Rejected: the approval record has a frozen
  shape and its verifiers would reject a mixed chain. A separate chain linked by
  `approval_payload_hash` keeps both verifiable.
- **Let the backend sign the proof.** Rejected: the guard must recompute output digests and a
  backend must not vouch for itself.
- **Clamp requests above the ceiling.** Rejected: silent clamping hides what actually ran; a refusal
  is explicit.
- **A real-process reference backend.** Rejected: ADR 0024 forbids it in the engine. An in-memory
  simulator proves the contract semantics; real isolation is proved by running the suite against a
  real backend.

## Non-goals

- Naming, choosing or recommending any isolation technology or orchestrator.
- Any binding, TypeScript or Python layer, or product-side integration.
- Multi-`exec` sandboxes and shared filesystems between executions.
- Proving the isolation strength of any backend.
- Changing the semantics of existing node events or of the approval attestation chain.

## Open points

- How the sandbox events reach the run event stream: mark `RunEvent` `#[non_exhaustive]` in the next
  major version and add the variant there, or cut a dedicated major version. Neither path is
  neutral, and the addition is not made in a minor version. The exact behavior of the SDKs facing an
  unknown event is not yet verified.
- The numeric defaults of sections 4 and 6 (limits, request bounds, input size) are proposals and
  have not been measured.
- The trait signatures are a sketch, to be frozen in the first batch. Nothing here has been compiled
  or executed.
