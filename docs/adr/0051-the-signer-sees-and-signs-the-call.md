# ADR 0051 — The signer sees and signs the call, and one signature is one call

- Status: **Proposed** — 2026-10-10, for review by the owner before any release. D1–D3 are
  implemented on branch `engine/signed-tool-input`; D4–D5 on the follow-up branch
  `engine/approval-per-call`; D6 is a proposal only.
- Date: 2026-10-10
- Deciders: Mathieu (owner)
- Driven by the product's beta QA of 2026-10-09 (product repo `docs/audits/2026-10-09-beta-qa`,
  §4.3 decision 3 and §8.3 « approbateur aveugle »), and the product's open points of 2026-10-03
  (« l'attestation Ed25519 signe `tool:refund`, pas l'empreinte de l'appel ») and 2026-10-09
  (« `__approvedTools` vaut pour le nom de l'outil tout le run »). Mathieu chose on 2026-10-10 to
  change the engine through a pull request he reviews before any release.
- Relates to: [0024](./0024-governed-virtual-filesystem-seam.md) (content-scoped grants, the
  canonical call hash), [0025](./0025-unified-agent-middleware-api.md) (the gate, intrinsic to the
  stack), [0038](./0038-replay-as-evidence.md) / [0040](./0040-verify-replay-control-plane.md)
  (verify-replay), [0045](./0045-host-nodes-in-rust-and-sdk-parity.md) (the engine decides a
  catalog run's approvals), [0046](./0046-an-approval-above-a-threshold.md) /
  [0048](./0048-an-approval-for-named-values.md) (a grant that is the call),
  [0049](./0049-the-host-keeps-each-checkpoint.md) D3 (a node is re-run, at least once).

## Context

Read from the source on 2026-10-10 (release 2.6.0, `origin/main` 15e0218).

**P0-2 — the approver does not see what they sign.**

- The gate is `MiddlewareStack::before_tool` (`crates/agents-core/src/middleware.rs:259`). For a
  tool gated by its name (`requires_approval`, no `approvalWhen`, not content-scoped) it built
  `ApprovalRequestItem { subject: "tool:<name>", reason, approval_key: None, input: None }`
  (`:279`–`:280`): the call's arguments were dropped. Only a content-scoped call (ADR 0024) or a
  conditioned one (ADR 0046/0048) carried `input` and `approvalKey`.
- The item is written into the agent's output channel (`crates/agents-core/src/node.rs:57`, the
  node's interrupt patch), read back as `pendingApprovals` (`collect_pending_approvals`,
  `crates/runtime-bridge/src/lib.rs:2115`) and, on a governed catalog run, filed in the host's
  approval engine by the filing plan (`normalize_subject`,
  `crates/runtime-bridge/src/catalog_approvals.rs:183`) — so the product stored
  `{ description: "tool:refund" }` and showed « Aucun paramètre structuré ».
- The attestation (`crates/approval-engine/src/attestation.rs:172` `build_view`, TypeScript
  `packages/approval-engine/src/attestation.ts` `toView`) signs `subject` = the description
  string: the proof of a refund's approval does not say which refund.
- verify-replay compares `{ status, subject }` (`crates/runtime-bridge/src/run_insight.rs:32`).

**P0-3 — one approval covers the tool's name for the whole run.**

- The grant key of a name-gated tool is its name (`approval_key(name, false, _)`,
  `crates/agents-core/src/tools.rs:131`). On resume the bridge validates each granted tool
  (`validate_approved_tools`, `lib.rs:571`: distinct resolver, well-formed key) and REPLACES
  `__approvedTools` with the validated names/keys (`lib.rs:493`). The agent node reads it
  (`node.rs:301`) and the gate lets every call whose key is in it run.
- So once `refund` is granted, the agent may call `refund` again with any arguments, any number
  of times, in that node execution and in every later one until the next resume replaces the
  channel. The product gives back the approvals of the current wait only
  (`approvedToolsFromState` reads `__approvalIds`, cleared when the run waits on something else —
  `filing_plan`).
- `__approvedTools` is engine-owned (`ENGINE_OWNED_CHANNELS`,
  `crates/graph-runtime/src/runtime.rs:68`): no node — the agent node included — can write it.
- A resumed agent node runs again from its first LLM call (ADR 0049 D3): calls it executed in a
  previous attempt are issued again. With a name grant they run again silently; this matters for
  D5/D6 below.
- The TypeScript fallback agent (`packages/agents-core/src/react-agent.ts:226`) grants by name and
  already refuses a name grant for a conditioned tool.

## Decision

### D1 — Every gate files the call it stops (P0-2) — implemented

Every approval request the engine opens for a tool call carries:

- `input` — the call's arguments, as the model wrote them;
- `callKey` — the call's identity, `"<name>#<sha256(canonical input)>"`, computed by the existing
  `approval_key(name, true, input)`: object keys sorted recursively, compact JSON, SHA-256 hex.
  Opaque to a host: it is filed, stored, signed and compared, never recomputed (numbers are
  written as `serde_json` writes them — an integer `40` as `40`, a float as `600.5`).

On `ApprovalRequestItem` (agent result, `pendingApprovals`) and on the catalog `ApprovalSubject`
the host's approval engine stores. **The grant does not change**: a gate by name still files no
`approvalKey` and is still unlocked by the name. Where the grant is the call (content-scoped,
conditioned), `callKey` equals `approvalKey`.

Why a new field rather than filling `approvalKey` for a name gate: `approvalKey` means « the
grant to give back ». The product already gives it back whenever a subject carries one
(`callKeyOfSubject`); filling it for a name gate would make the product send a call key that the
name gate does not look for — every resumed run would gate again. `callKey` is information;
`approvalKey` stays a contract.

### D2 — An attestation can sign the call (P0-2) — implemented, opt-in

`AttestationView` gains an optional `callKey`. An attestor built with `{ signCallKey: true }`
(TypeScript `new Ed25519Attestor(keys, { signCallKey: true })`, Rust
`Ed25519Attestor::signing_call_keys()`) copies the subject's `callKey` into the view it hashes and
signs. Verification (both languages) includes a record's `callKey` whenever the record has one, so
changing or dropping it breaks the record.

Off by default, because a host that stores records column by column — the product's
`attestations` and `approval_signatures` tables have no call key — would keep records it can no
longer verify. A record without a call key is unchanged byte for byte (the first four records of
the TypeScript golden fixture are identical after regeneration; the fifth, with a call key, is
signed by TypeScript and reproduced by Rust).

### D3 — A replay must request the same call (P0-2) — implemented

`verify_replay_decisions` compares a decision's `callKey` when **both** sides carry one: the
attestation's (D2) and the replayed pending approval's (D1). A side without one (evidence
attested before ADR 0051) is compared by `{ status, subject }` as before, so existing evidence
keeps its verdict.

### D4 — An agent may grant per call: `approvalScope: "call"` (P0-3) — follow-up branch

An agent carries `approvalScope: "tool" | "call"` (default `"tool"`: byte-for-byte today's
behaviour). In the agent spec and the catalog carrier (`approvalScope`), TypeScript
`agentNode({ approvalScope: "call" })`, Python `Agent(approval_scope="call")`.

With `"call"`, every approval-gated tool of that agent is granted per call, as ADR 0046 grants a
conditioned call: the grant key is the call key, the request files it as `approvalKey`, a grant by
name unlocks nothing, and a call with other arguments opens a new gate. Conditions (`approvalWhen`)
still decide *whether* a call needs a gate. The TypeScript fallback agent, which cannot pin a grant
to a call, refuses a name grant for such an agent's tools, as it does for a conditioned tool.

Versioned by data, not by engine version: the scope is in the graph (the agent carrier), so a
replay of a run re-derives exactly its gates, and the product migrates agent by agent.

### D5 — A per-call grant is spent by its call (P0-3) — follow-up branch

Within one execution of the agent node, a per-call grant unlocks **one** execution of its call:
once the call has run, the same call again (same arguments) files a new request. Across node
executions the grant lasts until the next resume replaces `__approvedTools` — what the bridge
already does, with what the host gives back for the current wait.

Residual, stated plainly: between two resumes, a *later* execution of an agent node (a graph loop
that revisits it, or another agent with the same tool) that issues the identical call again would
find the grant still in the channel. Closing it needs a node to record a spent grant durably,
which `ENGINE_OWNED_CHANNELS` forbids today — see D6.

### D6 — A gated call that ran is answered from the run's ledger — proposal only, needs a decision

A resumed agent runs again from its first LLM call (ADR 0049 D3), so it issues again the gated
calls it already executed in the previous attempt. Today, under a name grant, they **run again**
(an agent that refunds A, then waits on B, refunds A a second time on the resume that grants B).
Under D4, they **gate again** (the signer is asked about A once more) — visible, but a signer who
signs it refunds A twice, and an agent making N calls loops through N re-asks.

Proposal: an engine-owned, checkpointed ledger `__gatedCalls: { <callKey>: <observation> }`,
written by the agent node in the same patch as its result (one checkpoint, ADR 0049), through a
privileged write the runtime grants agent nodes for this one channel. On a re-run, a gated call
whose key is in the ledger is answered with its recorded observation — neither executed nor gated
again — and the same key, signed again later, is a new call only if the model issues it again
after the ledger answer. This also makes D5 durable (a spent grant is the ledger entry) and
removes the residual. It changes who may write an engine-owned channel and what « at least once »
means for a gated tool — a runtime invariant: **not implemented here, for the owner to decide**
(and, if accepted, its own ADR or a revision of this one).

## Consequences

- **Compatibility.** D1 and D3 are additive; a host that ignores `input`/`callKey` behaves as in
  2.6.0. D2 is opt-in. D4 is opt-in per agent. No wire field changes meaning.
- **Determinism and replay.** `callKey` is a pure function of the tool name and the call's
  arguments; the arguments come from the model output, which a replay re-serves from the journal
  (ADR 0038). A replay therefore requests the same `callKey`s in the same order, and D3 checks it.
  `suspension_key` (what decides whether a resume waits on something new) still reads the
  `subject` string only, so filing and clearing of `__approvalIds` is unchanged. D4's scope is in
  the graph, so a replay gates where the run did.
- **Checkpoints and events.** Unchanged: the call is carried in the same agent result, written in
  the same interrupt patch, one checkpoint; no new event.
- **Data protection.** The arguments are now in the checkpointed agent result and the host's
  approval store for every gated call (they were for content-scoped and conditioned calls
  already, and always in the model journal). A host masks them by its PII policy before showing
  or exporting them; an argument that must never be stored needs the channel's `noLog` (ADR 0032)
  or a redaction middleware.
- **Verifiers.** A verifier older than this change ignores `callKey` and therefore fails a record
  signed with D2: whoever verifies (the product, an auditor with `@ailu-ai/verify`) must be on the
  release that ships D2 before records are signed with it.

## What the product changes to consume it

1. Bump to the release that ships D1–D3. Nothing breaks: `approvedToolsFromState` keeps giving
   back what the subject's `approvalKey` holds (none for a name gate).
2. **Show the call** — read `subject.input` and `subject.callKey` from the stored approval (they
   are now there for every gated tool), mask the input with the PII policy (as `GET
   /runs/:id/approvals` does for conditioned calls), and show « L'appel que vous signez » for
   every tool gate, never « Aucun paramètre structuré ».
3. **Sign the call** — add `call_key` (nullable) to `attestations` and `approval_signatures`
   (schema change: Mathieu's review), persist `record.callKey`, rebuild records with it, then build
   the attestor with `{ signCallKey: true }`.
4. **Replay** — pass `callKey` on both sides of `verifyReplayDecisions` (attested records, and
   `pendingApprovals` from the replay) so a replay of another call is a mismatch.
5. **One signature, one call (after D4)** — set `approvalScope: "call"` on the carriers of agents
   whose gated tools move money or data (refund, payout, connector writes). Its existing
   `callKeyOfSubject` gives the key back on resume. Until D6 is decided, prefer it for agents that
   make one gated call per execution, and show the signer when a request repeats a call already
   approved in the run.

## Not decided here

- D6, the ledger of executed gated calls.
- A per-call scope set by policy at run time rather than in the graph.
- Showing the signer which rule opened the gate (the approval does not keep it).
