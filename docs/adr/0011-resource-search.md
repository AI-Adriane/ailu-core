# ADR 0011 — Unified resource search (graphs / agents / KB) with an Elasticsearch backend behind a `SearchProvider` seam

- Status: Accepted
- Date: 2026-06-21
- Deciders: Mathieu (owner)

## Context

The Studio top-nav carries a "Search resources…" input that is purely decorative. Users want
Algolia-style instant search across the resources they own — **graphs**, **agents** (agent types),
and **knowledge-base documents** — from a single box.

The KB already has *semantic* search (cosine over stored embeddings, per namespace). What is
missing is a **cross-resource, lexical, tenant-scoped** search surface. The owner chose
**Elasticsearch** as the real backend.

Two constraints shape the design:

1. **Open-core layering.** The engine packages must stay framework- and SDK-free
   (`llm-gateway` is the only package allowed to import a provider SDK). An Elasticsearch
   client (`@elastic/elasticsearch`) is an external SDK, so it **cannot** live in an engine
   package — it belongs in the control plane (`apps/api`).
2. **Lean free-tier deploy.** Local dev, tests, and a no-ES deployment must keep working. So
   ES must be optional, with a built-in fallback (mirrors the PII redactor seam, ADR 0008).

## Decision

Introduce a **`SearchProvider` seam**:

- **Engine** — new pure package `@ailu/search`: the `SearchProvider` interface, the wire
  types (`SearchDocument`, `SearchHit`, `SearchResourceType`, `SearchQueryOptions`), and an
  **`InMemorySearchProvider`** (token-overlap scoring, tenant + type filtering, snippeting).
  Zero runtime deps beyond the engine convention. This is the OSS default and the dev/test
  backend.
- **Control plane** (`apps/api`) — `ElasticsearchSearchProvider` (uses `@elastic/elasticsearch`).
  A `@Global` `SearchModule` picks the impl by a factory: `ELASTICSEARCH_URL` set →
  Elasticsearch (creates the index on boot); otherwise the in-memory fallback.
- **No DB schema change.** The search index lives in Elasticsearch (or in memory). Postgres
  stays the source of truth.

### Indexed documents

| type    | source                         | tenantId            | searchable text                              | href              |
|---------|--------------------------------|---------------------|----------------------------------------------|-------------------|
| `graph` | `graphsTable`                  | row tenant (or null = catalog) | name + `definition.metadata.description` + node labels | `/graphs/:id`     |
| `kb`    | `kbDocumentsTable`             | namespace owner     | title + content + tags (+ namespace)         | `/sources?ns=…`   |
| `agent` | agent-types (static, global)   | null (global)       | the agent-type id                            | `/agents`         |

### Tenancy

Every query is tenant-scoped: a hit is returned only when `doc.tenantId === callerTenant` **or**
`doc.tenantId` is null (global catalog / agent types). The control-plane controller derives the
tenant from the authenticated principal (`@CurrentTenant`), never from a client field. The
in-memory and Elasticsearch providers both enforce this filter.

### Write path

Indexing is **fire-and-forget, best-effort**: `GraphsService` (create/update/delete) and
`KnowledgeService` (document upsert) push/remove their own `SearchDocument` after the primary
write, wrapped in try/catch so an indexing failure (e.g. ES down) **never** fails or rolls back
the user's write. A `POST /admin`-style `POST /search/reindex` (owner-only) rebuilds the whole
index from Postgres + the static agent-type list, for backfill and recovery.

### API

- `GET /search?q=&types=graph,agent,kb&limit=` → `SearchHitDto[]` (tenant-scoped).
- `POST /search/reindex` → owner-only full rebuild.

### Studio

The static input becomes a `GlobalSearch` client component: debounced (≥2 chars) call to
`/search`, a results dropdown grouped by type with keyboard navigation (↑/↓/Enter/Esc), each hit
linking to its `href`.

## Consequences

- Local dev / tests / no-ES deploys run on the in-memory provider with no setup; production sets
  `ELASTICSEARCH_URL` (Elastic Cloud or a self-hosted ES on Fly — documented in `DEPLOY.md`,
  with the cost trade-off).
- The seam keeps the engine SDK-free and lets the backend be swapped (Typesense/Meilisearch)
  without touching call sites.
- Search is eventually-consistent with Postgres (best-effort indexing); `POST /search/reindex`
  reconciles drift. The index is not authoritative — losing it only degrades search until a
  reindex.
- New engine public API (`@ailu/search`) — additive, no breaking change to existing
  packages.

## Alternatives considered

- **Postgres full-text (tsvector/trigram) behind the same seam** — zero new infra, but the owner
  asked for real Elasticsearch. The seam keeps PG-FTS as a future drop-in if desired.
- **Managed Algolia/Typesense Cloud** — extra vendor + API key + cost; rejected in favor of ES.
- **Reuse the KB semantic search for everything** — embeddings are per-namespace and don't cover
  graphs/agents; lexical cross-resource search is a different shape.
