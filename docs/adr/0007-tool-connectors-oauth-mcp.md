# 0007 — Tool connectors (OAuth + MCP) for knowledge ingestion

- Status: **Proposed — awaiting approval** (do not implement until accepted)
- Date: 2026-06-20
- Deciders: Mathieu (owner)
- Supersedes / relates to: [0006 — sovereign deployment & KB permissions](0006-sovereign-deployment-and-kb-permissions.md)

## Context

The knowledge base already ingests from a URL, pasted text, an authenticated API, and an MCP
connector (`POST /knowledge/:ns/{ingest-url,documents,ingest-api,ingest-mcp}`). What is missing
is a **connector layer**: a way for a tenant to *connect a tool they already use* (Notion,
Slack, HubSpot, Salesforce, Google Drive, ...), have Ailu hold the authorization, and sync
that tool's content into a namespace on a schedule.

Today the onboarding "Connect a tool" step and the Sources page capture the chosen tool
**client-side only** — there is no stored connection, no OAuth, no sync. This ADR proposes the
real thing. It touches the **control plane and the DB**, handles **OAuth secrets**, and adds a
**worker job**, so it requires review before any code (per `AGENTS.md` → mandatory human review).

## Decision (proposed)

Introduce a **connector framework** in the control plane with four parts:

1. **Provider abstraction** — a `ConnectorProvider` describing a tool: `id`, `authKind`
   (`oauth2 | api_key | mcp`), and hooks `authorizeUrl()`, `exchange()`, `listResources()`,
   `fetch()`. Adding a provider is implementing this interface, not a new subsystem.
2. **Connections registry** — a per-tenant, per-namespace record of an established connection,
   with encrypted credentials. Owner-only to create.
3. **OAuth in the control plane** — authorization-code flow with PKCE + `state`; the browser
   never sees a token. Tokens are encrypted at rest and refreshed server-side.
4. **Sync on the worker fleet** — a connection sync enqueues a BullMQ job; the worker pulls
   resources and feeds them through the **existing** knowledge ingestion (`ingest-mcp` for
   MCP-capable tools, `documents`/`ingest-api` otherwise), under the connection's namespace and
   the same governance.

### Approach: MCP-first, OAuth for the big ones

- **MCP-first.** Any tool that exposes an MCP server connects through the generic MCP connector
  with zero per-tool code (reuses `ingest-mcp`). This is the open, sovereign-friendly default.
- **OAuth providers.** For the high-value SaaS without a usable MCP server, implement OAuth2 per
  provider. Phase 1: Notion, Slack, Google Drive, GitHub. Phase 2: HubSpot, Salesforce,
  Atlassian (Confluence/Jira), Zendesk, Microsoft 365 (Graph), Linear, Intercom.
- **Rejected:** routing everything through a third-party unified-integration SaaS — it breaks
  the sovereignty posture (content would transit a non-EU aggregator).

## Data model (Drizzle, new table)

```
connector_connections
  id            uuid pk
  tenant_id     not null            -- tenant isolation
  namespace     not null            -- feeds this KB namespace (governance unit)
  provider      text not null       -- "notion" | "slack" | "mcp" | ...
  auth_kind     text not null       -- "oauth2" | "api_key" | "mcp"
  status        text not null       -- "connected" | "error" | "revoked"
  enc_credentials text              -- AES-256-GCM ciphertext (tokens / api key / mcp url)
  scopes        text[]              -- granted scopes
  created_by    not null            -- principal who connected (attribution)
  last_sync_at  timestamptz
  created_at / updated_at
```

Migration via `db:generate` (not `db:push` in prod). No plaintext secret is ever stored.

## API (new `connectors` module, control plane)

```
GET    /connectors                          # provider catalogue (id, authKind, scopes)
POST   /connectors/:provider/connect        # start OAuth (returns authorizeUrl) OR save api-key/mcp config   [owner]
GET    /connectors/oauth/callback           # code -> token exchange, store encrypted
GET    /connectors/connections              # list this tenant's connections
POST   /connectors/connections/:id/sync     # enqueue a sync job                                              [owner]
DELETE /connectors/connections/:id          # revoke + delete credentials                                    [owner]
```

DTOs in `@ailu/contracts`. Sync reuses `KnowledgeService` ingestion — no new ingest path.

## Security

- **OAuth tokens are secrets.** Encrypted at rest (AES-256-GCM) with a key from the environment
  (`CONNECTOR_ENC_KEY`); per-provider client id/secret from env (`NOTION_CLIENT_ID`, ...).
  Tokens never reach the browser; refresh happens server-side. (Consistent with the hard rule:
  secrets only via env.)
- **OAuth CSRF / interception:** `state` + PKCE; redirect URI allow-listed.
- **Tenant isolation:** every connection and every synced document is tenant- and
  namespace-scoped; cross-tenant access is impossible by construction.
- **Least privilege:** request the narrowest read scopes per provider.
- **Governance preserved:** connecting is owner-only; synced content is attributed
  (`created_by`) and lands under the namespace's existing KB governance and router policy.
- **No new execution surface:** the MCP connector uses the existing HTTP transport (no
  subprocess spawn → no RCE), matching how MCP-inbound already works.

## Impacted surface

- `packages/db` — `connector_connections` table + migration.
- `apps/api` — new `connectors` module (controller, service, oauth, provider registry);
  reuses `KnowledgeService`.
- `apps/worker` — connection-sync job processor.
- `apps/studio` — a Sources/Connectors management UI; the onboarding "Connect a tool" step
  wired to the real `connect` flow (replaces client capture).
- `contracts` — connector DTOs.
- `.env(.example)` — `CONNECTOR_ENC_KEY`, per-provider client id/secret, redirect base URL.

## Risks & mitigations

- **Secret management** → env-only + encryption at rest; document key rotation.
- **Token refresh / provider churn** → provider abstraction isolates each API; status surfaced
  in the Connections UI.
- **Sync volume / rate limits** → incremental sync (cursor/`last_sync_at`), backoff, per-job
  caps; log what was dropped (no silent truncation).
- **Scope creep** → ship behind a feature flag; phase 1 is the framework + MCP generic +
  Notion/Slack/Google/GitHub + the Sources UI.

## Rollout

1. **Phase 1** — framework, DB table, generic MCP + generic API connector, OAuth for
   Notion/Slack/Google/GitHub, Sources/Connectors UI, onboarding wiring. Feature-flagged.
2. **Phase 2** — HubSpot, Salesforce, Atlassian, Zendesk, MS365, Linear, Intercom.
3. **Phase 3** — scheduled/auto sync + incremental cursors + webhook-driven freshness.

## Open questions for review

1. OK to add `connector_connections` to the DB schema (and a migration)?
2. Phase-1 provider set: Notion, Slack, Google Drive, GitHub + generic MCP/API — agree?
3. Encryption key strategy: single `CONNECTOR_ENC_KEY` env now, KMS later — acceptable?
4. Auto-sync scheduling in phase 1, or manual sync only until phase 3?
