# ADR 0054 — A sandbox runs the command behind a port

- Status: **Accepted** — 2026-10-28, by the owner. The sandbox events reach the run event stream
  only once `RunEvent` is `#[non_exhaustive]`, at the next major version; until then they live in
  the guard's own durable record (see Consequences). The first two batches (types, errors,
  idempotency key and paths; then the trait, the guard and the policy) exist as pull requests (#377,
  #378); the rest lands in small batches, one per pull request.
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
re-run on replay, and tenants kept apart (an obligation on the backend and the host, which the guard
checks where it can).

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

- `ailu-sandbox` depends on `ailu-graph-core`, `ailu-fs-backend` (to reuse `normalize_path`, the
  same rule as the virtual filesystem), `serde`, `serde_json`, `thiserror`, `sha2` and `zeroize`.
  `async-trait` (for the traits) joins with the guard. `ailu-approval-engine` and
  `ailu-artifact-store` are **not** dependencies: the guard reaches approvals through the
  `GrantVerifier` port and records its transitions through the `SandboxJournal` port, and until the
  run's artifact store is wired in, `Collected` carries the redacted bytes in memory. The crate has
  no runtime dependency: no `tokio` outside dev, no HTTP client.
- `trait SandboxBackend` (async, `Send + Sync`, in the style of `FilesystemBackend`) has four
  operations: `create`, `exec`, `collect`, `destroy`. The backend executes; it does **not** decide
  policy.
- `GovernedSandbox<B: SandboxBackend>` is the **guard**, in the engine. It validates the request,
  applies the deployment policy, requires the approval, normalizes paths, recomputes digests, emits
  events, builds the proof and has it signed through the `ProofSigner` port (section 11).
- `SandboxHandle` and `IdempotencyKey` can only be constructed by the guard (crate-private
  constructors). A backend never builds a handle: `create` returns a raw `RawCreated { sandbox_id,
  isolation_domain }`, and the guard wraps it into a `SandboxHandle` carrying the tenant of the
  `SandboxContext`. A backend forges neither a key nor a handle, so no later operation can be
  addressed to it except through the guard, **as long as the guard owns the backend** (a private
  field); this non-constructibility, not the fact that the trait is
  exported, is what keeps calls from going around the guard. A key or a handle presented by another
  tenant yields `NotFound`.
- `TenantId` is a new opaque type defined in the `ailu-sandbox` crate.
- Features `sim` (the simulated backend, section 9) and `conformance` (the suite, section 10).
  Neither is on by default.

### 2. Identity, context, idempotence

```rust
pub struct SandboxContext {      // built by the host; never being the agent's is a host convention
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
  result already obtained **without re-running** (a replayed `create` joins the one in progress; for
  `exec`, see below); the same key
  with a different digest is `IdempotencyConflict`; `destroy` is idempotent (destroying an absent or
  already destroyed sandbox of that tenant succeeds).
- **Conflicts and ordering.** The same key with a different digest is `IdempotencyConflict` in
  **every** state; `InvalidState` is only for an operation out of order. Two concurrent `exec` with
  the same key: the second gets `Busy` and only one backend call is made; retried, it receives the same result. After a non-final error
  the state stays `Executing` (the outcome is unknown) and only the same `exec` or `destroy` is
  allowed. `Unavailable { retriable: false }` is never retried.
- **Persistence.** Idempotence **beyond `destroy`** (late replay, at-least-once delivery) requires
  the backend to keep, in durable storage isolated per tenant, at least
  `(key → request digest, result)` for a declared retention (at least the resume window of a run).
  It does **not** keep the proof, which it does not produce. Without it the backend must refuse (`Unavailable { retriable: false }`) rather than
  re-execute. A backend without durable storage declares the capability absent and may only serve
  tests.
- **Retention and attempt age.** The guard gives **every** `exec` and `collect` call a
  `run_started_at`: the instant the run started, a datum of the host, never read from the engine's
  clock. The guard **refuses**, fail closed, any attempt whose `run_started_at` is older than the
  result retention, with the policy reason `PolicyDenied::AttemptTooOld`, **before any effect**. `run_started_at` is
  not passed to the backend. An instant in the future beyond `max_clock_skew_ms` (5 minutes by default) is refused too, with
  `PolicyDenied::AttemptInFuture`.
  The retention is 24 h by default and at most 7 days, a deployment ceiling. The
  reason: a backend that keeps `(key → request digest, result)` for the retention must be
  able to tell a key that never ran from a key whose result has expired; without that it could not
  refuse to re-execute. `create` applies the same age check as `exec` and `collect`. Backend rule: an execution marker present, the result absent and no
  execution resource left means `Unavailable { retriable: false }`, never a re-execution.
- **Serializable records.** `RawCollected` and `ExecOutcome` stay serializable (serde) with bounded
  bytes, because a backend keeps them, encrypted per tenant, for the retention. This is a
  requirement on the types and on the backend; no storage or encryption technology is named.
  `RawCollected` never carries the secrets once persisted (they are not serializable): a backend
  that replays a `collect` must resolve the secrets again through its `SecretResolver`; otherwise
  the guard fails closed (fewer secrets than grants).
- **The proof is idempotent.** The `ProofSigner` must be idempotent by `idempotency_key`: a replayed
  `collect` does not add a second record to the chain.
- **Resume.** Resuming after an approval gate re-runs the node with the **same** `attempt`, hence
  the same keys, hence no double effect. A **retry** gets a new key, hence a new sandbox, only if the
  node supplies a new `attempt` in the context; the engine does not provide one (see Consequences).
- **Replay as evidence** (ADR 0038/0041), distinct from a live resume (above, which does call the
  backend and relies on its idempotence). The result of an `exec` is recorded in the run journal
  under the name `sandbox.exec`, with the digest of the `IdempotencyKey` as input hash. As in
  ADR 0041, the journal is served **in the order of the calls**, checking the input hash, not as a
  `(name, digest)` table. On replay as evidence the port is **never** called: the journal answers,
  and a mismatch is a divergence that fails loudly. Extending that journal to a sandbox node is an
  explicit dependency, to be written separately.

### 3. Lifecycle

**v1: one sandbox runs one `exec`, and a node runs one `exec` per attempt**, so a node needs a
single approval and its resume is coherent. Several `exec` in a node may be allowed later only if
**one approval covers the whole execution plan of the node** (an ordered list of `argv` inside the
approved digest). States: `Created → Executing → Executed → Collected → Destroyed`; a second `exec`
is `InvalidState`. Files are therefore not promised from one `exec` to another. `create` only
**reserves** (quota, isolation domain, `ttl`); a backend may create the execution resource at
`exec`. Several `exec` sharing a filesystem would be an extension announced by a backend capability,
never implicit. A `Poisoned` state (isolation violation, section 8) accepts only `destroy`.

| Guard operation                                                    | Input                                                                        | Output                                           |
| ------------------------------------------------------------------ | ---------------------------------------------------------------------------- | ------------------------------------------------ |
| `create(ctx, CreateRequest, planned: &ExecRequest, claim, cancel)` | profile, limits, network, input artifact refs, secrets, `ttl`, planned exec, claim | `SandboxHandle`                            |
| `exec(handle, ExecRequest, claim, run_started_at, cancel)`         | program and arguments, working dir, stdin                                    | `ExecOutcome`                                    |
| `collect(handle, request, run_started_at, cancel)`                 | output patterns to retrieve                                                  | `Collected` + `ExecutionProof` (signed via port) |
| `destroy(handle)`                                                  | none                                                                         | `()`                                             |

The backend trait (section 8) has the same four operations over raw types; the guard adds the
context, the claim and the host-supplied instants.

**Approval, claim and call key.**

- The approval **precedes `create`** and covers the **whole call**: `create` plus the planned
  `exec`. `exec` re-verifies against the request actually received; changing one argument
  invalidates the grant.
- The caller brings only an `ApprovalClaim { approval_id, call_key }`, never a grant. A `GrantVerifier`
  port (async, injected into the guard) returns the **verified** grant: requester, approver, tenant,
  run, `call_key` and `approval_payload_hash` (supplied by the verifier and carried into the proof).
  The expected host-side implementation is the approval engine or its attestation.
  A grant whose approver is the requester is refused (self-approval).
- The `call_key` binds tenant, run, node and the digests of the `create` and `exec` requests in a
  canonical form with length-prefixed fields. It does **not** depend on `attempt`, so a retry reuses
  the same approval with no new human decision. This is an assumed choice (see Consequences). It
  takes the name form of ADR 0051, `sandbox.exec#<sha256>`, but the hash covers a digest made of
  length-prefixed fields, not the canonical `callInput` JSON of ADR 0051: a verifier must recompute
  it from the request, not from the masked subject.

- `ExecRequest { argv: Vec<String>, cwd: Option<LogicalPath>, stdin: Option<Vec<u8>> }`. `cwd` is relative to the
  working root and `None` means the root. `argv` is non-empty
  and `argv[0]` is the program. It is **never a string interpreted by a shell**. Launching a shell
  is a program like any other and needs the same call-bound approval.
- **A sandbox lives at most one node attempt.** The guard offers
  `with_sandbox(ctx, create, planned, claim, cancel, f)`, where `f` is a
  `FnOnce(&SandboxSession) -> SessionFuture<T>` and the result is a `Result<T, SandboxError>`. It
  creates the sandbox (after the approval check), runs `f`, and destroys on exit (success, failure,
  cancellation, suspension); a destruction error is returned if `f` succeeded. If the returned
  future is dropped, nothing can be awaited: the backend's `ttl` sweep removes the sandbox and the
  guard gives the tenant's slot back at the same instant. `ttl` is mandatory at
  creation and capped by the deployment; it is a **sweep that the backend must perform** if the host
  crashes before `destroy`. A resource still present after its `ttl` is a conformance failure.
- The guard's registry is **bounded**: a capped number of records, eviction after retention plus
  `ttl`, collected bytes dropped at `destroy`. `Destroyed` is terminal. A quota slot is returned
  exactly once, including if the future is abandoned, and a slot's `ttl` expires through the
  injected clock.
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
- Other refusals by the guard, before the backend: a zero `ttl`; `NetworkPolicy::Allow` with an
  empty list; input destinations that are duplicated or nested inside one another; in a request,
  network hosts that are not lowercase ASCII DNS names, and IP literals (a deployment policy may
  allow IP literals through a separate rule). Refusal reasons never echo the content supplied by the
  caller.

### 5. Network

- `NetworkPolicy::Deny` is the default. `NetworkPolicy::Allow(Vec<HostRule>)` with
  `HostRule { host, port }`: exact names, no IP wildcard, no range.
- Each requested rule must be in the deployment's list; a rule outside the policy is refused
  (`PolicyDenied::NetworkRule`), never trimmed. The guard treats `Allow` as sensitive: the list is part of the
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
  paths, backslashes, NUL bytes), giving `InvalidPath`. Refusing an outgoing symlink is an
  obligation of the backend, since the guard cannot see the filesystem. A reference to another run
  or tenant is `NotFound`, with no disclosure; the guard checks this, and the bounds below, once it
  has the run's artifact store (a later batch, not the second one).
- Inputs are **materialized by the backend** (the contract does not say how) and **bounded** by
  `max_input_bytes` (proposed default 4 MiB in total) and `max_input_files`; beyond them,
  `PolicyDenied::InputTooLarge`, before any execution (checked together with the ownership of the
  references, in the same later batch).
- `CreateRequest` has **no host volume mount variant**: the prohibition is in the type, not only in
  the policy.
- Outputs are retrieved by `collect` through patterns under the working directory, **bounded** in
  count and size, written to the run's artifact store (until it is wired in, `Collected` carries the
  redacted bytes) and returned with their `sha256`. Truncating at the bound is an obligation of the
  **backend**; the guard refuses a delivery that is out of bounds. The sandbox disk is ephemeral and under quota.

Secrets:

- The initial environment is **empty** except for a fixed set declared by the profile (for example
  `LANG`, a minimal `PATH`), and that set is recorded in the proof. No secret of the host is
  inherited, and the suite tests it.
- A secret enters only through `EnvGrant { name, secret_ref }` declared in `CreateRequest` and
  **covered by the approval** (the list of names is in the approved digest). The value is resolved
  by a `SecretResolver` that belongs to the deployment and the backend side (it is not defined in
  the guard's crate), which **receives the tenant**
  (`resolve(&SandboxContext, &SecretRef)`) and refuses another tenant's reference. The value never
  crosses the request, the proof, the events or the journal (ADR 0032).
- A value resolved on the backend side is unknown to the guard, so the guard could not redact it.
  The backend therefore **hands the resolved values back to the guard**, in `RawCollected`, inside a
  `ResolvedSecrets` type that does not implement `Serialize`, `Debug` or `Display` and is dropped
  (and zeroized) once the guard has redacted. The guard uses it only for redaction and exposes it
  nowhere else. Redaction by known value stays limited to values the program has not transformed
  (encoded, split, hashed or otherwise altered); it is a safeguard, not a guarantee. It also
  depends on the good faith of the backend: the guard can compare the number of secrets handed back
  with the number of grants, but cannot know that a value is the right one.
- **The approver sees what is approved.** The `subject` of `ApprovalRequired` shows: `argv`
  (arguments truncated with a marker, and redacted by known values), `cwd`, the network hosts and
  ports, the **names** of the environment variables and the secret references (never the values),
  the destination paths of the inputs, the limits and the `ttl`. For `stdin` it shows only its size
  and a short digest. A subject made only of digests would ask the approver to sign what they
  cannot read.

### 7. Tenant isolation and violations

- **Two tenants never share a sandbox or an isolation domain.** This is an **obligation of the
  backend**. `create` returns an opaque `isolation_domain`; the guard records which tenant
  declared each domain **in its bounded in-memory table** and, if a domain declared for one tenant
  is declared again for another, fails with `IsolationViolation`. Beyond that table the guard sees
  nothing. The guard cannot see the real isolation, so the proof records
  the domain **as declared** by the backend. The suite checks that two tenants never get the same
  declared domain. That a domain is never reused between tenants, even after `destroy`
  (recycling within one tenant is allowed), is an obligation of the backend.
- The tenant comes from the `SandboxContext`, which the host builds. That it is never the agent's is
  a convention of the host, not a guarantee of the type.
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
  breaks. No consumer parses a message, and the suite checks the code, not the text. On the wire, every
  serde tag and every error code is snake_case (`not_found`, `policy_denied.*`).
- The guard never relays the free text of a backend, only its code. Variants reserved to the guard
  (for example `PolicyDenied`, `ApprovalRequired`, `IdempotencyConflict`) that a backend returns are
  converted into the generic backend error.
- Error (final), wait (`Busy`) and unavailability (`Unavailable`) are distinct; the caller retries
  only the last two.
- **Redaction details.** Collected paths are redacted and checked against the requested patterns;
  overlapping redaction intervals are merged; secret values are also matched in their hex, base64
  and URL-encoded forms. The guard fails closed if the backend hands back fewer secrets than there
  are grants.
- **Redaction.** The guard redacts **by known values** (the resolved secret values the backend
  hands back, section 6), not by the pattern-based redaction of ADR 0032, in `stdout`, `stderr` and
  collected artifacts **before** storing or exposing them. No raw copy is kept.
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

  `RawCreated` and `RawCollected` are raw types a backend can build. `RawCollected` carries the bounded bytes, the digests the backend declares, the resolved secret
  values (section 6) and the other values the backend declares. The guard redacts the bytes, stores them in the artifact store, recomputes the digests, and derives `Collected` and the
  `ExecutionProof` for signing (section 11). A backend signs nothing and approves nothing.
- **Events.** One transition, one event, written to the **guard's own durable record**, not to the
  `RunEvent` stream before the next major version (see Consequences):
  `sandbox_created`, `sandbox_exec_started`, `sandbox_exec_completed`, `sandbox_exec_failed`,
  `sandbox_collected`, `sandbox_destroyed`, `sandbox_denied`. They carry digests and identifiers,
  **never content**. Their order is guaranteed only within the guard's journal, per key; no order relative to
  `node_started` and `node_completed` is promised while the events are not in `RunEvent`. The events of a
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
  `Destroyed` is terminal; a quota slot is returned once, even if the call is abandoned; the
  registry stays bounded;
  `destroy` is idempotent; the backend sweeps on `ttl`; any residue after a simulated host crash or
  after `ttl` fails the case.
- **Idempotence**: 20 parallel `exec` with one key give one backend execution, the others `Busy`,
  and a retry then returns the same result; a replayed `collect` adds no second chain record; digest
  conflict in every state; two concurrent `exec` with one key give `Busy` for the second and one
  backend call; after a non-final error only the same `exec` or `destroy` is allowed;
  `Unavailable { retriable: false }` is not replayed; an attempt older than the retention, or too far
  in the future, is `AttemptTooOld` or `AttemptInFuture` before any effect; resume after interruption; replay without any
  backend call.
- **Limits**: time, memory, processes, disk, truncated output; a request above the ceiling is
  refused.
- **Network**: default deny, DNS, off-list host, redirect, private IP, metadata; DNS rebinding,
  IPv6, IPv4-mapped, IPv6 metadata, relaying host.
- **Secrets and privileges**: fewer secrets returned than grants fails closed; hex, base64 and URL
  forms of a value are redacted; collected paths are redacted; empty environment, nothing inherited, an uncovered secret refused, the
  value absent from events, proof and logs.
- **Paths**: `..`, absolute, backslash, NUL; an outgoing link (a backend case); a reference to another run or tenant is
  `NotFound`.
- **Multi-tenant**: two tenants in parallel never share an isolation domain; per-tenant quota with
  immediate refusal and fairness.
- **Governance**: execution without approval is `ApprovalRequired` and nothing runs; the approval is
  bound to the exact call (changing one argument invalidates the grant); approver differs from
  requester (`SelfApproval`); a bare claim without a verified grant is refused; `create` without an
  approval covering the planned `exec` is refused; a backend that returns a guard-reserved variant
  or free text sees it converted and not relayed.
- **Proof**: signer unavailable means fail closed and no result is returned; the chain stays
  ordered under concurrent calls; valid chained signature; a delivered output that differs from its declared digest is
  refused; altering a stored output afterwards changes its digest and invalidates the proof. A
  backend that lies consistently about content is out of reach of the suite and is said so.
- **Events**: one event per transition in the guard's durable record, in order, without content; a replayed node is deduplicated
  or marked `replayed`.
- **Keys and handles**: a key or handle of another tenant is `NotFound`; keys whose naive
  concatenation would collide stay distinct; idempotence still holds after `destroy`; cancellation
  through `CancelToken`.
- **Request bounds**: `argv`, stdin, inputs, collect patterns and `ttl` out of bounds are
  `PolicyDenied`.
- **Engine invariants**: checkpoint after the node (the sandbox is not in the state), resume from
  the last checkpoint, clean suspension on the gate (`run_suspended`) with no sandbox held,
  cancellation at boundaries.

Cases fall into three classes:

- **Properties of the guard**: policy, approval, idempotence, registry, redaction, proof, events.
  They hold whatever the backend and run against the guard with any conformant backend.
- **Simulator only**: a backend that lies, crash injection, forged digests. Only the simulated
  backend can be made to behave so.
- **Real backend through `probe`**: isolation, limits, network, residues, sweeps.

The target exposes the hooks the suite needs: inspecting residues (resources still present after
`destroy` or `ttl`), injecting a host crash, and reading the idempotency marker. A backend that
cannot offer a hook marks the dependent cases *not applicable*.

The simulated backend must pass **100 %** of the suite. A production backend must pass every
applicable case and list the not-applicable ones in its report.

### 11. Execution proof and attestation chain

`ExecutionProof` is a canonical record holding digests only: `run_id`, `node_id`, `tenant_id`,
`sandbox_id`, `idempotency_key`, `isolation_domain`, `request_digest` (argv, cwd, stdin, network,
named secrets, requested limits), `profile_digest`, `limits_effective`, `inputs_digest` (sha256 over
sorted path/sha256 pairs), `outputs_digest`, `stdout_digest`, `stderr_digest`, `truncated`, `exit`
(code, signal or kill cause), `duration_ms`, `started_at` / `finished_at`, `violation`,
`approval` (call key and approver), and `approval_payload_hash` (the payload hash of the approval
record that authorized the call).

- **Two chains, not one.** The approval attestation has a frozen shape, and its verifier would
  reject a mixed chain. The proof keeps the **same primitives** (JavaScript-style canonical JSON,
  Ed25519, `prev_hash`) but forms a **separate sandbox chain per run**, with a distinct record type
  whose `kind: "sandbox_execution"` field is **signed**. Each proof carries the `payload_hash` of the
  approval that authorized it (`approval_payload_hash`), which links the two chains without mixing
  them.
- **Keys.** A proof is verified against **verifier-pinned keys**, identified by a `kid`, with
  rotation (several valid `kid`s, revocation), never against a key embedded in the record.
- **Declared values.** The backend declares only the exit and the duration; the proof names them
  `declared_*` and does not guarantee them. The guard dates `started_at` and `finished_at` itself,
  with the `SandboxClock`.
- **Who signs.** Never the backend, and not the guard itself with a key of the host. The guard
  builds the `ExecutionProof` and submits it to a `ProofSigner` port (a trait of the crate, async,
  injected into the guard; provisional name) that returns the signed record (signature, `kid`,
  `prev_hash`). The **signer**, not the guard, holds the order of the run's sandbox chain (a single
  writer per run) and **may re-verify before signing** (tenant and run, approval bound to the call,
  digests of the outputs). The deployment chooses the implementation of the port. The crate
  provides an in-process implementation for tests and self-hosting; it places the signing key in
  the same process as the host, and its documentation says so.
- **Fail closed.** Without a signed proof, the sandbox node fails with `Unavailable { retriable:
  true }` (code `unavailable`, frozen) and no unsigned result is handed back to the graph. The
  effect has already happened by then; the node fails, and the resume is served by the idempotent
  `collect`, which signs again (the signer being idempotent by key).
- **What the signature proves.** The consistency between the guard's report, the approval and the
  delivered bytes. It does not prove that the sandbox really produced these outputs. A reader must
  never present the proof as "executed in verified isolation", only as a "consistent proof".
- The guard **recomputes** the digests of the bytes it receives and stores. It therefore detects a gap
  between the digest the backend declared and the bytes it delivered, and any later alteration of
  what was stored. It does **not** detect a backend that lies about the content itself (bytes it
  never produced, a result that is false). Nor can it verify what the backend claims about its own
  isolation profile: the proof attests *what was declared and received*, not the truth of the
  execution nor the strength of the isolation.
- The replay journal references the proof; a proof holds no content and no secret value.

### 12. Runtime invariants

1. **Checkpoint after every node.** The node state holds only artifact references and the proof
   digest; the sandbox is disposable.
2. **One event per transition** (section 8), in the guard's durable record; the `RunEvent` stream
   does not carry the `sandbox_*` events before the next major version.
3. **Human gates.** The approval precedes `create` and covers the whole call (section 3). Without a valid grant the node returns
   `NodeOutput::interrupt` and the run emits `run_suspended`. On resume the node re-runs, the keys
   are identical, and the call-bound approval unblocks. v1 has one `exec` per node, so a single
   approval to resume.
4. **Determinism, resume and replay.** Two cases are distinct. A **live resume** (the node re-runs
   after a gate or a crash) goes through the guard to the backend with the same keys, and the
   backend's idempotence prevents a second effect. A **replay as evidence** (ADR 0038/0041) reads
   the journal alone and never calls the port.
5. **Cancellation.** Observed at boundaries; `destroy` in every case.

Implementation plan (small batches, one concern per pull request): (1) types, errors, idempotency
key, paths; (2) trait, guard, policy; (3) simulated backend; (4) conformance suite; (5) proof and
attestation; (6) events.

### 13. Normative shapes and backend obligations

The exact shapes are those of the rustdoc of the `ailu-sandbox` crate, which prevails over the
sketches above: `ExecOutcome`, `DeclaredExit`, `Truncation`, `CreateRequest`, `InputRef`,
`EnvGrant`, `CollectRequest`, `RawCollected`, `SandboxPolicy`, `SandboxJournal` and `SandboxEvent`,
`PolicyReason` and `QuotaKind`. The pattern syntax of `collect` is: `*` matches any run of
characters except `/`, `**` any run including `/`, `?` one character other than `/`, and anything
else is literal.

After `destroy`, the backend:

- holds no execution resource, no file, no environment and no secret value of the sandbox;
- keeps only the idempotency records `(key → request digest, result)` for the retention;
- answers a replayed key with the stored result or error and never executes again;
- answers `destroy` again with success.

## Consequences

- The engine gains an execution boundary without gaining a process. Any isolation technology can
  sit behind `SandboxBackend`; none is named or favored here.
- Safety rests on the guard and on the suite, not on trust in a backend. A backend that fails an
  applicable case is not conformant.
- **Events.** `RunEvent` is not `#[non_exhaustive]`, and consumers match on it exhaustively, as do
  the typed event schemas of the SDKs. Adding a `RunEvent::Sandbox` variant therefore breaks them.
  The sandbox events are **not exposed on the run event stream** until `RunEvent` is marked
  `#[non_exhaustive]` at the next major version (route (a), retained); the variant then follows in
  that same version. Until then the guard keeps its transitions as its own durable record, and
  that record is what the suite's events group tests. The addition is never made in a minor
  version.
- No binding and no TypeScript or Python layer is part of this ADR. Any exposure to another language
  is a separate decision, and parity is proved on the same conformance suite.
- Idempotence beyond `destroy` puts a durable, tenant-isolated store on the backend's critical path;
  a backend without it is limited to tests.
- **Retry and `attempt`.** The engine's retry loop re-runs the node handler up to `max_attempts`
  without consulting `FailureCategory` (the category only feeds the `NodeFailed` events and the
  error edge), and the handler receives only the `GraphState`, not the attempt number. A sandbox node
  therefore does not know the engine's `attempt`. A counter kept in the state does **not** work: a
  failed attempt never applies its state update. Only a counter captured outside the state works,
  and it is lost on restart. In practice the engine's internal retries reuse the same key, so the
  retry returns the stored result or the stored error.
- **Integration requirement.** The node cannot know whether a failure is final, so it must not try
  to decide it. For a key whose outcome is final (`Unavailable { retriable: false }` or a final
  error) the guard returns the remembered error without calling the backend again. Classifying the
  failure as `Permanent` does not by itself stop the engine's retry.
- **One approval can cover several real executions.** The `call_key` has no `attempt`, so a single
  approval authorizes every retry of the node, and each new attempt that gets a new idempotency key
  runs for real. The minimal alternative is a single-use grant, or a grant with a `ttl`; neither is
  decided here.
- **A human gate has a bounded wait.** The age check on `run_started_at` (section 2) applies to
  `create` too, so a run waiting on a human approval for longer than the result retention (24 h by
  default, 7 days at most) is refused when it resumes.
- v1 is deliberately narrow (one `exec` per sandbox and per node). It trades flexibility for a
  single approval and a coherent resume.
- The proof attests declarations, not isolation strength. Reviewers must not read it as a
  guarantee about the backend.
- The signing key lives behind a port, so the deployment decides where it is held; the in-process
  implementation puts it next to the host and is meant for tests and self-hosting. A signer outage
  fails sandbox nodes closed.
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
- **Sign with a key of the host, held by the guard.** Rejected: it ties the key to the engine
  process and leaves chain ordering to every guard instance; a signer behind a port holds both.
- **Let the backend sign the proof.** Rejected: the guard must recompute digests of what it receives
  and a backend must not vouch for itself.
- **Clamp requests above the ceiling.** Rejected: silent clamping hides what actually ran; a refusal
  is explicit.
- **A real-process reference backend.** Rejected: ADR 0024 forbids it in the engine. An in-memory
  simulator proves the contract semantics; real isolation is proved by running the suite against a
  real backend.

## Non-goals

- Naming, choosing or recommending any isolation technology or orchestrator.
- Any binding, TypeScript or Python layer, or host-side integration.
- Multi-`exec` sandboxes and shared filesystems between executions.
- Proving the isolation strength of any backend.
- Changing the semantics of existing node events or of the approval attestation chain.

## Open points

- Sandbox events on the run event stream: route (a) is retained (mark `RunEvent`
  `#[non_exhaustive]` at the next major version, then add the variant there); the addition never
  happens in a minor version. What remains open is only the exact behavior of the SDKs facing an
  unknown event, which is not yet verified.
- The same `argv` run in two nodes is lifted by the call order of the ADR 0041 journal. What stays
  open is the explicit dependency: extending that journal to a sandbox node, with the digest of the
  `IdempotencyKey` as input hash, is to be written.
- The numeric defaults of sections 4 and 6 (limits, request bounds, input size) are proposals and
  have not been measured.
- Making the engine's retry loop respect the `Permanent` category would be a separate runtime
  change, outside this ADR.
- The default result retention (24 h) and its ceiling (7 days) are proposals.
- The trait signatures are a sketch, to be frozen in the first batch. Nothing here has been compiled
  or executed.
