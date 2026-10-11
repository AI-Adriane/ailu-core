# ADR 0051 — The signer sees and signs the call, and one signature is one call

- Status: **Accepted** — 2026-10-10, by the owner, after the architecture review (« VALIDÉ SOUS
  RÉSERVE » for D1–D5, D6 refused as proposed), with the owner's five decisions of that day — see
  Revision 1. Per decision:
  - **D1–D3: Accepted**, as revised below; they ship in **2.7.0**.
  - **D4–D5: Accepted**; merged into `main` after 2.7.0 is cut, **released in 2.8.0** with ADR
    [0053](./0053-an-agent-resumes-its-loop-at-the-signed-call.md), never adopted by the product
    before it (owner's decision 3; architect R9).
  - **D6: Superseded** by ADR 0053 (an agent resumes its loop at the signed call). The
    `__gatedCalls` ledger is abandoned.
- Date: 2026-10-10
- Deciders: Mathieu (owner)
- Driven by the product's beta QA of 2026-10-09 (product repo `docs/audits/2026-10-09-beta-qa`,
  §4.3 decision 3 and §8.3 « approbateur aveugle »), and the product's open points of 2026-10-03
  (« l'attestation Ed25519 signe `tool:refund`, pas l'empreinte de l'appel ») and 2026-10-09
  (« `__approvedTools` vaut pour le nom de l'outil tout le run »). Mathieu chose on 2026-10-10 to
  change the engine through pull requests he reviews before any release.
- Relates to: [0024](./0024-governed-virtual-filesystem-seam.md) (content-scoped grants, the
  canonical call hash), [0025](./0025-unified-agent-middleware-api.md) (the gate, intrinsic to the
  stack), [0038](./0038-replay-as-evidence.md) / [0040](./0040-verify-replay-control-plane.md)
  (verify-replay), [0045](./0045-host-nodes-in-rust-and-sdk-parity.md) (the engine decides a
  catalog run's approvals), [0046](./0046-an-approval-above-a-threshold.md) /
  [0048](./0048-an-approval-for-named-values.md) (a grant that is the call),
  [0049](./0049-the-host-keeps-each-checkpoint.md) (D3 a node is re-run; D4 the effect key, revised
  by 0053), [0053](./0053-an-agent-resumes-its-loop-at-the-signed-call.md) (replaces D6).

## Revision 1 (2026-10-10) — after the architecture review, with the owner's decisions

The owner accepted the review and decided, the same day:

1. The engine files the hashed canonical string (`callInput`), recomputes `callKey` when it files,
   and exports `callKeyOf` in TypeScript and Python with shared reference vectors. The form is
   frozen before any first D2 signature. The effective grant enters the signed view.
2. D6 becomes ADR 0053 (the agent's continuation, with ADR 0049 D4's effect key in the same
   train). The `__gatedCalls` ledger is abandoned.
3. `approvalScope: "call"` (D4) merges into `main` after the R1 fix; neither released nor adopted
   by the product before ADR 0053.
4. D5 applies to every key grant (conditioned, ADR 0046/0048; guarded write, ADR 0024), under the
   `"tool"` scope too.
5. An argument masked by the PII policy is announced to the signer (« 1 champ masqué »). The
   engine checks coherence when it files (R4); the canonical input stays encrypted or in
   restricted access.

| Review | Change                                                                                                      | Where              |
| ------ | ----------------------------------------------------------------------------------------------------------- | ------------------ |
| R1     | A wait on a call its resume granted is filed again (separate fix, before D4)                                | Consequences       |
| R2     | A middleware may not rewrite the input of a gated call: the gate decides on the input that runs             | D1                 |
| R3     | `callInput` filed as text; the canonical form written down; `callKeyOf` exported; shared vectors            | D1 (decision 1)    |
| R4     | The engine recomputes `callKey` (and a call grant's `approvalKey`) when it files, and refuses a mismatch    | D1 (decision 5)    |
| R5     | The TypeScript fallback agent files `input`; it has no `callKey`, and says so                               | D1                 |
| R6     | The effective grant (`grant`) is signed next to `callKey`                                                   | D2 (decision 1)    |
| R7     | A replay without a `callKey` against a record that has one is a mismatch                                    | D3                 |
| R8     | `@ailu-ai/verify` compares `callKey`, and reads the attested side from the verified records                 | D3                 |
| R9     | A test documents that D4 does not converge without ADR 0053                                                 | D4                 |
| R10    | `approvalScope: "call"` where no call can be pinned is a typed error, not a tool that never runs            | D4                 |
| R11    | D5's scope: every key grant (decision 4), released with ADR 0053                                           | D5                 |
| —      | Personal data: `callKey` is an unsalted hash, `callInput` the arguments themselves                          | Consequences       |
| §3.6   | Under a name grant, an earlier call is asked again or run again depending on what the host gives back       | D6 (superseded)    |
| §3.9   | `callKey` (what is called) and the effect key (which call, ADR 0049 D4 as revised by 0053) told apart       | D1                 |

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

### D1 — Every gate files the call it stops (P0-2)

Every approval request the engine opens for a tool call carries:

- `input` — the call's arguments, as the engine parsed them from the model's output;
- `callInput` — **the exact text that is hashed**: the canonical JSON of `input` (below), as a
  string;
- `callKey` — the call's identity, `"<name>#" + hex(sha256(callInput))`.

On `ApprovalRequestItem` (agent result, `pendingApprovals`) and on the catalog `ApprovalSubject`
the host's approval engine stores. **The grant does not change**: a gate by name still files no
`approvalKey` and is still unlocked by the name. Where the grant is the call (content-scoped,
conditioned, or `approvalScope: "call"`), `approvalKey` equals `callKey`.

Why a new field rather than filling `approvalKey` for a name gate: `approvalKey` means « the
grant to give back ». The product already gives it back whenever a subject carries one
(`callKeyOfSubject`); filling it for a name gate would make the product send a call key that the
name gate does not look for — every resumed run would gate again. `callKey` is information;
`approvalKey` stays a contract.

**The canonical form, frozen (owner's decision 1).** It is what ADR 0024 and ADR 0046/0048 grants
already hash, so no key filed or granted before 2.7 changes:

- the arguments as the engine parsed them (`serde_json`, without `arbitrary_precision`);
- every object's keys sorted by their UTF-8 bytes, at every depth; arrays keep their order;
- compact: no whitespace;
- strings as `serde_json` writes them: `"` and `\` escaped, `\b \f \n \r \t`, any other character
  below U+0020 as `\u00XX` (lower-case hex), every other character as raw UTF-8 (`/` and non-ASCII
  are not escaped);
- numbers: an integer the parser read as a 64-bit integer (signed or unsigned) in decimal, exactly
  (`9007199254740993` stays itself); any other number as the shortest decimal that reads back to
  the same IEEE-754 double, with `.0` for an integral value, and an exponent — with its sign —
  for a large or small magnitude, as `serde_json` writes it (`40.0`, `-0.0`, `123.456`, `1e+20`,
  `1e+21`, `1.5e-7`; an integer beyond 64 bits is a double: `1.8446744073709552e+19`). The
  reference vectors below are the authority on this form. The model's `40.00` and `40.0` are both `40.0`;
  `40` stays `40` and is another call.

So a host checks a call without reimplementing the form: `callKey == name + "#" +
sha256(callInput)`, over the bytes of the string it stored. It stores `callInput` as **text**,
never as `jsonb` (a `JSON.parse` turns `40.0` into `40` and rounds an integer above 2^53), and
shows the signer the arguments parsed from `callInput` with a parser that keeps large integers
exact.

**`callKeyOf`, one implementation.** The engine exports the function it uses: TypeScript
`callKeyOf(name, inputJson)` and `callInputOf(inputJson)` from `@ailu-ai/graph-sdk` (napi), Python
`ailu.call_key_of(name, input_json)` and `ailu.call_input_of(input_json)`. They take the arguments'
JSON **text** — a JavaScript or Python value has already lost what tells `40` from `40.0`.
`callKeyOf(name, callInput)` returns `callKey`: the form is idempotent. Reference vectors (`40`,
`40.0`, `40.00`, `-0.0`, `1e20`, `1e21`, `2^53 + 1`, `2^64`, keys out of order at every depth, non-ASCII keys, an
astral key next to U+FFFF — UTF-8 and UTF-16 order them differently —, escapes, nested arrays, an
empty object, a non-object input) live in one file, `crates/agents-core/tests/fixtures/
call_key_vectors.json`, checked by Rust, TypeScript and Python. ADR 0053 adds `effectKeyOf`'s to
the same file.

**The engine recomputes, it never copies (R4, owner's decision 5).** The agent's output channel is
not engine-owned. When it files (`filing_plan`) and when it lists `pendingApprovals`, the engine
recomputes `callInput` and `callKey` from the request's `input`, and the grant key of a call
grant. A request whose `callInput`, `callKey` or call-grant `approvalKey` does not match its
`input` is not filed: the filing plan carries a `refusal` (`an approval request does not match the
call it files (ADR 0051 D1)`), which the host handles as every refusal (ADR 0045 rev. 1 R6: the
run fails with its reason). A signer can never see X while the key unlocks Y. A request without
`input` (filed before 2.7) is filed as before.

**The gate decides on the input that runs (R2).** For a call that needed a gate and holds its
grant, a middleware that answers `Allow { input_override }` is refused (`ToolControl::Deny`,
fail-closed): what is signed is what runs. A call that needed no gate may still be rewritten.

**`callKey` is what is called, not which call.** Two identical calls (two refunds of 40 € on one
order) have one `callKey`. Which occurrence is the effect key of ADR 0049 D4, as revised by ADR
0053 D3; this ADR does not need it.

**The TypeScript fallback agent (R5).** The standalone `ReActAgent` of `@ailu-ai/agents-core`
files `input` for every gate. It files no `callInput` or `callKey`: it has no access to the
engine's canonical form, and the catalog path, where hosts file and sign, always runs on the Rust
engine.

### D2 — An attestation can sign the call and what the signature unlocks (P0-2) — opt-in

`AttestationView` gains two optional fields, both signed when present:

- `callKey` — the subject's call key;
- `grant` — **the effective grant** (R6): what the resume gives back for this decision, the
  subject's `approvalKey` when it has one, else the tool's name. An auditor reads `grant ==
  callKey` as « this signature unlocked this call only », and `grant == "<name>"` as « it unlocked
  the tool for the execution »: under `approvalScope: "tool"` the model may then issue another call
  of that tool, which ran on the same signature.

An attestor built with `{ signCallKey: true }` (TypeScript `new Ed25519Attestor(keys, {
signCallKey: true })`, Rust `Ed25519Attestor::signing_call_keys()`) copies both into the view it
hashes and signs, for a request whose subject carries a `callKey` (a gate request carries
neither). Verification (both languages) includes a record's `callKey` and `grant` whenever the
record has them, so changing or dropping either breaks the record.

Off by default, because a host that stores records column by column — the product's
`attestations` and `approval_signatures` tables have no such columns — would keep records it can
no longer verify. A record without them is unchanged byte for byte (the earlier records of the
TypeScript golden fixture are identical after regeneration; the new one is signed by TypeScript
and reproduced by Rust). The form of `callKey` (D1) and of `grant` is frozen with this ADR: a
signature is final.

### D3 — A replay must request the same call (P0-2)

`verify_replay_decisions` compares a decision's `callKey`:

- both sides carry one: they must be equal;
- the attested side carries one and the replayed side does not: **a mismatch** (R7) — the replay
  ran on an engine older than the one that signed, and cannot show it is the same call;
- the attested side carries none (evidence attested before ADR 0051, or by a host that does not
  sign call keys): compared by `{ status, subject }`, as before.

`@ailu-ai/verify` (R8) applies it: it takes the attested decisions from
`capsule.attestation.records` — the records `verifyChain` has just verified — rather than from the
capsule's unsigned `replay.decisions.attested`, and passes `callKey` on both sides. Python's
`verify_replay_decisions` runs the same native function; its docstring says so.

**Journals recorded before this change still verify.** Their replay re-derives every call from
the journaled model output, so its pending approvals now carry `callKey`; their attested records
carry none, so they compare as before. A fixture recorded on 2.6.1 checks it.

### D4 — An agent may grant per call: `approvalScope: "call"` (P0-3) — released with ADR 0053

An agent carries `approvalScope: "tool" | "call"` (default `"tool"`: byte for byte the behaviour of
2.6). In the agent spec and the catalog carrier (`approvalScope`), TypeScript `agentNode({
approvalScope: "call" })`, Python `GraphBuilder.agent_node(approval_scope="call")` (typed
`Literal["tool", "call"]`).

With `"call"`, every approval-gated tool of that agent is granted per call, as ADR 0046 grants a
conditioned call: the grant key is the call key, the request files it as `approvalKey`, a grant by
name unlocks nothing, and a call with other arguments opens a new gate. Conditions
(`approvalWhen`) still decide *whether* a call needs a gate.

Where a call cannot be pinned, the scope is refused with a typed error rather than leaving a tool
that can never run (R10): the standalone TypeScript `ReActAgent` refuses `approvalScope: "call"`
when it is built, and so does a `mapAgents` sub-agent until ADR 0053 D6 ships, in both SDKs.

**Not before ADR 0053.** Without the agent's continuation, an agent that makes two gated calls in
one execution does not converge (R9): the host gives back the grants of the current wait, so a
resume that grants B asks for A again, and one that gives back A and B runs A twice. A test
documents it; the product adopts `"call"` only on the release that ships ADR 0053.

Versioned by data, not by engine version: the scope is in the graph (the agent carrier), so a
replay of a run re-derives exactly its gates, and the product migrates agent by agent.

### D5 — A key grant is spent by its call (P0-3) — released with ADR 0053

Within one execution of the agent node, a grant that is a call key unlocks **one** execution of
its call: once the call has run, the same call again (same arguments) files a new request. Across
node executions the grant lasts until the next resume replaces `__approvedTools`.

Scope (R11, owner's decision 4): every key grant — `approvalScope: "call"`, a conditioned call
(ADR 0046/0048), a guarded write (ADR 0024) — under either scope. A name grant is not spent.

It ships with ADR 0053's continuation and bound grants (0053-c, 0053-d) in 2.8.0, never in a 2.7.x:
without them, « A then A again » loops — the second signature gives back the same key, the
re-asked model runs the first A on it again, and the second A finds it spent (ADR 0053 Context,
item 3). With them, a grant is bound to its occurrence (`<callKey>@<effectKey>`), and D5's spent
set only covers a grant without an effect key.

### D6 — Superseded by ADR 0053

The proposal (an engine-owned `__gatedCalls` ledger written by agent nodes) is refused: it opened
an engine-owned channel to nodes, merged two identical legitimate calls, still asked the model
again, and left ungated effects uncovered. ADR 0053 replaces it: the runtime keeps the agent's
loop at the suspension, and a resume executes exactly the signed call.

For the record (review §3.6): today a resumed agent runs again from its first LLM call, and the
calls it made before the gate are **asked again** when the host gives back only the grants of the
current wait (the product), or **run again** when the host accumulates grants. Both happen,
depending on the host; ADR 0053 removes both.

## Consequences

- **Compatibility.** D1 and D3 are additive on the wire; a host that ignores `input`, `callInput`
  and `callKey` behaves as in 2.6. D3's one stricter case (R7) only fails a replay on an engine
  older than the signer. D2 is opt-in. D4 is opt-in per agent. No wire field changes meaning; no
  existing grant key changes (the canonical form is the one ADR 0024 introduced).
- **Filing.** The R1 fix (its own batch, before D4): a resume that suspends again on a call whose
  grant it held — the grant was spent (D5) — waits on a new request, so the plan clears the stash
  and files it. With D1, two waits on the same tool differ by the call each request holds.
- **Determinism and replay.** `callInput` and `callKey` are pure functions of the tool name and
  the parsed arguments, which a replay re-serves from the journal (ADR 0038): a replay requests
  the same keys in the same order, and D3 checks it. D4's scope is in the graph.
- **Checkpoints and events.** Unchanged: the call is carried in the same agent result, written in
  the same interrupt patch, one checkpoint; no new event.
- **Data protection.** The arguments of every gated call are now in the checkpointed agent result
  and in the host's approval store, twice (`input`, `callInput`). `callKey` is an **unsalted**
  SHA-256: for an input of low entropy (an amount and an order number, an e-mail address) the
  input is found by brute force, so `callKey` is personal data whenever the input is (capsule
  export, retention). An HMAC would break third-party verification, so none is used. A host masks
  `input` by its PII policy before showing or exporting it, tells the signer how many fields are
  masked (owner's decision 5), and keeps `callInput` encrypted or in restricted access. An
  argument that must never be stored needs the channel's `noLog` (ADR 0032) or a redaction
  middleware.
- **Tenants.** `callKey` holds no tenant or run: two tenants making the same call share a key. A
  host never looks a request up by `callKey` alone (product: `(tenant_id, run_id, call_key)`).
- **Verifiers.** A verifier older than D2 ignores `callKey` and `grant` and therefore fails a
  record signed with them: whoever verifies (the product, an auditor with `@ailu-ai/verify`) must
  be on the release that ships D2 before records are signed with it.

## What the product changes to consume it

1. Bump to 2.7.0. Nothing breaks: `approvedToolsFromState` keeps giving back what the subject's
   `approvalKey` holds (none for a name gate). Catch `ApprovalRefusedError` (also raised for a
   request that does not match its call, D1).
2. **Show the call** — read `subject.input`, `subject.callInput` and `subject.callKey` from the
   stored approval; keep `callInput` as text, encrypted or in restricted access; show the
   arguments parsed from `callInput` without losing large integers, masked by the PII policy,
   with « N champ(s) masqué(s) »; make invisible, bidirectional and confusable characters visible.
   Under `"tool"` the label is « l'appel qui a ouvert la demande »; « l'appel que vous signez »
   only when the grant is the call (`approvalKey` present).
3. **Sign the call** — add `call_key` and `grant` (nullable) to `attestations` and
   `approval_signatures` (schema change: Mathieu's review), persist them, rebuild records with
   them, then build the attestor with `{ signCallKey: true }` — three commits, released in that
   order. Product ADR 0104 D6 (« le hash du sujet lu ») reuses `callKey`.
4. **Replay** — pass `callKey` on both sides of `verifyReplayDecisions` (attested records, and
   `pendingApprovals` from the replay).
5. **One signature, one call (2.8.0, with ADR 0053)** — set `approvalScope: "call"` on the
   carriers of agents whose gated tools move money or data. Index `(tenant_id, run_id, call_key)`
   with a cross-tenant isolation test before showing that a request repeats an approved call.

## Implementation (small batches, each its own pull request)

1. R1: a wait on a call its resume granted is filed again.
2. `callKeyOf` and `callInputOf`, the canonical form and its shared vectors (Rust, napi, Python).
3. D1: every gate files `input`, `callInput`, `callKey`; recomputed when filed, a mismatch refused
   (R4); the TypeScript fallback files `input` (R5).
4. R2: a gated call's input is not rewritten after the gate.
5. D2: `callKey` and `grant` in the signed view (Rust, TypeScript, golden fixture).
6. D3: R7, R8, the Python docstring, and the fixture of a journal recorded on 2.6.1.
7. D4 and D5 for `"call"` (the earlier branch `engine/approval-per-call`, reworked): R9's test,
   R10's typed refusals, Python's `Literal`. Merged after 2.7.0 is cut.
8. Decision 4: every key grant spent by its call, under both scopes — after ADR 0053's bound
   grants (0053-d), for 2.8.0.

## Not decided here

- A per-call scope set by policy at run time rather than in the graph.
- Showing the signer which rule opened the gate (the approval does not keep it).
