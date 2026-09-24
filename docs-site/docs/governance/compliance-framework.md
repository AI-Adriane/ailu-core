---
sidebar_position: 6
title: Compliance framework
description: "What the engine guarantees for governance audits: principles, layers, wire evidence, mapping to regulations."
---

# Compliance framework

Ailu's runtime is built for regulated environments. This page maps the engine's guarantees to compliance concerns: what Ailu gives you, what your control plane and deployment must add, and how to verify the chain.

## The principles × layers × verification matrix

Ailu enforces **five core principles** at **two independent layers** (engine + control plane), verified via **wire-level artifacts**. This table is the architecture.

| Principle | Engine (Rust) | Control plane | Verify via |
| --- | --- | --- | --- |
| **Governed by construction** — approvals and tools have a built-in seam, not a bypass | No-self-approval guard at `approve()` / `resume()` entry points (`ensure_can_resolve`). `approvedTools` carries `{ name, requestedBy, resolvedBy }`. | Binds `resolvedBy` to an **authenticated principal**; rejects self-approval before engine (HTTP 409). Emits approval events. | Approval record with distinct `requestedBy` ≠ `resolvedBy`. Event journal shows gate, approval, resume sequence. |
| **No self-approval** — an agent cannot approve its own request | Cryptographic check: `resolved_by` ≠ `requested_by`. Violation is an error, not a log. | Enforces authenticated principal rule (same human cannot request and approve). | Audit log entry: `{ requestedBy: "agent-id", resolvedBy: "human-id", result: "approved" }`. Ed25519 signature on resolution. |
| **Determinism & replay** — a run restarted from a checkpoint executes identically | Checkpoints capture all state at human gates and node boundaries. Resume re-enters at persisted `currentNodeId` with prior channel state. No randomness in governance paths. | Persists and supplies checkpoints durably (Postgres, Redis, or in-memory for tests). Replays run from persisted journal. | `run_suspended` / `run_resumed` events in sequence. Checksums on persisted state. |
| **Audit attestation** — every governance decision is tamper-evident | Approvals are **Ed25519 signed**: signature binds `{ approvalId, subject, resolver, outcome }` to a specific key holder. Not reversible without the key. | Signs approvals with a **private key** scoped to a principal or deployment. Publishes **public key** for verification. | Signature verification against public key: `verify(signature, publicKey, message)` succeeds. Detector: missing signature = ⚠️ unsigned decision. |
| **PII/secrets redaction** — no personal data leaves the engine unredacted | A **`PiiRedactor` seam** wraps every LLM call. Outbound texts are POSTed to a redaction service; inbound responses are un-redacted from a vault. Default is no-op (no detection service configured). | Implements or configures a **detection service** (Presidio, GLiNER, OpenAI Privacy Filter) and a **vault** for placeholder↔value mapping. Blocks on high-confidence PII by policy. | `pii_detected` / `pii_redacted` / `pii_blocked` events in run journal. Redaction log with confidence thresholds. Final artifact is anonymized. |

## The wire-level evidence: the audit trail

Every governance decision is **recorded as an event** in the run journal and **attested with a signature**. Together, they form a **complete, tamper-evident audit trail**.

### Event sequence for a governed tool call

```text
1. run_started ──▶ Agent starts

2. node_started (assistant) ──▶ Agent reaches for tool `refund`
                                 ↓
                         ┌─ approval gate (tool has requiresApproval)
                         │
3. run_suspended ◀────────┘ Run parks; state checkpointed at `__approvedTools: []`

4. [approval_pending] ──▶ Control plane files ApprovalEngine request
                          requestedBy = "assistant"
                          subject = "tool:refund"
                          status = "pending"

5. [approval_resolved] ──▶ Human (≠ agent) approves
                           resolvedBy = "alice@example.com"
                           status = "approved"
                           [Ed25519 signature recorded]

6. run_resumed ──▶ Resume called; engine validates:
                   resolved_by ≠ requested_by ✓
                   (approved) ✓

7. node_continued (assistant) ──▶ Agent re-runs; tool executes

8. run_completed ──▶ Run ends
```

### The approval record (wire)

An approval record, once emitted by the control plane, carries:

```json
{
  "id": "<uuid>",
  "runId": "<uuid>",
  "requestedBy": "assistant",
  "subject": "tool:refund",
  "status": "pending",
  "resolvedBy": null,
  "createdAt": "2026-06-24T10:00:00Z",
  "resolvedAt": null,
  "attestation": {
    "signature": "<ed25519-hex>",
    "publicKeyId": "key-2026-06-24-v1",
    "algorithm": "Ed25519",
    "message": "<sha256-canonical-json>"
  }
}
```

When a human resolves it:

```json
{
  "id": "<uuid>",
  "runId": "<uuid>",
  "requestedBy": "assistant",
  "subject": "tool:refund",
  "status": "approved",
  "resolvedBy": "alice@example.com",
  "createdAt": "2026-06-24T10:00:00Z",
  "resolvedAt": "2026-06-24T10:05:30Z",
  "attestation": {
    "signature": "<ed25519-hex>",
    "publicKeyId": "key-2026-06-24-v1",
    "algorithm": "Ed25519",
    "message": "<sha256-canonical-json>"
  }
}
```

**What you can verify:** Given the public key for `key-2026-06-24-v1`, you can verify that:

- The record was signed by the holder of that key.
- The record has not been tampered with since signature.
- The decision (`approved`, `resolvedBy`) is bound to that signature.

## Mapping to regulatory frameworks

Ailu is **alpha** — not certified for any framework. This section is honest: what the engine gives you, and what remains your responsibility.

### EU AI Act — Article 5 (transparency) & 6 (documentation)

| Requirement | Ailu provides | You / your deployment must add |
| --- | --- | --- |
| **Transparent agent decisions** — log every decision the agent makes, including tool use | `run_suspended` event when an agent reaches for a `requiresApproval` tool; the approval record with `requestedBy` (agent), `subject` (tool), `resolvedBy` (human). Approvals are attested. | Integrate the event journal into your compliance dashboard. Publish logs to a read-only audit store. |
| **Documentation of high-risk systems** — records of what a system does, who reviewed it, and when | Event journal captures: node execution, agent tool calls, approval requests, human decisions, resume points. Approval records timestamp and attribute every decision to a principal. | Translate the event journal into your compliance format (EU AI Act Annex IV checklist). Publish risk assessments per deployment. |
| **Right to explanation** — trace how the system arrived at a decision | Replay the event journal from `run_started` to decision. Checkpointed state at each step is recoverable. Tool inputs/outputs are in the transcript. | Implement a UI or API that replays the run for stakeholders. Redact PII/secrets before display. |
| **GDPR — data minimization** — minimize what reaches third parties | PII redaction seam: every LLM call is scrubbed before a provider sees it. Personal data never leaves the engine. Redaction events logged. | Implement a redaction service (Presidio/GLiNER). Set a per-namespace redaction policy (level, entities, threshold). Manage the vault for re-hydration. |

**What Ailu does *not* give you:**

- **Certification or compliance attestation.** The engine is open-source; you own the audit.
- **Role-based access control (RBAC) on approvals.** The control plane must bind approvers to authenticated principals and enforce RBAC policies (e.g., "Finance users can approve refunds ≥ €100").
- **Data retention/deletion policies.** You must implement a log retention schedule and GDPR deletion handlers.
- **Encrypted at-rest storage of PII.** The vault is ephemeral and encrypted; artifacts are anonymized. You must encrypt checkpoints and audit logs at rest.

### SOC 2 Type II — controls

| Control | Ailu | You |
| --- | --- | --- |
| **CC6.1: Logical/physical access** — only authorized users change critical systems | Engine guards approval entry points. Control plane binds approvers to identity. | Implement identity provider (OAuth2, SAML, OIDC). Audit admin actions on the control plane. |
| **CC7.2: System monitoring** — detect and alert on anomalies | Event journal captures every transition. PII redaction events logged. | Set up alerts: repeated approval rejections, out-of-hours approvals, missing signatures. Export events to SIEM. |
| **A1.1: Service availability** — critical functions remain operational | Run lifecycle is checkpointed; suspension/resume is durable. No lost work. | Use durable checkpointer (Postgres); run control plane on a high-availability cluster. Test failover. |
| **A1.2: Service continuity** — recovery from failures | Replay from checkpoint: `run_resumed` event triggers re-execution from persisted state. | Backup checkpointer and approval database. Test recovery plan (RTO/RPO). |

**What Ailu does *not* give you:**

- **Encryption at rest.** You choose the checkpointer backend and must encrypt it.
- **Network segmentation.** You deploy on your infrastructure; you control the network.
- **Audit log retention.** You must persist the event journal and enforce retention policies.

### GDPR — Articles 32, 35 (data protection)

| Requirement | Ailu | You |
| --- | --- | --- |
| **Pseudonymization** — reduce personal data in logs | PII redaction seam replaces PII with placeholders. Final artifact is anonymized. | Configure redaction policy per data subject type. Rotate placeholder mappings. |
| **Integrity & confidentiality** — prevent unauthorized access/modification | Approvals signed with Ed25519. Event journal is append-only (via checkpointer). | Encrypt at rest (TDE for Postgres, S3 KMS, etc.). Restrict read access to audit logs (role-based). |
| **Data subject rights** — provide copies, delete on request | Run state + approval records are queryable and deletable. Events are persisted. | Implement a data deletion procedure: purge run, approvals, PII vault entries by `subject`. |
| **DPIA (Data Protection Impact Assessment)** — document risks | Ailu's governance model + PII redaction reduce the risk of data leaks to LLM providers. | Document your deployment topology, data flows, and residual risks in the DPIA. |

## Deployment checklist

Before you go to production, ensure:

### Engine and SDK
- [ ] Use the **Rust engine** (the golden path). TS fallback is for dev/test.
- [ ] Implement the `Checkpointer` interface against a **durable store** (Postgres, Redis, etc.). In-memory is fine for tests only.
- [ ] Enable **PII redaction**: set `AILU_PII_REDACTOR_URL` to your redaction service (Presidio, GLiNER, or custom).
- [ ] Verify `PiiRedactor` is working: check `pii_detected` / `pii_redacted` events in a test run.

### Control plane
- [ ] Implement or use **Ailu Studio** (managed governance platform). If you build your own:
  - [ ] Bind `resolvedBy` to an **authenticated principal** (OAuth2, SAML, OIDC).
  - [ ] Reject self-approval **before** the request reaches the engine (HTTP 409 on conflict).
  - [ ] File an `ApprovalEngine` request for every `suspendForApproval` tool call.
  - [ ] Validate `approved_tools` against pending approvals before `resume`.
  - [ ] **Sign approvals** with an Ed25519 private key; publish the public key.

### Audit & compliance
- [ ] **Persist the event journal** to a read-only, append-only store (Postgres jsonb, S3, etc.).
- [ ] **Encrypt checkpoints and approvals** at rest (TDE, KMS, or envelope encryption).
- [ ] Set up **alerts** for anomalies: unsigned approvals, self-approval attempts, missing `resolvedBy`, PII block events.
- [ ] Export events to your **SIEM** (Datadog, Grafana, Splunk) or compliance dashboard.
- [ ] Implement a **data deletion procedure** for GDPR `deletionRequests`: purge run + approvals + vault entries.
- [ ] Document your **deployment topology** and **data flows** in a DPIA (Data Protection Impact Assessment).
- [ ] Test **disaster recovery**: restore from checkpointer backup, resume from an old journal entry.

## FAQ

### Can an agent approve its own tool call?

**No.** The rule is enforced at two layers:

1. **Engine** — `ensure_can_resolve` checks `resolved_by ≠ requested_by`. A violation returns an error and blocks the resume.
2. **Control plane** — rejects any attempt to approve a request by the same principal that made it (409 Conflict) before it reaches the engine.

You cannot bypass this by reaching the engine directly: both layers guard independently.

### What if the control plane is misconfigured?

The **engine still holds the line**. Even if the control plane accidentally writes `{ name: "refund", requestedBy: "assistant", resolvedBy: "assistant" }` into the resume spec, the engine's `ensure_can_resolve` check will reject it and return an error. The run will not resume.

### Is PII redaction mandatory?

No, it is optional. The seam is a no-op by default (no detection service configured). To enable it:

1. Set `AILU_PII_REDACTOR_URL` to your redaction service.
2. Implement or host a service that speaks the redaction contract (`POST /redact-batch`).
3. (Optional) Set `AILU_PII_REDACTOR_TOKEN` for authentication.

Without these, the engine routes all LLM calls unredacted (the current behavior). Fail-open is deliberate: a flaky redaction service must not break otherwise-valid runs.

### How do I verify an approval signature?

The approval record carries:

```json
{
  "attestation": {
    "signature": "<ed25519-hex>",
    "publicKeyId": "key-2026-06-24-v1",
    "algorithm": "Ed25519",
    "message": "<sha256-canonical-json>"
  }
}
```

To verify:

1. Fetch the public key for `publicKeyId` from your key store.
2. Reconstruct the canonical JSON from the approval record (deterministic field order).
3. Hash it with SHA256.
4. Call `Ed25519.verify(signature, publicKey, hash)`.

If verification succeeds, the approval was made by the holder of that key and has not been altered.

### What if I need SOC 2 or ISO 27001 certification?

Ailu is an **open-source runtime**, not a certified service. You (the deployer) are responsible for certification. Ailu **supports** the controls you need — checkpoints, approvals, audit events, PII redaction — but does not provide a compliance attestation letter.

To certify, you must:

- Document your **deployment topology** (which components, where, who operates them).
- Audit the **control plane** you build on top (identity binding, role-based approval, encryption at rest).
- Test and document **incident response** and **disaster recovery** procedures.
- Work with your auditor to map Ailu's event journal and approval model to the required controls.

### Can I audit a run after it completes?

**Yes.** The event journal captures every transition: `run_started`, `node_started`, `run_suspended`, `approval_pending`, `approval_resolved`, `run_resumed`, `run_completed`. Paired with the approval records (signed), you can reconstruct the entire run and verify every decision.

To audit:

1. **Replay the run:** read the event journal from `run_started` to `run_completed`.
2. **Verify approvals:** for each `approval_resolved` event, check the signature against the public key.
3. **Check for anomalies:** unsigned approvals, missing `resolvedBy`, same `requestedBy` and `resolvedBy`, out-of-sequence resumes.
4. **Redaction check:** look for `pii_redacted` or `pii_blocked` events; if present, verify the final artifact is anonymized.

### What does "alpha" mean for compliance?

**Alpha means:**

- The engine is **production-ready** in terms of uptime and performance.
- The **governance guarantees** (no-self-approval, attestation, determinism) are **enforced and tested**.
- **Not yet certified** for SOC 2, ISO 27001, or compliance frameworks. Expect breaking changes to the event schema, approval record format, or PII redaction contract in future releases.
- You can **use it for regulated workloads** if you are willing to own the audit and update your compliance documentation as the schema evolves.

## Next

- [The governance model](/docs/governance/governance-model) — the three principles in depth.
- [Approval gates](/docs/governance/approval-gates) — `humanGate` and `suspendForApproval` mechanics.
- [Tool approval and attestation](/docs/governance/tool-approval-and-attestation) — the wire format and signature model.
- [PII redaction seam](/docs/governance/pii-redaction) — deployment of redaction services.
- [Observable runs](/docs/governance/observable-runs) — the event journal and live views.
