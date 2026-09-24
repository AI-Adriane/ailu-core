# ADR 0035 — Skills: SKILL.md progressive disclosure (deep-agent platform phase 12)

- Status: **Accepted** (signed off; shipped — `crates/skills` + `crates/agents-core/src/skill_middleware.rs` + bridge/SDK/contracts wiring + doc-site page & cookbook recipe, all tests + clippy + docusaurus build green).
- Deep-agent platform: [ADR 0023](0023-governed-deep-agent-platform-landscape.md) **phase 12** ("skills — SKILL.md progressive disclosure", previously "no ADR yet" — this is it).
- Builds on / mirrors: [ADR 0025](0025-unified-agent-middleware-api.md) (the `AgentMiddleware` seam + the sealed governed/efficiency split), [ADR 0026](0026-memory-architecture-engine-studio.md) (the **`MemoryMiddleware`** precedent — `before_run` seed-injection, bridge-sealed namespace+principal, fail-open, seed-only determinism — verified in `crates/agents-core/src/memory.rs`), [ADR 0024](0024-governed-virtual-filesystem-seam.md) (the **content-scoped approval grant** `name#sha256(input)` pattern), the `KnowledgeStore` vector seam (`crates/knowledge/src/lib.rs`).

## Context

A **skill** is procedural know-how an agent loads **progressively**: a SKILL.md = YAML frontmatter (`name` + `description` — a cheap, always-resident index) plus a markdown **body** loaded **on demand** only when the task matches. It is distinct from **tools** (callable functions, gated by `before_tool`) and from **RAG** (retrieved facts): a skill is *guidance text*, never executed. Phase 12 has had no ADR; nothing is built. The closest code precedent is `MemoryMiddleware` (ADR 0026): it embeds the seed, vector-recalls from a sealed namespace, and prepends hits to the seed conversation without mutating runtime state — exactly the shape a skill loader needs.

**Primary use case — skills for deep agents (à la LangGraph `deepagents`).** Skills are the **playbooks a deep agent and its sub-agents load progressively**. Ailu already has the deep-agent harness ([ADR 0023](0023-governed-deep-agent-platform-landscape.md): `writeTodos` planning, `mapAgents`/`taskNode` dynamic sub-agents, the governed fs (ADR 0024), the middleware API (ADR 0025)). What `deepagents` gets from SKILL.md — a main agent that discovers skills and pulls the relevant body in when a task matches, and specialized sub-agents that carry their own playbooks — Ailu delivers **governed**: every selected skill is attributable, namespace-scoped, deterministic, and (when it grants capability) approval-gated. This ADR's `SkillMiddleware` is the mechanism, and because `mapAgents`-spawned sub-agents are built through the **same** `build_react_agent` path as the main agent, a sub-agent inherits skill-loading too — each agent in the deep-agent composition progressively loads the right playbooks for its role.

## Decision

### The Skill type
A skill is `SKILL.md` = frontmatter + opaque markdown body. `SkillFrontmatter` (Zod in `contracts`, mirrored as a Rust struct):
- `name` (stable kebab id) · `description` (the load-bearing, embeddable task-match paragraph) · `version` (semver; referenced as `name@major.minor.patch`, matching the graph-DSL ref convention).
- `scope` (namespace convention: `skill:{tenant}:org` shared + `skill:{tenant}:agent:{id}`).
- `provenance` (who registered, principal, attestation id, `registered_at`).
- `requires?` (tools / profiles the body assumes — the **governance trigger**).
- `resources?` (L3 bundled refs — artifact ref / KB doc id / relative path).
- `embeddingModel` + `embeddingDim` (pinned per skill — the anti-drift determinism anchor).
- **Data, never code** (same invariant as `ConditionRegistry` / `ResponseFormat`). The body stays opaque markdown; it is prompt/context, never executed.

### D1 — A dedicated `SkillStore` seam (not a KB doc-type)
A new leaf crate `ailu-skills` (mirroring `ailu-memory`): the `Skill` type, a `SkillStore` trait (`register`, `select`, `get`, `tombstone`), and an `InMemorySkillStore` default (vector cosine over descriptions + explicit `name@version` lookup; insertion-ordered tie-break; version-pinned, **tombstone-not-mutate**). Postgres lives behind the same trait in the control plane (the open-core seam). Rationale (owner's call): native versioning + governance + scoping, decoupled from KB document semantics — at the cost of re-implementing selection (acceptable; it reuses the `cosine_similarity` primitive).

### D2 — Hybrid selection
The agent node carries a `SkillSpec`:
- `required: ["name@version", …]` — **explicitly pinned**, always loaded, trivially deterministic + reproducible (the governed / must-apply playbooks).
- advisory — **vector top-k** over skill `description`s in the run owner's skill namespace (the "find the right playbook" path), `k` capped.

Embedding-model drift is the determinism hazard, so each skill pins `embeddingModel`+`embeddingDim`; selection re-embeds only on a model change, and ranking is score-desc with an explicit insertion-order tie-break.

### `SkillMiddleware`
Implements `AgentMiddleware`; installed in the **efficiency** layer via `push_efficiency`, structurally inside the sealed **governed** layer (Redact / ApprovalGate / FsPolicy run first). Hook = **`before_run`** (mirroring `MemoryMiddleware`):
1. Resolve `required` (pin) + advisory (embed the seed → `SkillStore::select(skill_namespace, seed_emb, k)`).
2. Load the **L2 bodies** for the selected skills.
3. Resolve **L3 resources** on demand (D3): each `resources` ref (artifact ref / KB doc id / relative path) fetched via the artifact-store / fs seam when its skill is selected; size-capped.
4. **Prepend** the bodies (+ resolved resources) to the seed conversation as one message — the same seed-only mutation `MemoryMiddleware` uses: **no runtime state change, no new checkpoint path.**

`namespace` + `principal` are **sealed by the bridge** (never from user data), so selection is tenant-scoped by construction. Fail-open: a select/load error never sinks an otherwise-good run. The selected set (`name@version` + `embeddingModel`/`dim`) is recorded to run provenance in `after_run` for AI-Act traceability ("what know-how informed this action"). SkillMiddleware is ordered **before** any `ContextBudgetMiddleware` so the cap is final; it declines to inject a body whose token estimate would blow the budget (bodies size-capped at publish time).

### Deep-agent integration (the deepagents parity)
The `SkillSpec` is part of an agent node's config, so it applies to **both** the main deep agent and its **`mapAgents`/`taskNode` sub-agents** — each is built through `build_react_agent`, so each gets its own `SkillMiddleware` with its own `required` pins + advisory selection scoped to its role (e.g. a "research" sub-agent pins `web-research@1`, a "writer" sub-agent pins `house-style@2`). The spawned sub-agent's skill scope is sealed from the parent run context exactly like its other governance. Net: a planning deep agent + a fan-out of specialized, skill-equipped sub-agents — the `deepagents` shape, governed by construction. (Skill loading composes with `writeTodos` planning + the governed fs; a skill body may reference fs/artifact resources via D3.)

### D3 — L3 resources in v1
`Skill.resources` references (artifact ref / KB doc id / relative path) are resolved **on demand** when a skill is selected, through the existing artifact-store / governed-fs seam — never eagerly. Each resolved resource is size-capped and subject to the same redaction as the body.

### D4 — Governance
- **Registration** = **approver+** (the `contracts` `TenantRole` owner⊃approver⊃viewer), provenance-stamped (principal + attestation id + `registered_at`), **per-version** (a new version is a new registration; existing versions are immutable — tombstone-not-mutate, the fs-write pattern).
- **Scope** = namespace-scoped only in v1 (`skill:{tenant}:…`). Cross-tenant sharing / a marketplace / BKP export is a **future ADR**.
- A skill carrying **`requires`** (it grants tool / profile capability) routes through the **approval gate**, content-scoped by `name@version` (ADR 0024's `name#sha256` grant): selecting such a skill into a run that hasn't been granted it suspends, exactly like a gated tool.

## Invariants (governed by construction)
1. **Ungoverned injection is unrepresentable.** SkillMiddleware sits in the efficiency layer inside the sealed governed layer; Redact/ApprovalGate/FsPolicy see the world before any skill text, and a skill-supplied governance-kind middleware is type-rejected (ADR 0025). A skill is prompt/context only.
2. **Skills are never executed.** They are seed/system text, not tool defs; `before_tool` still gates every real action.
3. **Attributable, tenant-scoped, auditable.** Registration is approver+ + provenance-stamped; selection is namespace-checked **at the seam** (a tenant without access gets an empty result, never disclosure); the selected set is recorded per run.
4. **Deterministic + resumable.** Seed-only mutation; deterministic ranking + insertion-order tie-break; `name@version` pin + tombstone-not-mutate + pinned `embeddingModel`; the seed is rebuilt from the checkpoint so re-injection is idempotent.

## Build plan / touch-points (public flat repo)
- `crates/ailu-skills` (new): `Skill`, `SkillFrontmatter`, `SkillStore` trait, `InMemorySkillStore` (cosine select + pin + tombstone), `parse_skill_md` (frontmatter+body; reuse the OKF/round-trip parser). + unit tests.
- `crates/agents-core/src/skill_middleware.rs` (new): `SkillMiddleware` mirroring `memory.rs` (before_run, seed-prepend, fail-open, sealed scope) + `after_run` provenance record.
- `crates/bindings/src/bridge.rs`: a `SkillSpec` overlay on `AgentSpec` + `build_skill_middleware` pushed with namespace+principal **sealed** (never from `resolved_middleware`); `requires` → approval-gate wiring.
- `crates/bindings/src/spec.rs`: `SkillSpec` (`required[]`, advisory `k`, namespace).
- `crates/llm-gateway` redaction: skill body + resources scrubbed before storage/injection.
- `packages/contracts`: `SkillFrontmatterDto` + `SkillSpecDto` (Zod) + a `SkillCatalogEntryDto`.
- `packages/graph-sdk`: thread `skills` on `agentNode`; a `.rust.test.ts` asserting `SkillSpec` maps through the bridge to a real middleware (ADR 0025 parity precedent).
- SKILL.md format doc + a cookbook recipe (per the cookbook rule).

## Risks (+ mitigations)
1. **Prompt-injection via a skill body** (highest) — structural: layer ordering (governed sees pre-skill world) + registration gating (only approver-registered, attested skills are selectable) + PII/secrets redaction of skill text before storage/injection.
2. **Context bloat / U-shaped attention** — lean L1, L2 only on match, L3 lazy, bodies size-capped, `k` capped, ordered before `ContextBudgetMiddleware`.
3. **Untrusted / poisoned bodies** — only approver-registered + attested skills are advertised/selectable.
4. **Embedding-model drift** breaks replay/traceability — pin `embeddingModel`+`dim` per skill; re-embed only on change; explicit tie-break.
5. **Cross-tenant leak on selection** — the run-internal select pre-pass MUST tenant-check the skill namespace at the seam; skills do not ship until this holds (a real exfiltration path otherwise).
6. **TS↔Rust drift** — selection/desugaring specced in TS, executed in Rust; a `.rust.test.ts` asserts the `SkillSpec` → middleware mapping.
7. **Version drift on resume** — forbidden by `name@version` pin + tombstone-not-mutate + the recorded selected set.

## Implementation notes (as shipped)
- **Redaction needs no new code.** SkillMiddleware injects into the seed in `before_run`; the governed `RedactMiddleware` scrubs every `before_model` request, so injected skill text + resolved resources are redacted before any provider sees them **by construction** (the "governed sees the pre-skill world" invariant). The `llm-gateway` touch-point in the original plan is therefore a no-op.
- **The `requires` gate is omission-with-audit in v1, not a resumable suspend.** `before_run` returns only `Continue`/`Stop` (no `Gate`/approval channel like `before_tool`). So a selected capability-granting skill whose `skill:{name}@{version}` grant is absent from the run's approval set is **withheld** (never injected — invariant #1 holds) and recorded `gated` in the selected set, rather than suspending the run the way a gated *tool* does. This is the fail-safe reading of D4; the resumable human-gate-on-select UX (giving `before_run` an approval channel) is a follow-up below. **(Deviation from the literal "suspends, exactly like a gated tool" wording — flagged for the owner.)**
- **Provenance recording** is exposed via `SkillMiddleware::selected()` (the captured `name@version` + `embeddingModel` + `gated` set) for the bridge/control-plane to write to run provenance, rather than adding a field to `AgentResult` (avoids rippling the public result struct).
- **L3 resolution** is a `SkillResourceResolver` seam (optional `Arc<dyn …>`); the OSS middleware lists refs and the control plane plugs an artifact-store/fs-backed resolver — keeps `agents-core` free of those deps.

## Reserves / next
- The resumable **suspend-on-select** for capability-granting skills (a `before_run` approval/gate channel so an ungranted `requires` skill suspends like a gated tool instead of being omitted).
- A **napi skill-registration API** so the OSS in-memory store can be seeded from TS (today registration is control-plane only; the engine select pre-pass is unit-tested but not exercised end-to-end over napi).
Cross-tenant skill sharing, a skill marketplace, and BKP/portable export are a future ADR. Skill *authoring* tooling (a `create`/scaffold for SKILL.md, an inspector lens showing which skills a run loaded) follows once the seam lands.
