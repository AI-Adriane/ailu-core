# ADR 0054 — A sandbox runs the command behind a port

- Status: **Proposed** — the owner validated the contract this ADR is based on; later refinements
  are confirmed at merge. Batches 1 to 3 (types and keys; trait, guard and policy; simulated
  backend) are in review as pull requests #377, #378 and #379.
- Date: 2026-10-28
- Deciders: the owner
- Replaces (partially): [0024](./0024-governed-virtual-filesystem-seam.md) §5 (the `execute` seam).
  The future `CommandExecutor` reached by a URL is replaced by a trait. The other guarantees of §5
  stay: no process in the engine, and every execution is approval-gated.
- Relates to: [0032](./0032-secrets-redaction-and-no-log.md) (redaction),
  [0038](./0038-replay-as-evidence.md) /
  [0041](./0041-host-tools-on-catalog-runs-replayed-and-attested.md) (the replay journal: a host
  tool is never re-run on replay), [0044](./0044-cooperative-run-cancellation.md) (cooperative
  cancellation), [0051](./0051-the-signer-sees-and-signs-the-call.md) (the signature covers the
  exact call)

## Context

ADR 0024 §5 kept shell execution out of the engine and foresaw a `CommandExecutor` reached through
a configured URL. A URL is a weak boundary: no types, no lifecycle, no idempotence, no limits, and
nothing a second implementation could be tested against.

Agents need to run commands, which is the most sensitive action the engine can offer. The engine
must keep its guarantees around it: deterministic by default, every run resumable, a checkpoint
after every node, an event for every transition, human gates that suspend cleanly, a node that is
re-run on resume, a host tool that is never re-run on replay, and tenants kept apart.

## Decision

Add a crate `ailu-sandbox` that defines the execution boundary as a **trait**, behind a **guard**
that lives in the engine. The engine never spawns a process: it has no `std::process::Command`.
The port knows no orchestrator, provider or isolation technology; it speaks of an opaque
*isolation profile* that the deployment resolves.

Principles:

1. Every execution is **sensitive**. It passes an approval gate bound to the exact call (ADR 0051).
   An agent never approves its own output or its own execution.
2. **Fail closed.** When in doubt (policy, tenant, path, state), the answer is a typed refusal.
3. A **conformance suite** proves the contract against any backend.

### 1. Shape: one crate, one trait, one guard

- Dependencies: `ailu-graph-core`, `ailu-fs-backend` (for `normalize_path`, the rule of the virtual
  filesystem), `serde`, `serde_json`, `thiserror`, `sha2`, `zeroize`, and `async-trait` for the
  traits. `ailu-approval-engine` is a dev-dependency only, and `ailu-artifact-store` is not a
  dependency: the guard reaches approvals through the `GrantVerifier` port and records transitions through the `SandboxJournal` port. Until the
  run's artifact store is wired in, `Collected` carries the redacted bytes in memory. No runtime
  dependency: no `tokio` outside dev, no HTTP client.
- `trait SandboxBackend` (async, `Send + Sync`) has four operations: `create`, `exec`, `collect`,
  `destroy`. The backend executes; it does **not** decide policy.
- `GovernedSandbox<B: SandboxBackend>` is the **guard**, in the engine. It validates requests,
  applies the deployment policy, requires the approval, normalizes paths, recomputes digests, writes
  events, builds the proof and has it signed (section 11).
- **No forged handles or keys.** Only the crate can build a `SandboxHandle` or an
  `IdempotencyKey` (`pub(crate)` constructors). A backend returns a raw `RawCreated { sandbox_id,
  isolation_domain }`, and the guard wraps it into a handle carrying the tenant of the context. As
  long as the guard owns the backend (a private field), no call reaches the backend around the
  guard. A key or handle of another tenant yields `NotFound`. `TenantId` is a plain newtype
  (`TenantId(pub String)`): what is closed is the construction of keys and handles, not the type.
- Ports and signatures (the crate's rustdoc prevails):
  - `GrantVerifier::verify(&self, ctx, claim) -> Result<VerifiedGrant, GrantError>`. `VerifiedGrant`
    has `approval_id`, `call_key`, `tenant_id`, `run_id`, `requested_by`, `approved_by` and
    `approval_payload_hash`. A grant whose `approval_id` differs from the claim's is treated as
    unknown. `GrantError::Unknown` gives `ApprovalRequired`; `GrantError::Unavailable` gives
    `Unavailable { retriable: true }`.
  - `SandboxJournal::append(SandboxEvent) -> Result<Appended, JournalError>`, with `Appended::New`
    or `Duplicate`; a `JournalError` after an effect gives `Unavailable { retriable: true }`.
  - `SandboxClock::now_ms() -> u64`, in milliseconds since the Unix epoch. It is distinct from the
    `Clock` trait of `graph-runtime`; the engine does not read the clock here.
  - `SecretResolver::resolve(&self, ctx, reference) -> Result<ResolvedSecret, SecretError>` with
    `SecretError::{NotFound, Unavailable}`, used by backends only, never by the guard.
  - `ProofSigner`: its interface is specified in batch 5 (section 11).
- Features `sim` (section 9) and `conformance` (section 10), both off by default.

### 2. Identity, context, idempotence

`SandboxContext { tenant_id, run_id, node_id, attempt }` is built by the **host**. That it is never
the agent's is a host convention, not a guarantee of the type. The `attempt` is chosen by the host
or the node; the engine does not give it to a node. A naive node passes a constant (for example 1),
with the consequences described in Consequences.

An `IdempotencyKey` is `(tenant, run, node, attempt)` plus a `step` (`create` 0, `exec` 1,
`collect` 2, `destroy` 3), passed the same way to every operation. Its canonical form prefixes each
segment with its length and takes a sha256 hex digest, so `("a|b", "c")` and `("a", "b|c")` differ.
`step` is stable only if the node handler is deterministic; otherwise idempotence breaks, which the
guard detects as `IdempotencyConflict`.

- **Same key.** Same request digest returns the result already obtained without running again;
  a different digest is `IdempotencyConflict`. The guard gives `IdempotencyConflict` for the same
  context with a different request, and `NotFound` for another tenant.
- **Replayed `create`.** The guard does not join a `create` in progress: it calls the backend again
  (idempotent) and returns the same handle. A concurrent `create` of the same key may reserve a
  second quota slot for a moment, giving a spurious `QuotaExceeded`. This is a known limit.
- **Concurrent `exec`.** The second call gets `Busy` and only one backend call is made; retried, it
  receives the same result. After a non-final error the state stays `Executing` (outcome unknown)
  and only the same `exec` or `destroy` is allowed.
- **`destroy`** is idempotent (an absent or destroyed sandbox of the tenant succeeds).
- **Final errors.** Any error other than `Busy`, `Unavailable { retriable: true }` and `Cancelled`
  is final and remembered (`is_replayable`); the guard answers the same call with it again without
  calling the backend. `Unavailable { retriable: false }` is never retried.
- **Persistence beyond `destroy`.** The backend keeps `(key → request digest, result)` in durable
  storage isolated per tenant. It does not keep the proof, which it does not produce. A backend
  without durable storage declares the capability absent and serves tests only. Records stay at
  least the guard's `result_retention`. A policy can ask for up to `MAX_RESULT_RETENTION_MS`
  (7 days), so a backend keeps at least that; the simulator can only lengthen its retention. The
  clocks of the host and the backend are assumed to agree.
- **Retention and run age.** The host gives every `create`, `exec` and `collect` a
  `run_started_at_ms`, the instant the run started. It is a datum of the trusted caller, not bound
  to the `call_key`, and never read from the engine's clock. The guard does not pass it to the
  backend. The guard refuses, before any effect, a run older than the result retention
  (`SandboxError::PolicyDenied(PolicyReason::AttemptTooOld)`, retention 24 h by default, 7 days at
  most) or started in the future beyond `max_clock_skew_ms` (5 min by default, `AttemptInFuture`).
  The reason: the **backend** cannot tell a key that never ran from a key whose result expired, so
  it could not refuse to re-execute; the guard, which knows the run's age, refuses instead. The
  reliability of `run_started_at_ms` is an **obligation of the host**: a node that passes a fresh
  instant bypasses the check, and the suite cannot test it. A run waiting on a human approval longer
  than the retention is refused when it resumes.
- **Serializable records.** `RawCollected` and `ExecOutcome` stay serializable with bounded bytes,
  because a backend keeps them for the retention. `RawCollected` never carries secrets once
  persisted: a backend that replays a `collect` resolves them again through its `SecretResolver`,
  otherwise the guard fails closed (fewer secrets than grants).
- **Resume and retry.** A resume after a gate re-runs the node with the same `attempt`, hence the
  same keys and no double effect. A retry gets a new key, hence a new sandbox, only if the node
  supplies a new `attempt` (see Consequences).
- **Replay as evidence** (ADR 0038/0041) is distinct from a live resume, which calls the backend and
  relies on its idempotence. The result of an `exec` is recorded in the run journal under the name
  `sandbox.exec`, with the digest of the `IdempotencyKey` as input hash. As in ADR 0041, the journal
  is served **in call order**, checking the input hash, not as a `(name, digest)` table. On replay
  the port is never called, and a mismatch fails loudly. Extending that journal to a sandbox node is
  an explicit dependency, to be written separately.

### 3. Lifecycle and guard API

**v1: one sandbox runs one `exec`, and a node runs one `exec` per attempt**, so a node needs one
approval and its resume is coherent. Several `exec` would need one approval over the whole plan
(an ordered list of `argv` in the approved digest), and a shared filesystem would be a backend
capability, never implicit. States: `Created → Executing → Executed → Collected → Destroyed`, plus
`Poisoned` (isolation violation), which accepts only `destroy`. `create` only **reserves** (quota,
isolation domain, `ttl`); a backend may build the execution resource at `exec`.

The guard returns `InvalidState` for an operation out of order: `collect` before `exec`, while
`Executing`, when `Poisoned`, or after `Destroyed` (terminal). A second `exec` or `collect` under
**another key** is not decided by the guard; it is `InvalidState { from: Collected }` produced by
the **backend** (an obligation, section 13, which the simulator implements). The same `collect`
under the same key returns the recorded result.

The table is schematic; the rustdoc has the exact signatures.

| Guard operation                                                              | Output                                  |
| ---------------------------------------------------------------------------- | --------------------------------------- |
| `create(ctx, req, planned, claim: Option<&ApprovalClaim>, run_started_at_ms, cancel)` | `SandboxHandle`                |
| `exec(ctx, handle, req, claim, run_started_at_ms, cancel)`                   | `ExecOutcome`                           |
| `collect(ctx, handle, req, run_started_at_ms, cancel)`                       | `Collected` (+ signed proof)            |
| `destroy(ctx, handle)`                                                       | `()`                                    |

`ExecRequest { argv, cwd: Option<LogicalPath>, stdin: Option<Vec<u8>> }`: `argv` is non-empty,
`argv[0]` is the program, and it is **never a string interpreted by a shell** (a shell is a program
like any other, with the same approval). `cwd` is relative to the working root; `None` is the root.

**Approval, claim and call key.**

- The approval precedes `create` and covers the **whole call**: `create` plus the planned `exec`.
  `exec` re-verifies against the request actually received; changing one argument invalidates the
  grant. The approval is checked when an effect is about to happen: a replayed `create`, `exec` or
  final error is served without a new check.
- The caller brings only an `ApprovalClaim { approval_id, call_key }`. The `GrantVerifier` returns
  the verified grant. The expected host implementation is the approval engine or its attestation.
  A grant whose approver is the requester is refused (`SelfApproval`). The verifier must return
  tenant, run and principals in one canonical form (the guard compares principals trimmed and
  ignoring ASCII case).
- `approval_payload_hash` is 64 lowercase hexadecimal characters and never empty; it is carried into
  the proof and recorded in the guard's journal. `collect` takes no claim, so the proof batch must
  remember the grant and its hash at `exec` time.
- The `call_key` has the name form of ADR 0051, `sandbox.exec#<sha256>`, but the hash covers
  length-prefixed digests (recipe in section 13), not the `callInput` JSON of ADR 0051. A verifier
  recomputes it from the request, not from the masked subject. It does **not** depend on `attempt`.

**A sandbox lives at most one node attempt.** The guard offers
`with_sandbox(ctx, create, planned, claim, run_started_at_ms, cancel, f)`: it creates the sandbox,
runs `f`, and destroys on exit (success, failure, cancellation, suspension), returning a
destruction error if `f` succeeded. If the future is dropped, nothing can be awaited: the
backend's `ttl` sweep removes the sandbox and the guard returns the tenant's slot at the same
instant. `ttl` is mandatory and capped by the deployment. The sweep is an **obligation of the
backend**; a resource left after its `ttl` fails conformance.

The guard's registry is **bounded** (record counts, eviction after retention plus `ttl`, collected
bytes dropped at `destroy`). A quota slot is returned once, even if the future is abandoned, and a
slot's `ttl` expires through the injected clock. The registry lives in memory: after a restart it is
rebuilt by replaying nodes, and the table of isolation domains is empty. A sandbox is **not** in the
checkpoint and none is held while a node is suspended.

### 4. Limits and request bounds

`Limits` has `cpu_millicores`, `memory_bytes`, `wall_time_ms`, `output_bytes` (per stream),
`max_processes` and `ephemeral_disk_bytes`. `RequestBounds` caps the request itself:
`max_argv_count`, `max_argv_bytes`, `max_stdin_bytes`, `max_exec_per_sandbox` (v1: 1),
`max_collect_patterns`, `max_collect_files`, `max_collect_bytes`, `max_ttl_ms`, `max_input_bytes`,
`max_input_files`.

- `Limits::default()` is **low** (proposed: 250 millicores, 256 MiB, 30 s, 1 MiB per stream, 32
  processes, 64 MiB of disk). The **ceiling** comes from the deployment policy, never the agent. A
  request above it is refused (`PolicyReason::LimitAboveCeiling`), not clamped.
- Proposed bounds: 64 arguments, 32 KiB of `argv`, 64 KiB of stdin, 8 collect patterns, 256 files,
  16 MiB collected, 15 min `ttl`. A request outside them is refused by the guard
  (`RequestTooLarge`) before the backend. `max_argv_count` defaults to 64, but the effective
  maximum an approver can read is 32 (`UnreviewableCall`, section 6).
- A limit exceeded while running: the backend kills and returns `LimitExceeded { kind, partial }`
  with kind `WallTime`, `Memory`, `Processes` or `Disk` and the partial `ExecOutcome`. Output past
  its bound is **truncated** by the backend (`Ok` with a typed `Truncation`); the guard refuses a
  delivery that is out of bounds.
- Other refusals before the backend: a zero `ttl`; `Allow` with an empty list; duplicated or nested
  input destinations; network hosts that are not lowercase ASCII DNS names, and IP literals in a
  request (a future extension could allow them by a separate deployment rule). Refusal reasons never
  echo the content supplied by the caller.

### 5. Network

- `NetworkPolicy::Deny` is the default. `Allow(Vec<HostRule { host, port }>)` lists exact names: no
  IP wildcard, no range. Each requested rule must be in the deployment list; otherwise it is refused
  (`NetworkRule`), never trimmed. The list is part of the approved digest.
- Backend obligations, checked by the suite: a host or redirect target off the list is refused; DNS
  is refused in `Deny` mode; a resolution to a private, loopback, link-local, unique-local or
  metadata address (IPv4 and IPv6) is refused; **DNS rebinding** is refused (resolve once, pin the
  address, refuse a divergent second resolution). IPv6 is judged by an **allow list**: only global
  unicast `2000::/3`, minus the special blocks inside it (`2001::/23`, `2001:db8::/32`, `2002::/16`,
  `3fff::/20`), is public. IPv4-mapped and translated addresses are refused even when the IPv4
  address they carry is public. Redirects are limited in number (five in the simulator).
- Accepted risk: an allowed host that acts as a **proxy or relay** (open redirect, tunnel) bypasses
  the list, and the contract cannot know it. The mitigation is that the list is approved by name
  with the call. No conformance case has a pass or fail criterion for it.

### 6. Files, secrets and what the approver sees

Files:

- Inputs are artifact references of the current run. Destinations use the same normalization as the
  virtual filesystem of ADR 0024 (reject `..`, absolute paths, backslashes, NUL), giving
  `InvalidPath`. Refusing an outgoing symlink is a **backend** obligation; the simulator refuses it
  at `collect` with `InvalidPath`. Inputs are materialized by the backend.
- Input count is bounded in the guard (`BoundKind::InputFiles`). The total bytes and the ownership
  of references (another run or tenant is `NotFound`) are checked once the guard has the artifact
  store, in a later batch. The guard converts a backend's `QuotaExceeded` or `NotFound` into a
  generic error, so ownership checks are the guard's.
- `CreateRequest` has **no host volume mount variant**: the prohibition is in the type.
- Outputs are retrieved by patterns (`*` any run except `/`, `**` any run including `/`, `?` one
  character other than `/`), bounded in count and size, and returned with their `sha256`.

Secrets:

- The initial environment is **empty** except a fixed set declared by the profile (for example
  `LANG`, a minimal `PATH`), recorded in the proof. Nothing is inherited from the host.
- A secret enters only through `EnvGrant { name, secret_ref }` declared in `CreateRequest` and
  covered by the approval. The backend resolves it through the `SecretResolver` (the moment is the
  backend's; the simulator resolves at `exec`). `NotFound` covers an unknown reference and another
  tenant's alike. The guard turns a backend's `NotFound` into a generic backend error that is
  **final and remembered**: the key is not replayable even if the secret is added later. The value
  never crosses the request, proof, events or journal. Secrets must not go through `argv` or
  `stdin`; only `EnvGrant` is provided.
- The guard does not hold the values the backend resolved, so the backend **hands them back** in
  `RawCollected`, in a `ResolvedSecrets` type that has no `Serialize`, `Debug` or `Display` and is
  zeroized after use. The guard uses them only to redact. Redaction by known value covers only
  values the program has not transformed, and depends on the good faith of the backend: the guard
  can compare counts with the grants, not check a value.

What the approver sees:

- The subject is a structured object: `argv` (redacted by the host's known values), `cwd`, network
  hosts and ports, the **names** of environment variables and the secret references (never values),
  input destinations, limits and `ttl`; for `stdin`, only its size.
- **An unreviewable call is refused, never shown cut.** More than `MAX_REVIEW_ARGS` (32) arguments,
  or any argument, `cwd`, profile, host, destination, environment name or secret reference longer
  than `MAX_REVIEW_CHARS` (256 characters), is refused before any approval request with
  `PolicyReason::UnreviewableCall`. Control characters, escape sequences and bidirectional or
  invisible characters in those fields are refused with `UnsafeText` (checked before the length).
- `ApprovalRequired` carries a **minimal** `ApprovalSubject`: description `sandbox.exec`, the
  `call_key`, tenant, run, node and counters (arguments, stdin bytes, environment grants, inputs,
  network rules), with a masked `Debug`. The full subject is built on the host side by
  `ApprovalRequest::for_call_redacting`, a helper given to the host, which redacts with its own
  values; the guard does not, since it does not hold them.

### 7. Tenant isolation and violations

- Two tenants never share a sandbox or an isolation domain. This is an **obligation of the
  backend**, as is never reusing a domain across tenants, even after `destroy`. The guard records
  the tenant that declared each domain in its bounded in-memory table and fails with
  `IsolationViolation` if a domain is declared for another tenant; after a guard restart the table
  is empty. The proof records the domain **as declared**.
- Per-tenant admission: ceilings on concurrent sandboxes and creation rate. Beyond them,
  `QuotaExceeded` is returned **immediately**, never a silent wait, and other tenants stay served.
- A violation detected by the backend gives `IsolationViolation { signal }`: the sandbox is
  `Poisoned`, the proof is marked `violation`, and `destroy` is required. The contract does not
  claim a backend detects everything; it fixes what happens when something is detected.

### 8. Errors and events

`SandboxError` has the variants `PolicyDenied(PolicyReason)`, `ApprovalRequired { subject }`,
`QuotaExceeded { kind }`, `LimitExceeded { kind, partial }`, `IsolationViolation { signal }`,
`IdempotencyConflict`, `InvalidPath { reason }`, `NotFound`, `InvalidState { from }`, `Cancelled`,
`Busy { retry_after_ms }`, `Unavailable { retriable }` and `Backend(String)`. A refusal is written
`SandboxError::PolicyDenied(PolicyReason::X)`.

- **Stable code.** `code()` returns frozen snake_case values (`policy_denied`, `approval_required`,
  `quota_exceeded`, `limit_exceeded`, `isolation_violation`, `idempotency_conflict`, `invalid_path`,
  `not_found`, `invalid_state`, `cancelled`, `busy`, `unavailable`, `backend_error`); subtypes
  carry a code (`limit_exceeded.wall_time`, `policy_denied.unreviewable_call`). Adding a code is
  compatible; changing or removing one breaks. No one parses a message; the suite checks the code.
  Error (final), wait (`Busy`) and unavailability (`Unavailable`) are distinct; the caller retries
  only the last two.
- **Backend text is never relayed**, only its code. Variants reserved to the guard (policy,
  approval, quota, idempotency, state, not found) that a backend returns become a generic backend
  error.
- **Redaction.** The guard redacts `stdout`, `stderr` and collected artifacts **by known values**
  (section 6), not by the patterns of ADR 0032, before storing or exposing them. The guard keeps no
  raw copy. The **backend**, however, keeps the raw record for the retention (section 2), and
  known-value redaction does not cover what it stores: encryption per tenant and isolation of that
  record are obligations of the backend.
- **Events.** `sandbox_created`, `sandbox_exec_started`, `sandbox_exec_completed`,
  `sandbox_exec_failed`, `sandbox_collected`, `sandbox_destroyed` and `sandbox_denied`, written to
  the guard's own journal, not to `RunEvent` before the next major version. They carry digests and
  identifiers, never content. The order guaranteed is the append order of a node's sequential
  calls; none relative to `node_started` and `node_completed`. `exec_started` is written **before**
  the backend call, the others after the effect. A journal failure after the effect gives
  `Unavailable { retriable: true }`, and the replay writes the event again and deduplicates it.
  Deduplication on `(key, kind, code)` is an obligation of the `SandboxJournal` port, which must be
  durable and isolated per tenant (`InMemoryJournal` is not durable). A failed write of
  `sandbox_denied` is ignored: the refusal stands and the event is lost. `NotFound` is counted
  (`not_found_count`) and `Busy` is neither counted nor journaled, so that guessing handles cannot
  fill the journal. Failures other than refusals (`Unavailable`, `Backend`, `Cancelled`) of `create`
  and `collect` produce no event. So "one event per transition" holds for lifecycle transitions and
  refusals, not for every error.
- **Backend trait.** `create` returns `RawCreated`; `exec` returns `ExecOutcome`; `collect` returns
  `RawCollected` (bounded bytes, declared digests, resolved secrets); `destroy` returns `()`.
  Each takes the key; `create`, `exec` and `collect` take a `CancelToken`, a new type wrapping the
  cooperative check of ADR 0044 (`CancelCheck`). `destroy` is not cancellable. A backend signs
  nothing and approves nothing.

### 9. The simulated backend (`sim`)

ADR 0024 forbids launching a process in the engine, so the reference backend is **entirely
in-memory and deterministic** and runs no real program:

- "programs" are registered Rust functions over a virtual filesystem and a virtual `SandboxClock`;
- injectable faults: infinite loop, unbounded allocation, huge output, process explosion, disk
  fill, outage, isolation violation, `SharedDomain`;
- a simulated table of exact host names, with redirects and private resolutions. It analyses no
  host string (no IP literal, no wildcard): the guard's request validation is the barrier. Addresses
  are classified as in section 5;
- an outgoing symlink among collected outputs is modelled and refused at `collect`;
- the module is under `disallowed_types` and `disallowed_methods` lints that forbid process, file,
  clock, environment, socket and thread use, so no real process or I/O can slip in.

It proves the **semantics of the contract**, not the isolation of a production backend, which the
same suite covers when run against that backend.

### 10. The conformance suite (`conformance`)

`run_conformance(target)` runs the cases against a `ConformanceTarget`: a backend factory plus
`probe(kind)`, which gives, for a real backend, the command that provokes each behavior; `None`
marks the case *not applicable*, never *passed*. A case passes when the attack **fails** in a typed
way. The target also exposes hooks: inspecting residues, injecting a host crash, reading the
idempotency marker; a backend without a hook marks the dependent cases not applicable.

Cases are in three classes:

- **Guard**: policy, approval, idempotence, registry, redaction, proof, events, the refusal of an
  unreviewable call or unsafe text, the minimal `ApprovalRequired` subject, the payload hash, the
  conversion of a backend's `QuotaExceeded` or `NotFound`. Out-of-order operations (`collect` before
  `exec`, `Executing`, `Poisoned`, `Destroyed`) are `InvalidState`; the same context with another
  request is `IdempotencyConflict`; another tenant is `NotFound`.
- **Simulator only**: a backend that lies, crash injection, forged digests, `SharedDomain`, a
  retention shorter than the guard's maximum.
- **Real backend through `probe`**: limits, network (section 5), residues and `ttl` sweeps, empty
  environment, outgoing links, isolation domains, and the backend's own `InvalidState { from:
  Collected }` for a second `exec` or `collect` under another key.

Groups: lifecycle; idempotence (20 parallel `exec` with one key give one backend execution and the
others `Busy`; a replayed `collect` adds no second chain record; an attempt too old or too far in
the future is refused before any effect); limits; network; secrets (fewer secrets than grants fails
closed; hex, base64 and URL forms are redacted); paths; multi-tenant (isolation domains, immediate
quota refusal, fairness); governance (a bare claim, a claim for another call, or `create` without
approval of the planned `exec` is refused; changing an argument invalidates the grant); proof
(signer unavailable fails closed with no result; the chain stays ordered under concurrent calls;
a delivered output that differs from its declared digest is refused; altering a stored output
invalidates the proof; a backend lying consistently about content is out of reach and is said so);
events (append order, no content, `exec_started` before the call, deduplication on replay);
keys and handles; request bounds.

The engine invariants of section 12 are not in the suite: the crate does not depend on
`graph-runtime` and provides no node. They are **host integration** tests. The simulated backend
must pass 100 % of the applicable cases; a production backend must pass every applicable case and
list the others in its report.

### 11. Execution proof and attestation chain

`ExecutionProof` is a canonical record of digests only: run, node, tenant, sandbox, idempotency key,
isolation domain, `request_digest`, `profile_digest`, `limits_effective`, `inputs_digest`,
`outputs_digest`, `stdout_digest`, `stderr_digest`, `truncated`, `exit`, `duration_ms`,
`started_at` / `finished_at`, `violation`, `approval` (call key and approver) and
`approval_payload_hash`. `request_digest` is a canonical digest (domain `sandbox.proof.request.v1`)
of the three fingerprints `create`, `exec` and `collect`; `profile_digest` is separate. `violation`
is always false: a sandbox in violation is poisoned and cannot be collected. In the canonical form,
non-integer numbers are refused and integers are capped at 2^53 − 1.

- **Two chains.** The approval attestation has a frozen shape and its verifier would reject a mixed
  chain. The proof uses the **same primitives** (JavaScript-style canonical JSON, Ed25519,
  `prev_hash`) on a **separate sandbox chain per run**, with a record type whose
  `kind: "sandbox_execution"` is signed. Each proof carries the `approval_payload_hash` that links
  to the approval chain.
- **Keys.** Verification uses verifier-pinned keys with a `kid` and rotation, never the key embedded
  in the record. `trust()` refuses small-order keys and a `kid` already pinned with another key;
  revocation is final.
- **Chain end.** A chain cut at its end still verifies; only `verify_chain_ending_at`, given the
  expected last hash, detects the truncation.
- **Declared values.** The backend declares only exit and duration (`declared_*`). The guard dates
  `started_at` and `finished_at` with the `SandboxClock`. On a replay, the dates of the `exec` come
  from the first signature, and `same_content_as` ignores them.
- **Who signs.** Not the backend, and not the guard with a host key. The guard builds the proof and
  submits it to a `ProofSigner` port (async, injected; provisional name) that returns the signed
  record (signature, `kid`, `prev_hash`). The signer holds the chain order (one writer per run), may
  re-verify before signing (tenant and run, approval bound to the call, output digests), and must be
  idempotent by `idempotency_key`. The deployment chooses its implementation; the crate provides an
  in-process one for tests and self-hosting, which puts the key in the host's process and holds at
  most 65 536 records.
- **Fail closed.** Without a signed proof the node fails with `Unavailable { retriable: true }`
  (code `unavailable`) and no unsigned result reaches the graph. The effect has already happened;
  the resume is served by the idempotent `collect`, which signs again. The failure is closed when the
  signer is absent, unavailable or refuses: a signer that signs badly is not caught by the guard, and only the verifier
  checks the signature. Refusing the proof is `PolicyReason::ProofRefused`
  (`policy_denied.proof_refused`). `collect` returns only `Collected`; `collect_proved` returns the
  proof as well.
- **What it proves.** The guard recomputes the digests of the bytes it receives, so it detects a gap
  between declared digest and delivered bytes and any later alteration. It does not detect a backend
  that lies about content, nor check the backend's isolation profile. The signature proves the
  consistency between the guard's report, the approval and the delivered bytes. A reader must
  present it as a "consistent proof", never as "executed in verified isolation".

### 12. Runtime invariants and plan

The host writes the node. Invariants 1 and 3 are **obligations of that node**:

1. **Checkpoint after every node**, holding only artifact references and the proof digest.
2. **Events** per section 8, in the guard's journal.
3. **Human gates.** The approval precedes `create`. Without a valid grant the node returns
   `NodeOutput::interrupt` and the run emits `run_suspended`. On resume the node re-runs with the
   same keys, and the approval bound to the call unblocks it.
4. **Resume and replay.** A live resume goes through the guard to the backend with the same keys;
   a replay as evidence reads the journal and never calls the port.
5. **Cancellation** is observed at boundaries, with `destroy` in every case.

Plan (small batches, one pull request each): (1) types, errors, keys, paths; (2) trait, guard,
policy, `SandboxJournal` and the seven events; (3) simulated backend; (4) conformance suite;
(5) proof and attestation; (6) exposing the events on the `RunEvent` stream, at the next major
version.

### 13. Normative shapes and backend obligations

The rustdoc of `ailu-sandbox` prevails over the sketches here, for `ExecOutcome`, `DeclaredExit`,
`Truncation`, `CreateRequest`, `InputRef`, `EnvGrant`, `CollectRequest`, `RawCollected`,
`SandboxPolicy`, `SandboxEvent`, `PolicyReason` and `QuotaKind`.

**Recipe of the `call_key`.** Each field is written as `<tag length>:<tag>=<value length>:<value>|`,
lengths in bytes as decimal ASCII, after a first field `domain`; the digest is lowercase hex sha256
and the key is `sandbox.exec#<hex>`. Numbers are decimal ASCII. Structured values are the JSON of
plain structs in declaration order, never maps. Shared test vectors are required.

- `call_key`: domain `sandbox.call.v1`, then `tenant`, `run`, `node`, `create`, `exec` (the two
  fingerprints).
- Create fingerprint (`sandbox.create.v1`): `profile`; `limits` and `network` as JSON; `ttlMs`;
  `inputs` (count) then each `input` as JSON; `env` (count) then for each `envName`, `envRef`.
- Exec fingerprint (`sandbox.exec.v1`): `argc`, then each `arg`; `cwd` (or the marker `\0none`);
  `stdin` written raw after its length prefix (or `\0none`).

The node files the `call_key` and the subject with the approval engine; an ADR 0051 filter must not
recompute this key.

**Backend obligations** (stated once here):

- The request digest covers the whole request, `stdin` included. While a key runs, the same key gets
  `Busy`. An execution marker with neither result nor resource gives
  `Unavailable { retriable: false }`, never a re-execution.
- Second `exec` or `collect` under another key: `InvalidState { from: Collected }` (or the matching
  state).
- A `sandbox_id` is unique and never reused. The guard rejects a known id with `IsolationViolation`
  (signal `sandbox_id_reused`); a replayed `create` returns the same id and domain.
- Declared digests in `RawCollected` are computed over the delivered bytes before redaction; paths
  match the patterns; duplicates are refused.
- Errors a backend may return: `Busy`, `Unavailable`, `Cancelled` (not on `destroy`);
  `LimitExceeded` on `exec`; `IsolationViolation` and `InvalidPath` where they apply; `Backend`
  anywhere. Any other error is final and remembered.
- After `destroy`, the backend holds no execution resource, environment or secret value, and no file
  outside the stored results; keeps only the idempotency records for the retention; answers a
  replayed key with the stored result or error and never executes again; answers `destroy` again
  with success.
- It stores the raw record encrypted per tenant (section 8), sweeps on `ttl`, and never shares an
  isolation domain across tenants.

**Port obligations.** A `SandboxJournal` deduplicates by `(key, kind, code)`, is durable and isolated
per tenant. A `GrantVerifier` returns the decision, honours its expiry, and returns canonical forms.

## Consequences

- The engine gains an execution boundary without gaining a process. Any isolation technology can sit
  behind `SandboxBackend`; none is named or favored.
- Safety rests on the guard and the suite, not on trust in a backend. The proof attests declarations
  and consistency, not isolation strength.
- **Events.** `RunEvent` is not `#[non_exhaustive]` and consumers match on it exhaustively, as do the
  typed event schemas of the SDKs, so a `RunEvent::Sandbox` variant would break them. Option chosen:
  mark `RunEvent` `#[non_exhaustive]` at the next major version and add the variant there. Until
  then the events stay in the guard's journal. The addition never happens in a minor version.
- No binding and no TypeScript or Python layer is part of this ADR; any exposure to another language
  is a separate decision, and parity is proved on the same suite.
- Idempotence beyond `destroy` puts a durable, tenant-isolated store on the backend's critical path.
  The backend also stores a raw record for the retention, which known-value redaction does not
  cover; its protection is a backend obligation.
- **Retry and `attempt`.** The engine's retry loop re-runs the handler up to `max_attempts` without
  consulting `FailureCategory` (it only feeds `NodeFailed` events and the error edge), and the
  handler receives only the `GraphState`, not the attempt. A sandbox node does not know the engine's
  attempt. A counter in the state does **not** work: a failed attempt never applies its state
  update. Only a counter captured outside the state works, and it is lost on restart. In practice
  the engine's internal retries reuse the same key, so the retry returns the stored result or error.
  A node cannot know whether a failure is final; for a final outcome the guard returns the remembered
  error without calling the backend. A `Permanent` failure category does not by itself stop the
  engine's retry.
- **One approval, several real executions.** The `call_key` has no `attempt`, so one approval
  authorizes every retry, and each attempt with a new key runs for real. The minimal alternative is
  a single-use grant or a grant with a `ttl`; neither is decided here.
- **A human gate has a bounded wait**, set by the result retention (24 h by default, 7 days at most).
- v1 is deliberately narrow (one `exec` per sandbox and node): one approval and a coherent resume.
- The signing key lives behind a port, so the deployment decides where it is held; the in-process
  implementation is for tests and self-hosting. A signer outage fails sandbox nodes closed.
- The two attestation chains add one verification surface, in exchange for leaving the approval
  chain and its verifiers untouched.

## Alternatives considered

- **Keep the URL seam of ADR 0024 §5.** Rejected: untyped, no lifecycle or idempotence contract, no
  way to test two implementations against one definition.
- **Run processes inside the engine.** Rejected: it breaks the rule that the engine spawns nothing
  and puts isolation in the runtime's trust domain.
- **Several `exec` per sandbox from v1.** Rejected for now: it needs an approval over a whole plan
  and a shared-filesystem capability; `step` leaves room.
- **One attestation chain for approvals and executions.** Rejected: the approval record has a frozen
  shape; a separate chain linked by `approval_payload_hash` keeps both verifiable.
- **Sign with a host key held by the guard.** Rejected: it ties the key to the engine process and
  leaves chain order to every guard instance; a signer behind a port holds both.
- **Let the backend sign the proof.** Rejected: a backend must not vouch for itself.
- **Clamp requests above the ceiling.** Rejected: silent clamping hides what ran; refusal is
  explicit.
- **A real-process reference backend.** Rejected: ADR 0024 forbids it in the engine. A simulator
  proves the contract; real isolation is proved by running the suite against a real backend.

## Non-goals

- Naming, choosing or recommending any isolation technology or orchestrator.
- Any binding, TypeScript or Python layer, or host-side integration.
- Multi-`exec` sandboxes and shared filesystems.
- Proving the isolation strength of any backend.
- Changing existing node events or the approval attestation chain.

## Open points

- The exact behavior of the SDKs facing an unknown run event is not verified.
- Extending the ADR 0041 journal to a sandbox node, with the digest of the `IdempotencyKey` as input
  hash, is to be written.
- Making the engine's retry loop respect the `Permanent` category would be a separate runtime
  change.
- The numeric defaults (limits, request bounds, input size, retention 24 h / 7 days) are proposals
  and have not been measured.
- The `ProofSigner` interface, the proof and its signing order, the conformance suite and the events
  on the run stream are not implemented yet; the crate's rustdoc prevails over this text.
