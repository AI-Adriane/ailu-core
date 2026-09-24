# ADR 0026 — Memory architecture: durable knowledge graph, agent memory, and portable export (engine ↔ control plane ↔ Studio)

- Status: **Accepted** (GO given 2026-06-24; the OSS-engine increment phase 11a/11b is shipped — see "Engine increment shipped" below; control-plane sub-phases A,C,E,F remain)
- Date: 2026-06-23
- Deciders: Mathieu (owner)
- Builds on: [ADR 0003](0003-ts-engine-deprecated-sdk-on-rust.md) (Rust engine + thin SDKs), [ADR 0005](0005-multi-provider-llm-gateway.md) (provider/embeddings gateway, BYOM), [ADR 0006](0006-sovereign-deployment-and-kb-permissions.md) (KB permissions + sovereign modes), [ADR 0007](0007-tool-connectors-oauth-mcp.md) (inbound connectors → KB), [ADR 0008](0008-pii-redaction-and-anonymization.md) (per-namespace policy DSL + redaction), [ADR 0011](0011-resource-search.md) (`SearchProvider` seam), [ADR 0013](0013-llm-council-governed-deliberation.md) + [ADR 0024](0024-governed-virtual-filesystem-seam.md) (the "governed version of a known primitive" bet; seam + approval-gate composition), [ADR 0014](0014-engine-token-efficiency.md) (working-memory compression)
- Relates to: the Ailu-Nexus founding thesis — this ADR is the build plan for **moat #1 (the institutional knowledge graph)** and the ICP-2 promise ("your AI learns / memory capitalizes").

## Context

"Memory" in Ailu is not one thing — it is spread across several surfaces that grew independently. Before deciding *what to build*, name *what exists* (grounded in the code, 2026-06-23):

| Plane | What it is | Where it lives today | State |
| --- | --- | --- | --- |
| **M1 Execution** | Run state channels + checkpoints | `graph-runtime` (Rust authoritative) | **Built.** The determinism/resume spine. Out of scope here. |
| **M2 Knowledge** | Institutional memory: documents + a typed graph | `knowledge` seam (`KnowledgeStore`) + control-plane `KnowledgeService` → Postgres | **Partial.** Durable, but **document-level** graph, no pgvector. |
| **M3 Agent memory** | Per-agent/tenant recall across runs (KV + semantic) | `memory-store` `BaseStore` | **Stub.** `InMemoryStore` only; `PgStore` throws "not implemented". |
| **M4 Portable** | Export/import of knowledge | `okf` package + `GET :ns/okf` | **Format only.** OKF round-trips; no versioned protocol. |

What M2 actually does today (more than the v1 gap analysis claimed):
- **Ingestion** — five paths into the KB, all `@Roles("approver")`, tenant-scoped via `ensureNamespaceAccess`: manual `documents`, `ingest-url`, `ingest-api`, `ingest-mcp` (inbound MCP), and OAuth connectors (ADR 0007) whose `runSync` calls `KnowledgeService.ingest`.
- **Persistence** — `kbDocumentsTable` (content, `embedding` **jsonb `number[]`**, type, title, tags, links, frontmatter…), `kbRelationsTable` (`id = "{from}|{type}|{to}"`, namespace, fromId, toId, type), `kbNamespacesTable` (namespace → tenantId, first-write claim). **No `vector` column, no pgvector** — cosine is computed **in-process** over a namespace's docs (`KnowledgeService.search`).
- **Embeddings** — real Mistral via the gateway (`MISTRAL_API_KEY`) or a deterministic offline hash (FNV-1a bag-of-tokens, L2-normalized) so dev/test run with no key.
- **Extraction (signal vs noise)** — `ExtractionService.extractRelations(namespace)` already exists: an LLM pass (Mistral, `temperature 0`) reads the corpus and proposes **typed relations between documents** (`depends-on`/`relates-to`/`enables`/`contrasts-with`/`part-of`), validates every id against the corpus (cannot invent ids), and persists to `kbRelationsTable`. Its own comment: *"the LLM-extraction half deferred in P4"* — i.e. this is the **document-level** version; **entity-level extraction is the explicit remaining work**.
- **Activation (outbound)** — `ActivationService.activate` retrieves KB hits for a query and POSTs them to an http(s) webhook (CRM / Slack / agent). Knowledge flows back out, not just in.
- **Governance already attached** — tenant isolation (ADR 0006), role gating, per-namespace PII policy (ADR 0008), lexical cross-resource search (ADR 0011, ES seam). Studio: `/sources` (ingest, semantic search, URL ingest, connectors, PII policy) and `/sources/graph` (doc-graph view, typed edges).

The honest deltas this ADR targets:
1. **No pgvector** → in-process cosine is O(n) per namespace; will not scale past a few thousand docs.
2. **Graph is document-level, not entity-level** — nodes are whole documents; there are no first-class **entities** (person / project / decision / system / policy). The thesis moat #1 (a graph of *decisions / people / projects*) requires sub-document entities — the deferred half.
3. **Agent memory is not durable** — `PgStore` is a stub; working-memory compression (ADR 0014) exists but is not persisted. Agents do not recall across runs.
4. **Memory writes are not provenance/governance-first** — no uniform "who/what/when/from-which-source/with-what-confidence/under-which-approval" on every memory item; extraction is not attested.
5. **No memory lifecycle** — no freshness/decay, dedup/merge, forgetting (GDPR erasure), or retention.
6. **Run-internal retrieval is not tenant-checked** — ADR 0006 flagged this: the `semanticRetriever` pre-pass does not yet verify the run owner's access to the namespace.
7. **OKF is a format, not a portable versioned protocol** — no enforced `okf_version`, integrity manifest, or import conformance (no "BKP").

Architectural constraint (open-core; ADR 0003/0011/0024): engine packages & crates stay **framework- and DB-free** — seams + in-memory defaults; real persistence (Postgres + Neo4j) lives in the **control plane** behind those seams; Studio consumes only `contracts` + `ui`. The Rust core is authoritative; SDKs are thin.

## Decision

Adopt a **unified, governed memory architecture**: four memory planes behind **stable engine seams**, persistence and governance in the control plane, surfaces in Studio. The bet is the same as the council (ADR 0013) and the fs (ADR 0024): not a new trick — the **governed** version of known primitives. Every memory write is **attributable, tenant-scoped, PII-aware, and auditable**; high-impact entity assertions can be **approval-gated**; every retrieval is **deterministic and tenant-checked**.

### 1. Engine seams (Rust authoritative + TS mirror, DB-free)

Keep the existing seams (`KnowledgeStore`, `BaseStore`, rag `VectorStore`/`Retriever`/`Embedder`). Add, additively:

- An **entity layer** on the knowledge model (documents stay nodes; entities are a new node kind; `mentions` edges link docs↔entities):

```ts
type KbEntity = {
  id: string;            // canonical, dedup key (e.g. "person:jane-doe")
  namespace: string;
  type: string;          // person | project | decision | system | policy | … (open kebab vocab)
  name: string;
  attributes: Record<string, unknown>;
  provenance: MemoryProvenance;
};
type KbEntityEdge = { from: string; to: string; type: string; provenance: MemoryProvenance };
```

- A **`MemoryProvenance`** value object threaded through *every* write seam (M2 + M3):

```ts
type MemoryProvenance = {
  runId?: string; nodeId?: string;   // which run/node wrote it
  principal?: string;                // who (agent id or human)
  sourceDocId?: string;              // which document it came from
  attestationId?: string;           // Ed25519 chain entry (governed writes)
  extractedAt: string;              // ISO-8601
  confidence?: number;              // extractor confidence (0..1)
  status?: "asserted" | "verified" | "rejected";
};
```

- A pure **`RetrievalPolicy`** (tenant scope, `k`, score/recency weighting, dedup) with **stable, deterministic ordering** (explicit tie-break) so retrieval is reproducible — the determinism contract extends to reads.
- Embeddings stay behind `EmbeddingsAdapter` (ADR 0005): provider-agnostic, BYOM, local model on-prem (ADR 0006). The engine default impls stay **in-memory** so the OSS dev experience is full-fidelity with zero DB.

### 2. Control-plane persistence (Neo4j behind the seams)

> **Resolved (decision #1 = Neo4j, scope = "everything in Neo4j").** All memory — M2 (documents +
> entities + edges) and M3 (agent memory) — and **all vector search** (Neo4j 5 native vector index)
> live in **Neo4j**. Postgres keeps **only the relational control plane**: tenancy, namespace
> ownership (`kb_namespaces`), per-namespace policies, runs, approvals, events, workers, connector
> config. The existing `kbDocumentsTable` / `kbRelationsTable` **migrate out of Postgres** into Neo4j.
> Neo4j is a **hard dependency for all memory persistence** (dev included). `neo4j-driver` is an
> external SDK → it lives in the **control plane only** (the ADR 0011 precedent: an external client
> never enters an engine package); the engine keeps its in-memory seam defaults.

- **Neo4j model** — `(:Document)`, `(:Entity)` nodes (labelled by `type`), typed relationships
  (`[:RELATES_TO {type}]`, `[:MENTIONS]`, doc↔doc extracted edges), `(:MemoryItem)` for M3. Every
  node carries `namespace`, `provenance`, and an `embedding` property indexed by a **native vector
  index** (`db.index.vector.createNodeIndex`, cosine). Store `embeddingModel` + `dim` per node
  (decision #4); a model change re-embeds (a vector index is dimension-fixed). Tenant scoping: nodes
  carry `namespace`; access is gated by `kb_namespaces` ownership in Postgres (`ensureNamespaceAccess`),
  `approver+` to write (ADR 0006).
- **Traversal** = Cypher (`MATCH … -[*..d]- …`) instead of recursive CTEs — the reason Neo4j was
  chosen (rich, cheap multi-hop).
- **Edition / sovereignty** — Neo4j **Community, self-hosted** in all three deploy modes (ADR 0006);
  Aura (managed) is excluded (breaks on-prem / egress). Community is **GPLv3** — fine as an external
  service the customer runs (like Postgres), but it does **not** link into the Apache-2.0 engine.
- **Governed extraction v2** — extend `ExtractionService` from doc→doc to **entity extraction**: an LLM
  pass (gateway, `temperature 0`, **keep the no-invented-ids / grounding guard**) emitting entities +
  edges with `provenance` + `confidence`, written as Neo4j nodes/relationships. **PII-redact** text
  before the model and before storage (ADR 0008). Writes are **attributable + attested** (Ed25519
  chain, ADR 0024 precedent). **High-impact assertions** route through the **existing approval gate**
  (ADR 0024). Idempotent (`MERGE` on a canonical entity key).
- **Agent memory (M3)** — the control-plane `BaseStore` impl targets Neo4j (`(:MemoryItem)` + vector
  index). Namespacing per decision #7: `tenant:{tenantId}:agent:{agentId}` + an opt-in shared
  `tenant:{tenantId}:org` scope.
- **Lifecycle** — freshness (`updatedAt`), dedup/merge (`MERGE` on canonical keys), **forgetting**
  (GDPR erasure: `DETACH DELETE` the node + cascade rels + re-index + an attested *tombstone* of the
  deletion in Postgres), per-namespace retention.
- Reconcile with search (ADR 0011): **lexical cross-resource** search stays in Elasticsearch;
  **semantic** retrieval = Neo4j vector index. Both behind their seams; no overlap. Postgres pgvector
  is **not** used (decision #1).

### 3. Retrieval governance (the read path that feeds agents)

- **Tenant-check the run-internal pre-pass** — close the ADR 0006 follow-up: `semanticRetriever` must verify the run's owner tenant has access to the namespace; cross-tenant → empty result, never disclosure.
- **Record the retrieval set per run** — which doc/entity ids + scores informed each run, stored as run provenance. This makes a governed run able to answer *"what knowledge informed this action"* — extending the AI Act traceability report (a real, shipped surface).
- **Observability** — one `Span` per retrieval (`memory.retrieve {namespace,k,ids,scores,principal,runId,nodeId}`), off the hot path; visible in Studio.

### 4. Studio surfaces (contracts + ui only — never import an engine package)

- **Sources** (exists) — add per-document provenance, freshness, PII status, re-extract.
- **Knowledge graph** (`/sources/graph` exists, doc-level) → **entity graph**: entities + docs as nodes, typed edges, filter by type; click a node → provenance + source docs + the runs that wrote it. The visual proof of *"the company's memory it owns."*
- **Agent memory inspector** (new) — per agent/tenant: browse M3 items, semantic search, see what an agent "remembers", edit/forget (governed).
- **Run memory panel** (new) — per run: the retrieval set from §3 (traceability), surfaced in the run + compliance views.
- **Memory governance** — forgetting/erasure (owner), per-namespace retention (PII precedent), entity-extraction approvals surfaced in the existing approvals queue.

### 5. BKP — portable memory protocol (built on OKF)

Promote OKF from a *format* to **BKP v1**, a *protocol*: a versioned (`okf_version` enforced + a `bkp_manifest`), integrity-checked (content hashes) bundle that **exports** a namespace's documents + entities + edges + provenance, **with embeddings (model-tagged) and the namespace's M3 agent memory included** (decision #6), and **re-imports losslessly** into another Ailu (sovereignty/portability) or a third party (the ecosystem seed). Transport on **MCP** (the thesis bet). A **conformance test** (export → import → export is byte-stable) is what makes it a *standard*, not just a dump. Including embeddings makes a bundle model-specific (a portability caveat the manifest records); a re-import into a different embedding model must re-embed. Certification program + partner marketplace are **explicitly out of scope** (their own ADR) — this delivers the portable protocol, not the ecosystem program.

### 6. Invariants preserved

No new runtime path: memory writes/reads live in tool/agent node handlers and control-plane services, so the runtime contract (checkpoint after each node, event per transition, clean suspend/resume) is inherited. Extraction approval **reuses** the existing gate (ADR 0024) — no new gate mechanism, no new interrupt kind. Engine stays DB-free behind seams. Wire shapes camelCase; `Result` discriminated unions; typed error classes; no `eval`/dynamic import of strings; secrets via env only.

## Engine increment shipped — phase 11a/11b (2026-06-24, Mathieu GO; "vector + graph")

The first OSS-engine slice landed (the seams + in-memory defaults + the loop wiring); Neo4j persistence, governed LLM extraction, lifecycle, BKP and Studio stay control-plane/follow-up (sub-phases A,C,E,F below). Resolved decisions for this increment:

- **D1 — a new unified `ailu-memory` crate** (not extending the scattered crates): one home for both recall modalities. `MemoryItem` (vector) + `MemoryEntity`/`MemoryEdge` (the entity graph — "vector + graph") + `MemoryProvenance` + `RetrievalPolicy` + the `MemoryStore` seam + `InMemoryMemoryStore` default (cosine vector recall via `rag-pipeline` + depth-limited adjacency-BFS graph recall, deterministic insertion-order tie-break). DB-free; the control plane plugs a Neo4j impl behind the same seam.
- **D2 — recall is GOVERNED, with tunable quality knobs**: `MemoryMiddleware` (agents-core, `before_run` recall→inject seed / `after_run` persist) installs via `push_governed`, constructed with its `namespace` + `principal` **sealed by the bridge** (never user-routable, since `RunCtx` carries no tenant) — `top_k`/`recall` are author-tunable on that sealed instance. Never a `{kind:"memory"}` efficiency entry.
- **D4 — a `memory` overlay on the agent node** `{ namespace, topK?, recall? }` (forward-compatible), threaded SDK → wire → bridge → middleware (mirrored in contracts).
- **D5 — provenance on every write IN-SCOPE; LLM entity extraction DEFERRED** to the control plane (sub-phase C): this increment ships entity TYPES + provenance + heuristic persist (the run's reasoning) — **no LLM claim-writing in the OSS engine**.
- OSS default persistence is the **process-global in-memory store** (recall works across runs intra-process); cross-process durability + the native vector index = Neo4j, control-plane.

## Engine increment shipped — phase B durable-recall channel (2026-06-26, Mathieu GO "1b")

Sub-phase B's engine half: a **reserved input channel** lets the control plane supply the durable
recall that the in-process store cannot (cross-process Neo4j hits). Minimal + additive — one read in
`MemoryMiddleware::before_run`, no new wire shape, no binding change (channels already flow through
runtime state):

- **`__memoryRecall`** (`crate::memory::MEMORY_RECALL_CHANNEL`) — a JSON array of strings. When the
  control plane recalls from Neo4j pre-run and seeds this channel into the initial state, `before_run`
  injects **those** items into the seed (the durable path) and **skips** the in-process store recall.
  Absent the channel, recall falls back to the in-process store (the unchanged OSS default). Tolerant:
  a missing/malformed channel → empty list (fail-open). Recall still mutates **only the seed** — no
  runtime-state change, determinism preserved.
- **Write path stays control-plane, NO engine change**: the control plane reads the run's reasoning
  from the outcome state post-run and persists it to Neo4j (`(:MemoryItem)` + vector index). The
  engine's `after_run` heuristic persist to the in-process store is untouched (OSS default).
- Gated on the **1.6.0** napi/SDK republish (the channel read ships in `ailu-agents-core`); seeding
  `__memoryRecall` against an older engine is harmless (the channel is simply ignored).

## Sub-phasing (each ships + is reviewed independently; governed parts get the closest review)

- **A — Neo4j + durable stores** (control plane, behind seams). Stand up Neo4j (Community, self-hosted) + vector index; migrate the KB out of Postgres; back the `KnowledgeStore` + `BaseStore` (M3) seam impls with Neo4j. No engine seam-shape change. Highest scale unlock + the substrate for the entity graph.
- **B — Agent memory wired** (M3). Wire the Neo4j-backed agent store + semantic recall into the agent loop and working-memory compression (ADR 0014); namespacing + provenance (per-(tenant, agent) + opt-in org scope). *Delivers ICP-2's "your AI learns across runs."*
- **C — Entity knowledge graph** (M2 — the moat). Engine entity model + tables + **governed extraction v2** (PII-redacted, attributable, attested, optionally gated) + dedup/merge; Studio entity graph. **Closest review — it writes claims about people/decisions.**
- **D — Retrieval governance**. Tenant-check the run pre-pass (close ADR 0006), deterministic + recorded retrieval set, run memory panel + observability.
- **E — Lifecycle + forgetting**. Freshness, dedup/merge, GDPR erasure + retention, Studio governance UI.
- **F — BKP v1**. Versioned + integrity export/import + conformance test. (Marketplace/certification deferred.)

Recommended order: **A → B** (immediate ICP-2 value, low risk) then **C** (the moat, highest risk) with **D** alongside; **E**/**F** follow.

## Resolved decisions (2026-06-23, Mathieu)

| # | Decision | Call |
|---|---|---|
| 1 | Entity / memory store backend | **Neo4j** — *and* "everything in Neo4j": all memory (M2 docs+entities+edges, M3) + all vector search in Neo4j 5; Postgres = relational control plane only; pgvector **not** used. Edition: Community, self-hosted (sovereign), all modes. |
| 2 | Extraction trigger | **Manual + scheduled**, per-namespace auto opt-in. |
| 3 | Approval for entity writes | **`asserted → verified` review queue**; high-impact assertions gated through the existing approval engine. |
| 4 | Embedding model/dim | Store **`embeddingModel` + dim per node**; dim 1024 (Mistral); **re-embed on model change** (vector index is dimension-fixed). |
| 5 | Forgetting | **Erase content (`DETACH DELETE`) + keep an attested deletion tombstone** (in Postgres). |
| 6 | BKP scope | **M2 docs + entities + edges + provenance, embeddings included (model-tagged), and M3** agent memory. |
| 7 | Agent-memory scope | **Per-(tenant, agent)** + an opt-in shared `org` scope. |

**Derived architecture change (from #1 "everything in Neo4j"):** the original pgvector plan is dropped.
Neo4j becomes a hard runtime dependency for all memory; the existing Postgres KB tables migrate to
Neo4j; dev/test compose gains a Neo4j service (not a pgvector image). The engine seams are unchanged
(in-memory defaults stay); only the control-plane impls behind them target Neo4j. See the execution
plan: [memory-architecture-plan.md](../memory-architecture-plan.md).

**Residual sub-question (low stakes):** whether KB document *content* fully relocates to Neo4j, or
Postgres stays a thin system-of-record for ingested rows with Neo4j as the graph+vector layer. Current
reading: full relocation (Postgres keeps only tenancy/namespace-ownership/policies/runs/approvals/
events). Revisit if the ingestion pipeline is simpler with a Postgres landing table.

## Risks

- **Entity-extraction hallucination (highest severity)** — the LLM asserts a false relation about a person/decision → reputational/compliance harm. Mitigations: id/grounding validation (already present), `provenance` + `confidence`, optional approval gate, an `"asserted" → "verified"` status, and a human-review surface. *It writes claims about people — treat it like the fs gate.*
- **PII into the graph** — extracted text or entity attributes carry PII. Must redact on **every** ingest + extract path (ADR 0008), not just chat — verify all five ingest paths apply the namespace policy.
- **Cross-tenant retrieval leak** — the un-tenant-checked run pre-pass (ADR 0006) is a live gap; §3/phase D closes it; document the limit until then.
- **Neo4j as a second datastore (the cost of decision #1)** — a hard dependency for *all* memory in every deploy mode (dev included); ops burden + the existing Postgres KB must be migrated into Neo4j. Community is GPLv3 → external service only, never linked into the Apache-2.0 engine. The native vector index is dimension-fixed → a model change forces a re-embed. (The single-Postgres alternative would have avoided this — accepted trade-off for richer traversal.)
- **Unbounded growth / cost** — embeddings + entities accumulate; lifecycle (E) must land before heavy use.
- **Non-deterministic retrieval** — semantic search must have a stable tie-break or governed runs stop being reproducible (violates the core contract).
- **Scope creep** — A–F must stay independent; C (the moat) is the riskiest — it must not block A/B (the immediate, low-risk ICP-2 unlock).

## Consequences

- The thesis **moat #1 (the entity knowledge graph)** becomes real and **governed by construction**: attributable, attested, PII-aware, optionally gated, with provenance back to the run that wrote each fact. The "switching cost" power (a 12–24-month, irreconstituable memory) finally has a substrate.
- **ICP-2's "your AI learns / memory capitalizes"** becomes true (B), and Neo4j (A) makes it scale + gives the graph cheap multi-hop traversal — without moving the open-core boundary (engine stays seam-only; persistence stays control-plane).
- **Retrieval traceability** extends the AI Act compliance report ("what knowledge informed this action"), reinforcing the already-real governance moat.
- **BKP (F)** turns OKF-the-format into a portable protocol — the sovereignty promise and the seed of the ecosystem play (program deferred to its own ADR).
- No runtime change, no new gate mechanism, engine stays DB-free — composed from existing, tested primitives (the ADR 0013/0024 bet again).

## Alternatives considered

- **Postgres + pgvector + recursive-CTE** (the original recommendation) — one sovereign datastore, no new ops, keeps the ADR 0006 single-Postgres story intact. **Rejected** by decision #1 in favour of Neo4j (richer multi-hop traversal + native vector index, the thesis-named substrate); cost = a second datastore (see Risks). Still available behind the seam if Neo4j is ever dropped.
- **Keep JSONB + in-process cosine** — zero new infra, fine at toy scale, but O(n) per query and no ANN; rejected past a few thousand docs. Kept only as the export fallback.
- **External vector DB (Pinecone / Weaviate / Qdrant)** — listed in the thesis, but cloud SaaS (egress) conflicts with sovereign on-prem; Neo4j's native vector index keeps graph + vectors in one self-hosted store. Available behind the seam if a customer wants Qdrant on-prem.
- **One unified store for M2 + M3** — conceptually clean, but KB (institutional, shared, role-gated) and agent memory (per-agent recall, different lifecycle) have different access models; keep separate seams over shared infra.
- **Extraction as a hard-coded pipeline vs a governed Ailu graph** — building extraction itself as an Ailu graph (dogfood: gates, checkpoints, attestation for free) is attractive but heavier; start as a service, graph-ify later.
