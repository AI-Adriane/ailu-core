# ADR 0006 — Sovereign deployment modes + granular knowledge-base permissions

- Status: Accepted
- Date: 2026-06-19
- Builds on: ADR 0003 (Rust engine), the open-core boundary, the tenancy/RBAC model

## Context

The knowledge base (persistent OKF vector store + relational graph) is institutional
memory: an organization's accumulated knowledge. Two sovereignty requirements follow:

1. **Where the data runs is the customer's choice** (EU residency, or no egress at all).
2. **Who can read/write which knowledge base is controlled** — knowledge is partitioned by
   tenant and gated by role.

The KB endpoints shipped (P5–P7) were initially un-scoped; this ADR closes that.

## Decision

### 1. Granular per-knowledge-base permissions

- A **namespace is a knowledge base**, owned by a tenant. Ownership is recorded in
  `kb_namespaces` (`namespace → tenantId`) and **claimed by the first tenant that writes**
  to it. A request from another tenant is refused with **404** (no cross-tenant disclosure,
  consistent with the runs/graphs tenancy pattern).
- **Role gating** at the control plane: write operations (`documents` POST, `ingest-url`,
  `ingest-api`, `ingest-mcp`, `okf` POST, `extract`, `activate`) require **`approver+`**
  (`@Roles("approver")`); reads (`documents` GET, `okf` export, `graph`, `neighbors`,
  `search`) stay open to **`viewer+`**. Enforced by the global `RolesGuard`; the
  `ensureNamespaceAccess` check enforces tenant isolation. Under `AUTH_DISABLED=true` the
  system tenant/owner is injected (dev/seed).

### 2. Sovereign deployment — three modes

The engine is framework-agnostic and the whole stack is containerized; the same artifacts
deploy in three postures, selected by where Postgres/Redis and the LLM provider live:

| Mode | Hosting | LLM | Data egress |
| --- | --- | --- | --- |
| **EU cloud** | OVH / Scaleway / Hetzner | hosted EU (e.g. Mistral) via `MISTRAL_API_KEY` | within EU |
| **Private cloud** | customer's own AWS/Azure/GCP account | customer's provider keys | within customer perimeter |
| **True on-premise** | customer hardware (Docker/VM) | local models — `AILU_USE_OLLAMA=1` / `AILU_USE_LMSTUDIO=1` (no key leaves) | **none** |

BYOM is already a property of the multi-provider gateway (ADR 0005): the deployment picks a
provider via env, and the on-prem mode uses a local OpenAI-compatible server so **no data or
key ever leaves the perimeter**. No code differs between modes — only configuration.

## Consequences

- Each knowledge base is tenant-isolated and role-gated; multi-tenant SaaS and single-tenant
  on-prem use the same code path.
- On-premise sovereignty is real (local embeddings + local chat model + local Postgres) — a
  structural differentiator a cloud-only product cannot match.
- Cross-tenant isolation is enforced by `ensureNamespaceAccess`; live multi-tenant testing
  requires real auth (under `AUTH_DISABLED` a single system tenant owns everything).
- Run-internal KB reads (the `semanticRetriever` pre-pass) are not yet tenant-checked against
  the graph's owner — a follow-up hardening.
