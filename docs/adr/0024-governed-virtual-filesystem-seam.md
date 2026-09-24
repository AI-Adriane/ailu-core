# ADR 0024 — Governed virtual filesystem seam (deep-agent harness phase 2)

- Status: Accepted — implemented, phases 2a–2e (2a primitive, 2b engine+SDK wiring, 2c gate
  verb, 2d carrier + control-plane policy, 2e external durable HTTP backend). The `execute`
  shell seam remains out of scope (its own future ADR). Studio fs-policy panel UI is a follow-on.
- Date: 2026-06-22
- Deciders: Mathieu (owner)
- Implements: [ADR 0023](0023-governed-deep-agent-platform-landscape.md) **phase 2** (the "real build")
- Builds on: [ADR 0022](0022-deep-agent-harness-gap.md) (the virtual FS is the headline gap),
  [ADR 0008](0008-pii-redaction-and-anonymization.md) (per-namespace policy DSL precedent),
  [ADR 0013](0013-llm-council-governed-deliberation.md) (the "governed version of a known pattern" bet)
- Phase 1 shipped: PR #35 (writeTodos + task)

## Context

LangChain's `deepagents` offers an agent-operable virtual filesystem (`ls`/`read_file`/
`write_file`/`edit_file`/`glob`/`grep`/`execute`) for **context offloading** — work that exceeds
one context window lives on "disk", not in the prompt. ADR 0022 identified this as the single
biggest primitive Ailu lacks (`artifact-store` exists but is not an agent-operable fs). ADR 0023
scoped it as phase 2, "the real build", **security-relevant → its own ADR + sign-off**. This is
that ADR.

The bet (same as the council, ADR 0013): not a new trick — the **governed** version of a
known-good primitive. deepagents gives agents a filesystem; Ailu gives them one where every
write is **versioned, attributable, audited, and (on guarded paths) approval-gated**.

Grounding (from a 5-reader sweep of the actual code):
- `ArtifactStore` (trait in `crates/artifact-store/src/interfaces.rs`) is already **versioned,
  immutable, attributable**: `write(ArtifactWriteInput{run_id,node_id,name,media_type,content,
  metadata}) -> Artifact`, `read(id)`, `read_version(id,v)`, `list_by_run(run_id)`,
  `list_versions(id)`. `ArtifactId = "{run_id}:{name}"`, `version` is a 1-based monotonic counter.
- Approval-gated tools already suspend/resume cleanly: `requires_approval` (react.rs) →
  `ApprovalRequestItem` → `AGENT_APPROVAL_INTERRUPT` (node.rs) → `run_suspended` + checkpoint →
  resume with `__approvedTools` → `validate_approved_tools` re-checks **no-self-approval** (bridge).
  `Ed25519Attestor` produces a chained, tamper-evident audit.
- The per-namespace policy DSL (PII/router, ADR 0008) is the precedent for a per-path permission
  DSL: an owner-only table of rules, resolved server-side, baked into a DTO with computed `canWrite`.
- The env-gated external-service seam (`redactor.rs`/`compressor.rs`: trait + `Noop` + `Http::from_env`)
  is the precedent for a pluggable backend.
- The pure agent-callable tool pattern (`todos.rs`, shipped phase 1) is the precedent for the fs tools.

## Decision

Add a **governed virtual filesystem** as (1) a `FilesystemBackend` seam, default-backed by the
existing `ArtifactStore`, (2) six agent-callable fs tools in `agents-core`, (3) a per-path
permission DSL (`deny|read|write|gate`), and (4) gate routing that **reuses the existing
approval-gated-tool path verbatim**. The shell/`execute` primitive is explicitly **out of scope**
— it is a separate external, always-gated seam in a future ADR (the hard rule: no eval/exec of
strings in-engine).

### 1. The seam + tools

A new crate `crates/fs-backend` (+ `packages/fs-backend` mirror) holds:

```rust
#[async_trait]
pub trait FilesystemBackend: Send + Sync {
    async fn read(&self, path: &str, version: Option<ArtifactVersion>) -> Result<FileContent, FsError>;
    async fn write(&self, path: &str, content: FileContent, ctx: &FsWriteCtx) -> Result<ArtifactRef, FsError>;
    async fn edit(&self, path: &str, patches: Vec<EditOp>, ctx: &FsWriteCtx) -> Result<ArtifactRef, FsError>;
    async fn ls(&self, prefix: &str) -> Result<Vec<FileEntry>, FsError>;
    async fn glob(&self, pattern: &str) -> Result<Vec<String>, FsError>;
    async fn grep(&self, pattern: &str, paths: Vec<String>) -> Result<Vec<GrepMatch>, FsError>;
}
```

Backends: **`ArtifactFsBackend`** (DEFAULT, wraps `Arc<dyn ArtifactStore>` — a real working
in-memory governed fs for the OSS engine, not a stub); **`HttpFilesystemBackend::from_env`**
(`AILU_FS_BACKEND_URL`, fail-**closed** unlike the redactor — a missing file is a semantic error);
**`NoopFilesystemBackend`** (`FsError::NotSupported`). Six tools in a new `agents-core/src/fs_tools.rs`
(`(ToolDefinition, ToolHandler)` pairs, the `todos.rs` pattern), capturing
`Arc<dyn FilesystemBackend>` + `Arc<dyn PathPolicy>` + run/node identity:
`read_file`/`ls`/`glob`/`grep` (never gated) + `write_file`/`edit_file` (gating per policy).

### 2. artifact-store mapping (no store change)

Path **is** the artifact `name`; `ArtifactId = "{run_id}:{path}"` (flat per-run namespace).
`write`/`edit` → `ArtifactStore::write` (new version; `edit` = read-latest → apply line patches →
write). `read` → `read`/`read_version`. **Directory gap**: artifact names are flat opaque strings —
no `mkdir`. `ls`/`glob` synthesize directory views over the flat keyspace (a name with a further `/`
yields a synthetic `FileEntry{isDir:true}`); empty dirs are not representable (documented limit).
**`ls`/`glob` have no prefix index** (only `list_by_run`) → the backend post-filters; O(n)-per-run,
acceptable at phase-2 scale, hidden behind the seam so a future `PgArtifactStore::list_by_prefix`
needs no surface change. **delete/rename**: not exposed in phase 2 (immutable append-only store);
if needed, tombstone (`metadata.deleted=true` new version) keeps the audit trail — deferred.

### 3. Per-path permission DSL (`deny|read|write|gate`)

Modeled on the ADR 0008 namespace-policy shape, keyed by **path glob**:
- `deny` = invisible (`ls`/`glob` filter it; `read` → `NotFound`, **never** `PermissionDenied`, so
  deny does not leak existence). `read` = read/ls/glob/grep. `write` = read + ungated write/edit.
  `gate` = read + write/edit **through an approval gate** (the keystone).
- Resolution = **most-specific-glob-wins, most-restrictive-on-tie** (fail-closed). Unmatched →
  `read` (visible, not silently writable). Policy-less run default: `** => read` + `scratch/** => write`.
- Engine side: a pure `PathPolicy` trait + `StaticPathPolicy` in `fs-backend` (no DB). The control
  plane compiles its table into `EngineSpec.fs_policy: Vec<{glob,verb}>` (additive). DB/RBAC stay in
  the control plane: a tenant-scoped `fs_path_policy` table, **owner-only** `PUT` (the verified
  `@Roles('owner')` resolved server-side), soft-delete via `revokedAt`.

### 4. Gate routing reuses the existing approval path

A `gate`-verb write rides the existing `requires_approval → __approvedTools →
validate_approved_tools → resume` machinery — **no new gate mechanism, no new interrupt kind**.
Mechanism: `gate` paths route to `write_file_guarded`/`edit_file_guarded`
(`requires_approval=true`); the loop suspends (`AGENT_APPROVAL_INTERRUPT` → `run_suspended` +
checkpoint); a **different principal** approves (`ensure_can_resolve` enforces no-self-approval);
resume re-checks at the bridge (defense-in-depth) and executes. Three refinements (all additive,
in the fs handler — not the runtime):
1. **Structured subject** — `subject = {description:"fs:write", path, contentHash, mediaType}`
   (the approval subject is already free-form), so the reviewer sees the exact path + a diff.
2. **Path-pinned grant** — a reserved `__approvedFsWrites: [{tool,path,contentHash}]` channel
   (sibling of `__approvedTools`) scopes the grant to the approved path+content; a different
   path/content under the same tool re-files an approval. (Closes "a tool grant unlocks all paths".)
3. **Signed audit** — every resolved fs approval chains an `Ed25519` `AttestationRecord` (existing).

### 5. Security boundary

- **`execute` (shell/code) is NEVER in-engine.** Not in this trait. A future `CommandExecutor` seam
  is reachable only over `AILU_SHELL_EXECUTOR_URL` (external sandboxed service), **always**
  `requires_approval=true`. Own ADR + sign-off. No `std::process::Command`/`child_process` ships.
- **Path traversal** — normalize + validate before a path becomes a name, fail-closed: reject `..`,
  absolute paths, backslashes, null bytes; canonicalize to forward-slash → `FsError::InvalidPath`.
  The `run_id` prefix is store-controlled (never agent-supplied), so a bypass cannot cross runs
  (defense in depth). Policy resolution runs on the normalized path.
- **Tenant isolation** — reuse the KB `ensureNamespaceAccess` claim pattern; cross-tenant read → 404
  (no existence disclosure). Engine `ArtifactStore` is run-scoped; tenant scoping at the control-plane
  boundary where run→tenant is known.

### 6. Runtime integration (invariants preserved)

No new runtime path — fs ops live inside tool/agent node handlers, so the runtime contract is
inherited. The handler **awaits `backend.write()` to completion before returning** `NodeOutput::update`
(the artifact is durable via its own versioned write; the channel carries the `ArtifactRef`), then
the runtime emits `NodeCompleted` + `persist_checkpoint` (documented order). One observability `Span`
per op (`fs.read`/`fs.write` … with `{action,path,principal,runId,nodeId,version,contentHash}`);
gated writes also chain an attestation. Audit must stay off the hot path (the bus is inline).

### 7. Parity (Rust authoritative + TS mirror)

`fs-backend` crate (authoritative) + `packages/fs-backend` (1:1 types) + `contracts/fs.ts` (HTTP
wire schemas, mirroring `pii.ts`) + `contracts/fs-policy.ts` (the DSL DTOs) + bindings
`build_fs_backend`/`build_path_policy` wired where `wrap_with_redactor`/`wrap_with_compressor`
compose. Parity tests on identical fixtures (path→key, version increment, ls/glob+synthetic-dir,
edit line-patch, traversal rejection, deny-invisibility, gate→suspend→approve(distinct principal)→
resume→write, no-self-approval at all three layers, attestation chain).

## Sub-phasing (each ships + is reviewed independently; the gate gets the closest review)

- **2a** — `fs-backend` crate + `ArtifactFsBackend` + the 6 tools + path normalization + `Noop`.
  Read/write/deny only (no gate, no DSL). Pure-engine, additive, DB-free, fully testable. *A working
  in-memory governed fs for OSS out of the box.*
- **2b** — the `PathPolicy` DSL: `EngineSpec.fs_policy`, deny-invisibility + most-specific resolution,
  default rules. Engine-only, fail-closed.
- **2c** — **the gate verb** (security-relevant core): guarded tool variants, structured subject,
  `__approvedFsWrites` path-pinned grant, attestation, suspend/resume parity under fan-out. *Closest review.*
- **2d** — control-plane surface: `fs_path_policy` table, owner-only policy endpoints, resolved DTO,
  tenant isolation, the approval-review UI hook (path + diff). No engine change.
- **2e** — `HttpFilesystemBackend::from_env` (external/sovereign backend), fail-closed. Optional/last.
- **Deferred (own ADR)** — the `execute` external gated seam.

## Open decisions (need Mathieu's call before code)

1. **Gate granularity** — per-write content review (path-pinned `__approvedFsWrites`, one suspend per
   distinct path+content) vs one approval per path-glob per run. UX: approve-per-file vs per-session.
2. **delete/rename** — ship none in phase 2 (recommended), or tombstone-delete + copy-rename now?
   (Will agents be blocked without `rm`/`mv`?)
3. **Who approves a gated write** — owner-only, or owner+approver (router precedent)? And who may set
   fs policy — strictly owner (PII precedent), or approver too?
4. **Two-tool gate routing** (`write_file` + `write_file_guarded`, recommended) vs one dynamic tool
   (needs a runtime change to evaluate policy mid-loop). LLM ergonomics of two names + server re-check.
5. **Default policy** — ship `** => read` + `scratch/** => write` (ergonomic) vs fail-closed read-only
   everywhere (agents must be granted a scratch area).
6. **Max file size / binary handling** — content is in-memory `serde_json::Value`, no streaming/limit.
   Set a max + reject (or route large files to the HTTP backend only)?

## Risks

- **Over-grant (highest severity)** — a plain tool grant unlocks the tool for the whole run; the
  `__approvedFsWrites` path-pinning is the mitigation but is NEW code in the fs handler → closest tests.
- **Gate evasion** — a model could call ungated `write_file` on a gate path; the server-side re-check
  fails it (`PermissionDenied`) but noisily mid-loop rather than a smooth suspend → prompt/manifest must
  steer to the guarded tool reliably.
- **Unbounded storage** — append-only versioning, no GC/delete; long edit sessions bloat the (in-memory)
  store → retention/compaction story needed before heavy use.
- **Perf cliff** — `ls`/`glob`/`grep` are O(n-per-run) until a `PgArtifactStore` adds a prefix index.
- **Traversal** — path normalization is the sole name-layer barrier; needs adversarial fuzzing (unicode,
  mixed separators, overlong encodings). Blast radius limited to within-run by the store-controlled prefix.
- **Audit on the hot path** — Ed25519 attestation must stay off the synchronous bus (queue, or attest
  control-plane-side after resume).
- **Scope creep** — 2a–2e sub-phasing MUST be enforced so the gate (2c) gets isolated security review.

## Consequences

- The headline deep-agent capability lands **governed by construction**: versioned + attributable
  (free from artifact-store), permissioned (per-path DSL), gated (the existing approval path), audited
  (existing attestation). It unlocks more than deep agents — any agent gets governed, versioned scratch
  storage.
- The OSS engine gets a **working** in-memory fs (2a) with zero external dependency; the sovereign/scale
  path (HTTP backend, Pg prefix index) is additive behind the seam.
- No runtime change, no artifact-store schema change, no new gate mechanism — the design is deliberately
  composed from existing, tested primitives (the council bet again).
