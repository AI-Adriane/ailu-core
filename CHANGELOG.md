# Changelog

All notable changes to the Ailu engine are documented here. The project follows
[Semantic Versioning](https://semver.org/).

## 2.7.4 — 2026-10-21

Engine fix, reached by every SDK through the same bridge: a replay builds no LLM provider gateway,
so the TypeScript SDK (`replayCatalogGraph`, hence `@ailu-ai/verify`) and the Python SDK
(`engine_replay`) both re-derive an agent's run without a provider key. No new field, no API
change. Cut from `release/2.7` (v2.7.3 + this change).

### Fixed — 2.7.4

- **`npx @ailu-ai/verify` verifies a run with an agent node on a machine with no provider key.**
  Building an agent's gateway checked the provider's credentials before looking at the run mode, so
  a replay without `ANTHROPIC_API_KEY` (or `AILU_LLM_MOCK=1`) failed with « no API key for provider
  'anthropic' » and a genuine capsule read NOT VERIFIED — although a replay's model answers come
  from the journal's `ReplayGateway` alone. In replay mode the bridge now hands the agent that
  gateway before any credential lookup: no LLM provider adapter is built and no call to the LLM
  provider is made. Live and recording runs check credentials exactly as before, and an unknown
  provider is still refused in replay. The verify tests no longer set `AILU_LLM_MOCK` for the whole
  suite: offline mode is on only while a fixture records, and the agent-node case is verified in an
  environment stripped of every `AILU_*` and `*_API_KEY` variable, as is a capsule signed by the
  issuer whose agent journal lacks a recorded LLM call (NOT VERIFIED: a journal gap, never a live
  call).

### Known limitation — 2.7.4

- An agent declared by **tier only** (no `provider`) still resolves its model from the environment
  (the provider keys present) when it is replayed, so a replay on a machine whose keys differ from
  the recording one asks for another model than the journal holds and misses. Agents that name
  their provider are unaffected. Tracked separately, pending the maintainer's decision.
- **A replay still calls the services the environment configures** — the external PII redactor
  (`AILU_PII_REDACTOR_URL`), the external secrets redactor (`AILU_SECRETS_REDACTOR_URL`), the
  LLMLingua compressor (`AILU_LLMLINGUA_URL`, for an agent whose middleware asks for `compress`) and
  the rerank endpoint (`AILU_RERANK_ENDPOINT`, for a `reranker` node). The guarantee of this release
  is « no call to the LLM provider », not « no network call ». They are not skipped in replay on
  purpose: redaction and compression rewrite the request in `before_model`, before the gateway, so
  the journal holds the rewritten request and the replay matches it by equality; a replay without
  them would rebuild another request and miss on every genuine capsule recorded with them. For an
  offline verification, run with none of these variables set (`env -i`): a capsule recorded without
  them verifies; one recorded with them needs the same services to rewrite the request the same way.
- **A journal without host-tool results replays tools as a stub.** When the journal carries no
  `toolResults` — absent or empty, which read the same (`ToolReplayOutcome::NoJournal`) — each tool call an agent makes in the replay answers
  `{ "tool": <name>, "ok": true }` instead of failing. This is a deliberate degradation for evidence
  recorded before ADR 0041, which journaled no tool result: the replay still re-derives the model's
  decisions (the tool requests and the gates, which the chain must match), but not the tools'
  outputs. A journal that holds tool results serves them; a call it holds no result for gets a
  `tool_input_mismatch` error, never the stub.

## 2.7.3 — 2026-10-20

Every SDK in the same release: `replayedDecisions` reads the Rust engine's filing plan, which the
TypeScript SDK and the Python SDK (`ailu.replayed_decisions`) both call. No engine change, no new
field; the SDK functions that existed behave as before. The one behaviour change is
`@ailu-ai/verify`'s replay check, which now verifies capsules of runs with a human gate. Cut from
`release/2.7` (v2.7.2 + this change).

### Added — 2.7.3

- **`replayedDecisions(replay, graph, subgraphs?)`** (TypeScript, from the package root) and
  **`ailu.replayed_decisions(replay, graph, subgraphs=None)`** (Python): the decisions a replay
  re-derives, the replayed side of `verifyReplayDecisions`. A replay files nothing, so its
  `pendingApprovals` never name the human gate it stopped at, and a replay of any run with a human
  gate reproduced one decision fewer than its chain attests. The function reads the engine's own
  filing plan (`filing_plan`, ADR 0045 D3.1) for the state the replay stopped in: an agent's tool
  requests, each with its `callKey` (ADR 0051 D3), then the human gate (`gate:<node id>`), then
  those of a direct child run suspended inside a subgraph node. `status` is blank; nothing for a
  replay that did not suspend. One rule for every host: a control plane and `@ailu-ai/verify` no
  longer each keep their own. No engine change.
- **`attestedDecisions(records)`** (TypeScript): the attested side of `verifyReplayDecisions` —
  one decision per request, its last link, in chain order, read from signed fields only. Links are
  the same request when `runId`, `approvalId`, `subject`, `callKey` and `grant` all match: a
  request signed by a quorum has one link per signer and is one decision, as the replay re-derives
  it once; a link of the same id that signs another subject or call stays a decision of its own. `@ailu-ai/verify` uses it, so a capsule of a gate signed by two members
  verifies. TypeScript only: the Python SDK has no attestation chain.

### Fixed — 2.7.3

- **`@ailu-ai/verify` verifies a run that stops at a human gate.** The replay check built its
  replayed side from `pendingApprovals` alone, so every capsule of a run with a human gate (a
  draft, an action) read NOT VERIFIED — one attested decision (`gate:<node>`), none replayed —
  while the control plane said the run was replay-verified. It now uses `replayedDecisions`: the
  gate the replay stops at counts, and only that one. A chain that signs another gate, or a
  capsule altered by one byte, still fails. Capsules already exported verify as they are: nothing
  to re-export.
- **`@ailu-ai/verify` replays a run from where it started (G1).** The replay is refused, and the
  capsule NOT VERIFIED, when `reproduction.entryState.currentNodeId` is not the graph's
  `entryNodeId`: an entry state moved onto a gate would skip the path to it and still reach the
  attested gate. The engine's entry state always starts at the entry node, so genuine capsules are
  unaffected.
- **`@ailu-ai/verify` never reads VERIFIED without a pinned key.** Without `--key` (or
  `expectedPublicKey`), a capsule is checked against its own embedded key, which a forger chooses:
  one who signs the bundle and the chain with their own key passed every check and read VERIFIED.
  Such a capsule now reads **INTACT — origin not proven**. Behaviour change for callers:
  `verifyCapsule` returns a new `verdict` field (`"verified" | "intact" | "not-verified"`), and
  `ok` is now `verdict === "verified"` — `false` for an unpinned capsule that used to give `true`.
  The CLI exits `3` for INTACT (`0` VERIFIED, `1` NOT VERIFIED, `2` usage error; `--key` without
  a value is now a usage error).
- **`@ailu-ai/verify` fails signed decisions it cannot re-derive.** A capsule whose attestation
  chain holds records but whose `reproduction` is absent or `null` skipped the replay leg and could
  read VERIFIED; it now reads NOT VERIFIED (« replay missing »). A capsule with no record and no
  reproduction (a run with no governed decision, not recorded) reads INTACT: the signature and the
  pinned key hold, but nothing was re-derived.
- **`@ailu-ai/verify` trusts a chain only under the anchoring key (G2).** Every attestation record
  must be signed by the pinned key (`--key`), else by the key that signs the bundle; `chain` fails
  otherwise. Before, each record was checked against the key it carries, which a forger chooses.
  The control plane signs its chain and its bundles with the same key, so its capsules are
  unaffected.

## 2.7.2 — 2026-10-17

Every SDK in the same release: the compression changes live in the Rust engine's runtime bridge
and LLM gateway, which the TypeScript SDK, the Python SDK and the C ABI all run; the new knobs are
environment variables read by the engine (`AILU_LLMLINGUA_TOKEN`, `AILU_LLMLINGUA_TIMEOUT_MS`,
`AILU_LLMLINGUA_BODY`). No new SDK type, no new field; the new Rust items are exported from
`ailu-llm-gateway` only. Backport of #370 to `release/2.7`; the same changes reach `main` for 2.8.0
through #370.

### Added — 2.7.2

- **The compression service takes a token and a deadline.** `AILU_LLMLINGUA_TOKEN` is sent as
  `Authorization: Bearer <token>` (never logged), as `AILU_PII_REDACTOR_TOKEN` is.
  `AILU_LLMLINGUA_TIMEOUT_MS` (default 2000) bounds each compression call, independently of
  `AILU_HTTP_READ_TIMEOUT_SECS` (600 s, sized for completions). Past it, or on any other failure,
  the message keeps its original text and stderr gets one line with a reason code (`timeout`,
  `transport`, `status`, `decode`) and never the text. Rust: `HttpPromptCompressor::new(url,
  token)` with `with_rate`, `with_min_chars`, `with_timeout`; `compress_timeout_from`,
  `DEFAULT_COMPRESS_TIMEOUT` and `CompressFallback` are exported from `ailu-llm-gateway`.
- **The compression client speaks the Hugging Face Inference Endpoint shape.**
  `AILU_LLMLINGUA_BODY=inputs` sends `{ "inputs": <text>, "parameters": { "rate": <rate> } }`
  instead of `{ "text": <text>, "rate": <rate> }`. The inference toolkit refuses any body without
  `inputs` and writes the refused body, prompt text included, to its logs (kept 30 days), so the
  default shape must never reach such an endpoint. `text` (the default, unchanged) and `inputs`
  are the only values: any other one installs no compressor, rather than send a shape the
  endpoint could refuse and log. The answer stays `{ "compressed": string }`. Rust:
  `CompressBody` and `HttpPromptCompressor::with_body`.

### Changed (behaviour) — 2.7.2

- **Compression is opt-in: `AILU_LLMLINGUA_URL` alone compresses nothing.** An agent whose
  resolved middleware list was empty got the `compress` middleware as soon as
  `AILU_LLMLINGUA_URL` was set, through the pre-phase-3d back-compat path. The TypeScript SDK
  resolves an empty list for every agent without a profile, `outputStyle`/`contextBudget` knob or
  `middleware` — so setting the URL on a worker compressed every such agent of every tenant,
  whatever its policy. Now only an agent whose resolved list names `compress` (an explicit
  `{ kind: "compress" }`, or the `fast` / `governed-deep` profile) is compressed; the URL only
  says where the service is. The other back-compat knobs (`outputStyle: "terse"`,
  `contextBudget`) are unchanged: each applies only when its own knob is set. A graph persisted
  before phase 3d that counted on the URL to compress its agents must now add
  `{ kind: "compress" }` to their `resolvedMiddleware`.
- **Compression keeps a message whose PII placeholders it would break.** When the compressed text
  of a `user` message does not carry exactly the `[ENTITY_n]` placeholders of the original (one
  dropped, cut, renumbered or added), the message keeps its original text and stderr gets
  `reason=placeholder_integrity`. The check (`placeholders_preserved`, pure) runs in the
  `compress` middleware and in `CompressingGateway`, whatever `PromptCompressor` they wrap
  (`compress_preserving_placeholders`).
- **The PII redaction call names its run.** The body `HttpPiiRedactor` posts to
  `AILU_PII_REDACTOR_URL` is now `{ "texts": [...], "runId": <run id> }` when the LLM request
  carries a `run_id` (a child run sends its own `{parent}:{node}` id), so a control plane can apply
  that run's tenant policy and share the redaction service fairly between tenants. Without a run
  id, or with one that is empty or longer than 256 UTF-16 units (the control plane's limit), the
  body stays `{ "texts": [...] }`. The answer, the `AILU_PII_REDACTOR_TOKEN` bearer and the
  fail-open / fail-closed behaviour are unchanged; neither the texts nor the run id are logged.

## 2.7.1 — 2026-10-15

Every SDK in the same release: F1 is in the Rust engine's resume check, which the TypeScript SDK,
the Python SDK and the C ABI all call before a governed resume runs anything. No new type, no new
field. A host that gives back only the grants of the run's stashed, approved requests (as Ailu's
control plane does) sees no change.

### Changed (behaviour) — 2.7.1

- **A resume validates supplied grants against filed approvals** (ADR 0045 Revision 2, F1).
  `resume_problems` now checks every grant in `approvedTools` against the approval requests the
  run filed, in every case: a grant must answer an approved request among the ids the run stashed
  in `__approvalIds` (subject `tool:<name>`, the same `resolvedBy`, the same key when the grant
  has one), as it already did when ids were stashed. Two cases are new:

  - a suspended run that stashed no id: each grant is refused with `tool '<name>' is granted, but
    the run filed no approval request it could answer` (after « the run waits on an approval that
    was never recorded… » when it waits on one);
  - a run that is not suspended (a running checkpoint, a cancelled run): it waits on no approval,
    so each grant is refused with `tool '<name>' is granted, but the run is not suspended: it
    waits on no approval`. A resume of such a state without grants goes on as before.

  `resumeCatalogGraph` throws `ApprovalNotGrantedError` (code `AILU_APPROVAL_NOT_GRANTED`) and
  `resume_catalog_graph` raises `ailu.ApprovalNotGrantedError`, with those `problems`, before the
  engine runs anything. A host that gives back only the grants of the run's stashed, approved
  requests sees no change. Without an `approvalEngine` nothing changes: only the no-self-approval
  check applies. Three golden cases are added. Ships in 2.7.1 from `release/2.7`.

## 2.7.0 — 2026-10-15

Every SDK in the same release. ADR 0051 D1 and D3, the refusal of a rejected tool gate (ADR 0053
D4), the rollback filter (K6) and the `apiKeyEnv` allow-list are in the Rust engine and reach the
TypeScript SDK, the Python SDK and the C ABI alike. ADR 0051 D2 (`signCallKey`) is an option of
the attestor, which lives in `@ailu-ai/approval-engine` and in Rust: the Python SDK has no
attestor (it never signed decisions), so it has nothing to switch on; it verifies replays with
call keys (`verify_replay_decisions`). ADR 0051 D4–D5 (`approvalScope: "call"`), ADR 0049 D2
and D4, and the rest of ADR 0053 are not in 2.7.0.

### Added — 2.7.0

- **A run left by a 2.8 engine can be resumed by 2.7 without leaking its agent's transcript**
  (ADR 0053 « Rollback », K6). 2.8 will keep a suspended agent's loop — its conversation and the
  call it stopped at — in the engine-owned channel `__agentResume`. 2.7 never writes or reads it,
  but now hides it the way it hides `__mapResults`: no node handler (so no agent seed and no host
  node payload), no named condition, no child run and no event receives it, at the top level or in
  any child state under `__subgraphStates`, and `explain_run` neither lists nor shows it. A run's
  input and a node's update cannot write it (Rust `AGENT_RESUME_KEY`, exported). The state keeps
  what it found. A run rolled back from 2.8 restarts its agent, as in 2.7; roll back to 2.7.0 or
  later, never below. Nothing changes for a run that never met 2.8.
- **An attestation can sign the call and what the signature unlocks** (ADR 0051 D2).
  `new Ed25519Attestor(keys, { signCallKey: true })` (type `AttestorOptions`; Rust
  `Ed25519Attestor::signing_call_keys()`) writes, for a request whose subject carries a `callKey`,
  that call key and the effective grant into the record, and signs them: `grant` is the subject's
  `approvalKey` when the grant is the call, else the tool's name. Off by default: a host turns it
  on once it stores both fields. `verifyAttestation` / `verifyChain` check them whenever a record
  has them; a record without them hashes and verifies byte for byte as before (the golden
  fixture's earlier records are unchanged).

- **A gated call files what the signer approves** (ADR 0051 D1). Every approval request the
  engine opens for a tool call carries the call's `input` (its arguments), `callInput` (their
  canonical text, the bytes hashed — store it as text, not `jsonb`) and `callKey`
  (`<name>#<sha256(callInput)>`): on the agent result's `approvalRequests`, on
  `pendingApprovals`, and on the subject a catalog run files in its approval engine. The engine
  computes `callInput` and `callKey` from the input when it files and when it lists pending
  approvals; it never copies them from the agent's output channel. A gate decided by the tool's
  name is still granted by the name and files no `approvalKey`. The TypeScript `ReActAgent` (the
  legacy runtime) files `input`, and no `callKey`.

- **`callKeyOf` and `callInputOf`: the canonical form of a call** (ADR 0051 D1).
  `callKeyOf(name, inputJson)` returns a call's identity, `<name>#` + hex(sha256(`callInputOf(
  inputJson)`)), and `callInputOf(inputJson)` the canonical text of its arguments, the exact bytes
  hashed: keys sorted by their UTF-8 bytes at every depth, compact, numbers as the engine parsed
  them (`40.0` stays `40.0`, `9007199254740993` stays exact). Both take the arguments' JSON
  **text**. TypeScript from `@ailu-ai/graph-sdk` (napi), Python `ailu.call_key_of` /
  `ailu.call_input_of`, Rust `ailu_agents_core::{call_key_of, call_input_of}`: one
  implementation. It is the form every content-scoped and conditioned grant already hashes, so no
  key changes. Shared reference vectors (`crates/agents-core/tests/fixtures/call_key_vectors.json`)
  are checked in the three languages.

- **`AILU_API_KEY_ENV_ALLOWLIST`: the operator restricts which variables `apiKeyEnv` may name.**
  An agent pointed at a custom OpenAI-compatible endpoint names the variable that holds its key,
  and the engine reads it: in a host that runs graphs for many organisations, a graph could name
  any variable of the process. The new operator variable takes comma-separated exact names and
  prefixes ending in `*` (`AILU_ENDPOINT_*,GATEWAY_KEY`; entries trimmed, blank ones ignored,
  only a trailing `*` is a wildcard). When it is set, even to an empty string (which allows
  none), a name that matches no entry is refused before the variable is read, whether or not it
  exists, with an error that names the variable and the allow-list, never a value. The same rule
  applies wherever an `apiKeyEnv` becomes a value:

  - the engine's graph path (`ailu-runtime-bridge`, new module `api_key_env`): the refusal is a
    message led by its code, `AILU_API_KEY_ENV_NOT_ALLOWED: …` (also what Python's
    `run_catalog_graph` and the C ABI report), and the TypeScript runner rethrows it as
    `ApiKeyEnvNotAllowedError` from `run()`, `resume()`, `approveAndResume()`, `signal()` and
    `replay()`;
  - `resolveProviderKeys` and `Model.invoke()` in `@ailu-ai/model-core`, which throw
    `ApiKeyEnvNotAllowedError` (code `AILU_API_KEY_ENV_NOT_ALLOWED`, `envVar`; exported with
    `API_KEY_ENV_ALLOWLIST_ENV`, and re-exported by `@ailu-ai/graph-sdk`);
  - Python's `llm_complete(api_key_env=...)`, which raises `ailu.ApiKeyEnvNotAllowedError` (a
    `RunError`, with `env_var`).

  One table of cases (`crates/runtime-bridge/tests/fixtures/api_key_env_cases.json`) is run by
  all three. A provider's own variable (`OPENAI_API_KEY`, …) is not affected. **Opt-in and
  non-breaking:** unset, every name is read as before.
- **The SDKs trim an `apiKeyEnv` name as the engine does.** `" GATEWAY_KEY "` now reads
  `GATEWAY_KEY` in `resolveProviderKeys` / `Model.invoke()` and in Python's `llm_complete`, and a
  blank name means no name: a keyless custom endpoint, or a named provider's default variable
  (it used to read a variable named `""` and fail).

### Changed (behaviour) — 2.7.0

- **A refused tool gate ends the run** (ADR 0053 D4). A rejected `tool:` request now blocks the
  resume, as a rejected `gate:` request always did. Until 2.6, a rejected tool « stayed locked »
  and the resume went on: the agent ran again without the tool, and could ask for it again. Now
  `resume_problems` returns, for each rejected request the run stashed (its own, and a direct
  child run's), exactly:

  `request <id> (tool:<name>) was rejected by <resolvedBy>` (`a reviewer` when the store names
  no resolver)

  so `resumeCatalogGraph` throws `ApprovalNotGrantedError` (code `AILU_APPROVAL_NOT_GRANTED`,
  `problems`), Python's `resume_catalog_graph` raises `ailu.ApprovalNotGrantedError`, and the C
  ABI's resume check reports it — before anything runs, with no LLM call. No « rejected »
  observation is ever fed to the model. The event vocabulary does not change (no `rejected`
  status, no `run_rejected` event): the host ends the run its way (a cancelled or failed run,
  with the refusal as its reason). A nested child and a `mapSubgraph` fan-out's children stay
  unwalked, as for a rejected gate. Without an `approvalEngine` nothing changes. **Migration:** a
  host that resumed after a tool refusal to let the agent finish without the tool must end the
  run instead; a host that already ends the run on a refusal (as Ailu's control plane does) sees
  no change.
- **A replay must request the call it was signed for** (ADR 0051 D3). `verifyReplayDecisions`
  (Python `verify_replay_decisions`) compares a decision's `callKey`: when the attested decision
  names its call, the replayed one must name the same, and a replay that names none (an engine
  older than the signer's) is a mismatch. A decision attested without a call key (evidence from
  before ADR 0051) is compared by status and subject, as before: a journal recorded on 2.6.1
  replays and verifies unchanged (fixture). `@ailu-ai/verify` takes the attested decisions from
  the capsule's signed `attestation.records` — the records `verifyChain` checks — instead of its
  unsigned `replay.decisions.attested`, and compares call keys.

- **A middleware may not rewrite the input of a gated call** (ADR 0051 D1, R2). For a call that
  needed an approval and holds its grant, an installed `before_tool` middleware that answers
  `Allow { input_override }` is refused (`Deny`): the call runs with the input its signer saw, or
  not at all, and the model reads `tool_error:denied:Tool '<name>' was approved for the input its
  signer saw; a middleware may not change it.` A call that needed no approval may still be
  rewritten. No built-in middleware rewrites an input.

- **A request that does not match its call is refused** (ADR 0051 D1, R4). An approval request
  whose `callKey`, `callInput` or call grant (`approvalKey`) is not the one its `input` hashes to,
  or that names a call key without its input, is not filed: the filing plan carries the refusal
  `an approval request does not match the call it files (ADR 0051 D1)`, and `runCatalogGraph` /
  `resumeCatalogGraph` throw `ApprovalRefusedError` (Python: `ailu.ApprovalRefusedError`), as for
  every refusal. The engine never writes such a request; only a changed agent channel holds one.

- **A tool approval in a child run is refused instead of looping** (ADR 0045 Revision 1, R6). A
  grant cannot reach a child run: the bridge writes `__approvedTools` into the top-level state
  only, and a child resumes from its own snapshot. So when an agent inside a subgraph asked for
  an approval-gated tool, its request was filed and could be approved, but on every resume the
  agent asked for the same call again: the wait looked the same, the approval stayed stashed, and
  the run looped, calling the model each time while the signer's « yes » never acted.

  With an `approvalEngine`, such a wait is now refused. The engine's filing plan files nothing and
  carries a `refusal`, which `resume_problems` returns first. `runCatalogGraph` and
  `resumeCatalogGraph` throw `ApprovalRefusedError` (code `AILU_APPROVAL_REFUSED`), and
  `run_catalog_graph` and `resume_catalog_graph` raise `ailu.ApprovalRefusedError`, with:

  - the message `Run '<runId>' cannot wait for an approval: a tool approval in a child run cannot
    be granted yet (ADR 0045 rev. 1, R6).` (Python: `run '<runId>' cannot wait for an approval: …`);
  - `reason`: `a tool approval in a child run cannot be granted yet (ADR 0045 rev. 1, R6)`;
  - `state`, the run's state as it stopped;
  - `outcome`, the run's outcome (`replayJournal`, `entryState`…) when the run had executed before
    it was refused, absent when a resume is refused before anything runs.

  Where the run used to return `suspended`, it now throws. **Migration:** catch
  `ApprovalRefusedError`, fail the run with `reason`, keep `outcome`'s journal as for any other
  run, and do not retry it. `state` and `outcome` hold the run's channels, personal data included:
  never log them (in TypeScript they are not enumerable, so `JSON.stringify(error)` leaves them
  out). The run's own gated calls, a child's human gates, and every other approval decision are
  unchanged; two golden cases change accordingly.

  Without an `approvalEngine` (`approveAndResume`), nothing changes, and the grant still does not
  reach the child: the same loop remains there until the follow-up revision that routes the grant
  into the child run.

### Fixed

- **A wait on a call its resume granted is filed again** (ADR 0051 review, R1). A resume that
  gives back a call's key (`approvedTools` with `key`) and then waits on that same call again — the
  call ran on the grant, and a key grant is spent by its call (ADR 0051 D5) — waits on a new
  request. The filing plan saw the same wait as before the resume and filed nothing, so the run
  waited on a decision already made and nobody saw the new request. It now clears the stash and
  files the request (`filing_plan`, so `runCatalogGraph` / `resumeCatalogGraph` and Python's
  `run_catalog_graph` / `resume_catalog_graph`). A wait on another call, a rejected call asked
  for again and a run re-driven without a resume are unchanged. One golden case is added.

- **The replay of a `mapSubgraph` node's resume re-attaches its items** (ADR 0045 Revision 1,
  PR 1b). `replay_from` forks a new run id (`<run>:fork:<n>`), and a fork's item runs are named
  after it (`<run>:fork:<n>:<node>:<index>`), while the state it forks from records them under the
  original's (`<run>:<node>:<index>`). So replaying the segment that resumed a map node (what
  verify does) found neither the items' snapshots nor their kept results: a finished item ran
  again, a waiting one asked its gate again, and the replay reported a divergence
  (`node_input_mismatch`) on a run that had not diverged. A map node now first moves each record
  of the same logical item (ADR 0043's `logical_run_id`, matched on the item's exact
  `:<node>:<index>`) under the fork's own item id. The replay then re-attaches as the recorded
  resume did. A fork adopts only what its own state records: from a checkpoint before the map
  node, its items run afresh under its own ids, never attached to the original's runs in the same
  runtime. A run that is not a fork, and every journal recorded before this release, run and replay
  as before. `logical_run_id` moves from `ailu-agents-core` to `ailu-graph-runtime` (exported), its
  one definition.
- **A `mapSubgraph` item that finished is not run again by the next resume** (ADR 0045
  Revision 1, R4). When one item finished while a sibling still waited (at a human gate, a timer,
  a signal), the node dropped the finished item's snapshot, and the resume that re-entered the
  node started that item again from its entry: its steps ran again, and its gate, if it had one,
  was asked again. On a fresh runtime per call (napi, PyO3, C ABI), two items could ping-pong for
  several resumes; one item's step ran three times in a probe. The finished item's join element
  is now kept in a new engine-owned channel, `__mapResults`, bound to the sha256 of the item, and
  written in the same checkpoint as the node's suspension. The resume reuses it and runs only the
  items still outstanding. A list that changed while the node waited fails the node, rather
  than pairing a result with another item. The channel is dropped when the node completes and
  when the run completes or fails; a cancelled run keeps it, since it can be resumed (ADR 0044).
  It cannot be written by a run's input or a node's update, and no node handler or named
  condition receives it, nor a child state that holds it (`handler_view`), so an agent's seed or
  a host condition callback never carries item results. A host that writes its own copy of a
  state outside the engine (a refusal checkpoint, an export) should drop `__mapResults` from it. A state
  kept before this release has no `__mapResults`: its finished items run once more, as before,
  and a journal recorded before it replays as recorded.
- **A second gated call of the same tool is filed** (ADR 0045 D3.1, ADR 0046; ADR 0051 review
  R1). An agent asked `refund(A)`, gated by its amount: A was filed, approved and ran; then it
  asked `refund(B)`. Both waits read `tool:refund` at the same node, so `filing_plan` took the
  second for the first: B was never filed, the id of A's request stayed stashed, and a later
  resume with A's grant passed the check and ran A again. What a wait is now includes the call
  each request holds (its grant key and input) and the requests of a child run, so B is filed
  and A's id dropped, and a resume is refused until B is decided. The same fix files a child run
  that moved on to its next gate inside the same subgraph node. A wait that holds the very same
  call (a rejected tool asked for again) is still the same wait, nothing filed twice. Nothing
  changes in a checkpoint, a request, the golden decisions or the bindings' surface; a state
  stashed before the fix resumes as before.
- **A human gate a loop comes back to needs a new decision** (ADR 0045 D3.1). A resume always
  passes the human gate it waited at; when a graph loop (draft → review → revise → review) brought
  the run back to the same gate, the second wait looked like the first, so nothing was filed and
  the first visit's approval stayed stashed: the next resume — a redelivered job, a « resume »
  click — passed the gate a second time without anyone deciding. A resume from a wait at a human
  gate (the run's own, or a direct child's) that waits at a human gate again is now a new wait:
  the stashed ids are dropped, the gate is filed again, and a resume is refused until a person
  decides that visit. Agent waits are unchanged (a rejected tool asked for again files nothing
  twice). Nothing changes in a checkpoint, a request, the golden decisions or the bindings'
  surface; a state stashed before the fix resumes as before.

## 2.6.1 — 2026-10-10

### Fixed

- **A context budget no longer cuts the user's request** (ADR 0052, ailu QA N3-1). Memory, the
  governed brain and skills are prepended to an agent's first message before the budget runs, and
  the budget kept the first `chars` characters: with enough context, the request was what went (an
  agent with two installed skills under a 12 000-character budget answered « request not found »).
  Over budget, the budget now always keeps the `Input` line and the `input` / `question` channels,
  then fills the rest: the other state entries (ordinary before `__*`), and the prepended context
  only when the whole state fits, kept from its end so skills are cut first. Deterministic; a
  message within budget is unchanged. The replay journal records the cut (`contextBudgetTrim`); a
  journal recorded before replays with the old cut, so its prompts match byte for byte. Rust:
  `BudgetTrim`, `trim_seed`, `ContextBudgetMiddleware::with_trim`; `ContextBudgetMiddleware::new`
  keeps the request.

  Every SDK in the same release: the fix is in the Rust engine and changes no SDK surface. The
  TypeScript and Python SDKs run it as they are, `contextBudget` keeps its shape on the wire, and
  `contextBudgetTrim` travels inside the replay journal, which both SDKs carry as an opaque string
  (`replayJournal` / `replay_journal`).

## 2.6.0 — 2026-10-08

### Added

- **The host keeps each checkpoint** (ADR 0049 D1). A host may install its own checkpoint store:
  every checkpoint the runtime writes (node completion, state mutation, suspension, cancellation,
  failure) is handed to it and awaited before the run goes on and before the event that follows it
  is emitted. A checkpoint the store cannot keep stops the run (`CheckpointSaveFailed`). TypeScript:
  `runCatalogGraph` / `resumeCatalogGraph` take `checkpointer?: Pick<Checkpointer, "save">`. Python:
  `checkpointer=` on `GraphRunner.run`, `resume`, `approve_and_resume`, `signal`,
  `run_catalog_graph` / `resume_catalog_graph` and the builder. C ABI: `AiluCallbacksV3` adds
  `on_checkpoint`, read only when `struct_size` says so. Rust: `GraphRuntime::with_checkpoint_sink`,
  `HostCallbacks::on_checkpoint` (defaulted) and `EngineSpec.hostCheckpointer`. Without a store,
  nothing changes.
- **A running checkpoint resumes by running its node again** (ADR 0049 D3). A process that dies
  while a node runs leaves the checkpoint written before it, status `running`; resuming from it
  re-runs that node, at least once, and never the ones before. Now pinned as the contract in Rust,
  TypeScript and Python.
- **Each `mapAgents` sub-agent reports its lifecycle** (ADR 0050). For every item of
  `overChannel`, the node sends `spawn_started` (`spawnId`, `itemIndex`, `item`), then one of
  `spawn_completed` (`output`, `usage`), `spawn_failed` (`error`) or `spawn_suspended` (`reason`),
  to `onEvent` (TypeScript) / `on_event` (Python) and the C-API's event callback. `spawnId` is the
  item's index — the id that sub-agent's `token_delta` events carry. Like `token_delta`, they are
  observational: never on the event bus, never in a checkpoint or a replay journal. The fan-out
  node's own `node_started` / `node_completed` are unchanged. Rust `map_node_handler` takes an
  optional `SpawnEventSink`; TypeScript exports the four variants in `RunEvent` and `SpawnUsage`.

### Changed (may break a graph that relied on the old behaviour)

- **A fan-out over something that is not a list fails** (ADR 0050, ailu#2033). A `mapAgents` or
  `mapSubgraph` node whose `overChannel` holds a string, an object, a number or a boolean now fails
  (`node_failed`, category `permanent`, then `run_failed`) with
  `mapAgents node '<id>': overChannel '<channel>' must be a JSON array, got string`. It used to
  write an empty array and go on, silently. An absent, `null` or empty channel is still a no-op.

## 2.5.0 — 2026-10-04

### Added

- **An approval for named values** (ADR 0048). A condition on a gated tool may name values:
  `approvalWhen: [{ argument: "agentName", in: ["Nordlys"] }]` (TypeScript `ToolDefinition`, the
  catalog carrier, Python `Tool(approval_when=...)`), next to ADR 0046's `{ argument, above }` —
  exactly one test per condition. The engine gates such a call when the argument is absent, not a
  string, or equal byte for byte to one of the values (`condition: 'agentName = "Nordlys"'`); any
  other string runs without a gate. The grant is that call, filed as ADR 0046's. A condition with
  both tests or neither, no value or an empty value is refused when the agent is built.

### Changed

- `ApprovalCondition` (Rust) carries `above` as an option, next to `one_of` (`in` on the wire); a
  2.4.0 condition reads and writes unchanged. In TypeScript it is a union of the two shapes.

## 2.4.0 — 2026-10-03

### Added

- **An approval above a threshold** (ADR 0046). A gated tool may carry conditions on its
  arguments, as data: `approvalWhen: [{ argument, above }]` (TypeScript `ToolDefinition`, the
  catalog carrier, Python `Tool(approval_when=...)`). The engine gates such a tool per call — when an
  argument is absent, not a number (`"600"` written as text) or above its threshold — and the
  grant is that call: the key is `<name>#<sha256(canonical input)>`, so a grant by name unlocks
  nothing and another call goes through the conditions again. A request names what crossed
  (`condition: "amount 600 > 500"`); its subject stays `tool:<name>`. A condition on a tool that is
  not approval-gated, an empty argument or a threshold that is not finite is refused when the
  agent is built.
- **A catalog run files the call** (ADR 0046 D4). A filed request's subject carries
  `approvalKey`, `input` and `condition` when the engine set them; on resume, a keyed grant needs
  the request filed for that key, approved by the person the grant names. Requests filed before
  resume as before.

### Changed

- **The brain is shown to an agent that may see it** (ADR 0047). An agent that declares
  `visibleChannels` is given the host's governed knowledge (`__brainRecall`) only when it names
  that channel among them; an agent with no list is unchanged. A web agent narrowed to the run's
  input no longer carries the organisation's brain in its first message.
- The TypeScript `ReActAgent` never lets a grant by name unlock a tool with conditions (it cannot
  pin a grant to a call): such a tool stays gated there.

## 2.3.0 — 2026-10-03

### Added

- **The engine reads catalog graphs** (ADR 0045 D3.2). `catalog::spec_from_catalog` in
  `crates/runtime-bridge` turns a saved graph — its nodes' `component`, `agent` and `mapAgents`
  carriers, its subgraphs, and the ids and names of the host's bindings — into the `EngineSpec`
  that the TypeScript SDK assembled itself until now: same defaults (provider `"anthropic"`, output
  channel `agentResult`, empty tool and approval lists), same `HostNodeBindingError` cases, same
  warning for a malformed `mapAgents` carrier. Bindings: `engineSpecFromCatalog` in
  `@ailu-ai/napi`, `engine_spec_from_catalog` in the Python extension. 40 golden cases recorded from
  the TypeScript SDK check the Rust function and both bindings.
- **The engine decides a catalog run's approvals** (ADR 0045 D3.1). `catalog_approvals` in
  `crates/runtime-bridge`: `filing_plan` lists what a suspended run files with its approval store
  — one request per gated tool an agent asked for (`tool:<name>`), one for the human gate it waits
  at (`gate:<node id>`), the same for a direct child run, filed under the child's run id — and
  whether a resume that now waits on something else clears the previously stashed ids;
  `approvals_to_check` and `resume_problems` decide whether a resume may go on (every request
  decided, no rejected gate, each granted tool approved by the approver the grant names). The host
  only stores and reads the requests. One rule is new: a request approved by its own requester
  blocks the resume (stores already refuse such an approval). Bindings:
  `engineCatalogApprovalPlan` / `engineCatalogApprovalsToCheck` / `engineCatalogResumeProblems`
  (`@ailu-ai/napi`) and their snake_case names in the Python extension. 41 golden cases recorded
  from the TypeScript SDK check the Rust functions and both bindings.
- **Python: run a saved graph** (ADR 0045 M2). `ailu.run_catalog_graph(definition, *,
  initial_data, run_id, nodes, tools, subgraphs, provider_keys, fs_policy, skills, on_event,
  is_cancelled)`, `resume_catalog_graph(definition, state, *, approved_tools, …)` and
  `replay_catalog_graph(definition, state, checkpoint_id, replay_journal, *, …)` mirror the
  TypeScript catalog runner over the engine's spec: agents, components, fan-outs, gates and
  subgraphs run on the engine, plain steps are yours, a replay calls nothing. A bad binding raises
  `ailu.HostNodeBindingError` (a `RunError`), a malformed `mapAgents` setting a `RuntimeWarning`;
  conditional edges are not taken, as in TypeScript. `GraphRunner.resume` takes `approved_tools`.
  `streamTokens` is not in Python yet: it comes with token streaming (M4).
- **The engine explains runs and checks replays** (ADR 0045 D3.4). `run_insight` in
  `crates/runtime-bridge`: `explain_run(state, events?)` (status, suspension and what unblocks it,
  failure, channel names, last 20 events) and `verify_replay_decisions(attested, replayed)` (the
  ordered `{ status, subject }` comparison of replay-as-evidence) — the TypeScript `explainRun` and
  `verifyReplayDecisions`, checked against 26 golden cases recorded from them. Bindings:
  `engineExplainRun` / `engineVerifyReplayDecisions` (`@ailu-ai/napi`), and in Python
  `ailu.explain_run(state, events=None)` and `ailu.verify_replay_decisions(attested, replayed)`.
- **Python: durable timers and signals, token streaming** (ADR 0045 M4). `ailu.sleep_until(wake_at,
  update)` and `ailu.wait_for_signal(name, wake_at=, update=)` — a step's return value that
  suspends the run, as the TypeScript `sleepUntil` / `waitForSignal` — with
  `ailu.read_suspend_meta(state)` and `ailu.read_signal(state_or_node, name)`. `stream_tokens=True`
  on `GraphRunner.run` and `run_catalog_graph` streams agents' replies as `token_delta` events (the
  TypeScript `streamTokens`).
- **Python: one model call** (ADR 0045 M4). `ailu.llm_complete(input, *, provider=, model=, tier=,
  max_tokens=, temperature=, response_format=, provider_keys=, base_url=, api_key_env=)` goes
  through the engine's gateway, as the TypeScript `model.invoke()`: the call itself moved from the
  N-API binding to `runtime_bridge::llm_complete_json`, which both SDKs now call. One difference:
  Python needs a `provider` or a `tier` (TypeScript's provider-less `model.invoke()` picks the
  provider in the SDK).
- **Python: a graph builder** (ADR 0045 M4). `ailu.create_graph(name)` with `.channel()`,
  `.node()` (a step of yours), `.agent_node()` (`ailu.Tool` for described / gated tools),
  `.component()`, `.human_gate()`, `.subgraph()`, `.edge()`, `.conditional_edge()`,
  `.error_edge()`, `.entry()`, `.fs_policy()` and `.compile()` → `CompiledGraph` (`run`, `resume`,
  `signal`, `replay`, `explain`, `definition`). It writes the definition the TypeScript
  `createGraph` writes for the same calls (checked against definitions recorded from it) and runs
  through the engine's catalog spec. Not yet in the Python builder: `mapAgents`, `taskNode`,
  `fanOut`, agents' memory / skills / filesystem / web search options.
- **Embeddings and vector search in the engine; Python gets them** (ADR 0045 M4).
  `llm_gateway::embeddings` (the Mistral / OpenAI `/embeddings` call, body, defaults and response
  reading of the TypeScript `createEmbeddings`) and `runtime_bridge::vectors` (cosine similarity
  and the top-k query of `createVectorStore`), checked against golden cases recorded from the
  TypeScript helpers. Bindings: `engineEmbed`, `engineEmbeddingsBody`,
  `engineParseEmbeddingsResponse`, `engineQueryVectors`, `engineCosineSimilarity` (napi) and their
  PyO3 counterparts. Python: `ailu.create_embeddings(...)` (with `transport=` for offline use),
  `ailu.create_vector_store(persist_path=None)` (the same JSON file as TypeScript) and
  `ailu.cosine_similarity(a, b)`. The TypeScript helpers keep their own implementation for now
  (switching them to the engine changes their HTTP stack: a follow-up for the owner to decide).
- **C ABI: cancellation** (ADR 0045 D2.4). `ailu_engine_run_json_v2`,
  `ailu_engine_resume_json_v2`, `ailu_engine_approve_and_resume_json_v2` and
  `ailu_engine_signal_json_v2` take a pointer to an `AiluCallbacksV2`: the `AiluCallbacks` fields
  plus `is_cancelled`, asked at every node boundary (non-zero stops the run there with status
  `"cancelled"`, ADR 0044), and a `struct_size` that lets a later release append fields. The
  original entry points and their by-value `AiluCallbacks` are unchanged, so SDKs built against an
  earlier release keep working. The C++ wrapper exposes the `_v2` functions; the other `sdks/`
  wrappers do not yet (each needs the struct in its FFI layer).
- **C ABI: saved graphs and their approvals** (ADR 0045 D3). `ailu_spec_from_catalog_json`,
  `ailu_catalog_approval_plan_json`, `ailu_catalog_approvals_to_check_json` and
  `ailu_catalog_resume_problems_json`: the same engine functions as on N-API and PyO3, for the C
  ABI SDKs (no wrapper calls them yet).
- **Python: approvals on saved graphs** (ADR 0045 D3.1). `run_catalog_graph` and
  `resume_catalog_graph` take `approval_engine=`, decided by the engine as the TypeScript
  `approvalEngine` is: a suspended run files its requests, a resume raises
  `ailu.ApprovalNotGrantedError` (`run_id`, `problems`) until they are approved. An approval
  engine is two methods over your storage — `request(*, run_id, node_id, requested_by, subject)`
  and `get_by_id(request_id)` (the `ailu.ApprovalEngine` protocol);
  `ailu.InMemoryApprovalEngine` adds `approve`, `reject` and `get_pending` for development, and
  refuses a requester resolving their own request (`ailu.ApprovalSelfApprovalError`).

### Changed

- **`engine_version()` reports the release.** The crates' workspace version now moves with the
  published packages (2.3.0); it had stayed at 1.4.0, so `engineVersion()`,
  `ailu.engine_version()` and `ailu_engine_version()` said 1.4.0 on the 2.x releases.
- **The Rust approval attestation is the TypeScript one, byte for byte** (ADR 0045 D3.3, step 1).
  `Ed25519Attestor` writes the public key as SPKI DER, as `@ailu-ai/approval-engine` does (a raw
  32-byte key is still accepted when verifying), and its canonical JSON is JavaScript's: numbers as
  `JSON.stringify` writes them (`100000000000000000000`, `1e+21`, `1e-7`, `-0` → `0`) and object keys
  in UTF-16 code unit order. `Ed25519Attestor::from_seed` derives the key pair a seed gives in
  TypeScript. A golden fixture signed by the TypeScript implementation
  (`crates/approval-engine/tests/fixtures/ts-attestations.json`, regenerated by
  `scripts/golden/attestations.ts`) is verified, and re-signed into the same bytes, by the Rust one —
  the step before any SDK switches to it.
- **TypeScript: the catalog runner uses the engine's spec** (ADR 0045 D3.2). `runCatalogGraph`,
  `resumeCatalogGraph` and `replayCatalogGraph` no longer read the carriers themselves: they hand
  the definition and the names of their bindings to `spec_from_catalog`. Same options, same
  results, same spec for the 40 golden cases; the malformed-mapAgents warning and
  `HostNodeBindingError` are unchanged, and a `toolSpecs` carrier that is not a list still throws a
  `TypeError`, now naming its node. Requires `@ailu-ai/napi` at the SDK's version, as before.
- **TypeScript: `explainRun` and `verifyReplayDecisions` are the engine's** (ADR 0045 D3.4). Same
  signatures and results (26 golden cases recorded from the TypeScript functions); they now need
  `@ailu-ai/napi`, like the rest of the SDK. A mismatch with nothing on one side has no
  `attested` / `replayed` key instead of one set to `undefined`.
- **TypeScript: the engine decides the catalog runner's approvals** (ADR 0045 D3.1). With an
  `approvalEngine`, `runCatalogGraph` and `resumeCatalogGraph` file what `filing_plan` lists and
  refuse a resume for the problems `resume_problems` reports; the `ApprovalEngine` only stores and
  returns requests. Same requests, ids, problems and `ApprovalNotGrantedError` for the 41 golden
  cases. One refusal is new: a request approved by its own requester (an `ApprovalEngine` that
  follows the contract never records one).

## 2.2.0 — 2026-10-02

### Added

- **Host nodes: your code as a step, journaled, never re-run by a replay** (ADR 0045 D1, #288). An
  `EngineSpec` names the nodes whose step is the host's in `hostNodeIds` (`jsNodeIds` is still
  read). Each execution's `on_node` payload carries an `effectKey`: sha256 of the run id, the node
  id and the state version at the node's entry — the same for a retry of the step from the same
  checkpoint, so an external effect (a message, a record) can be performed at most once. In record
  mode (`AILU_LLM_RECORD=1`) the journal stores each execution's result as `nodeResults` (the
  input hashed, never stored); a replay serves it and never calls the step. A replay that reaches
  a step its journal has no result for fails with `node_input_mismatch`. A journal recorded before
  2.2.0 has no `nodeResults` and replays as before.
- **TypeScript: host nodes on the catalog path** (#289). `runCatalogGraph` and
  `resumeCatalogGraph` accept `nodes: [{ id, execute }]`: a plain action or tool node of the graph
  runs `execute({ nodeId, channels, effectKey })` and applies the update it returns. A binding that
  names no plain node, or one twice, throws `HostNodeBindingError` (`AILU_HOST_NODE_BINDING`).
  `replayCatalogGraph` takes no bindings. Builder node functions receive `{ nodeId, effectKey }`
  as a second argument. Python binds host nodes through `GraphRunner` (below); its catalog runner
  comes in 2.3.0, when turning a saved graph into an engine spec moves to Rust (ADR 0045 M2).
- **Python: run a graph** (#290). `ailu.GraphRunner(spec, nodes=, tools=, conditions=, on_event=)`
  drives the same engine runner as the TypeScript SDK: `run`, `resume`, `approve_and_resume`,
  `signal` and `replay`, with an `is_cancelled` check at node boundaries. A step receives
  `HostNodeInput(node_id, channels, effect_key)`. Still TypeScript-only (ADR 0045 phasing): saved
  graphs with agent settings on their nodes (2.3.0); a graph builder, token streaming, model calls
  and the durable helpers (2.5.0).

### Changed

- A record-mode journal now carries a result for every plain step of a catalog run, bound or not.
  Its replay checks the state each step was given against the recording, so a replay that used to
  pass while the run diverged between model calls now fails with `node_input_mismatch`.

## 2.1.0 — 2026-09-29

### Added

- **Web search by the model provider** (#284). `agentNode({ webSearch: { maxUses?, allowedDomains?,
  blockedDomains? } })` lets the provider itself search the web on the agent's calls. With Mistral
  this is the `web_search` connector, through its Conversations API with `store: false`. With
  Anthropic it is the `web_search` server tool. `agentResult.webSearch` reports what the search
  produced: the pages consulted or cited (URL, title, quoted passage, page age, cited or not),
  the number of searches, the queries the model wrote when the provider reports them, and the
  provider's error code if a search failed. On the gateway: `LlmRequest.webSearch` and
  `LlmResponse.webSearch`. Both are part of the recorded LLM journal, so a replay returns the same
  sources without a network call. Anthropic's paused search turns (`pause_turn`) are continued.
- The engine refuses web search where it cannot honor it, rather than answering without the web
  (`AILU_WEB_SEARCH_INVALID` in the SDK, `WebSearchUnsupported` from the gateway): another
  provider, tools, the filesystem or memory on the same agent (the provider's search cannot be
  mixed with client tools yet), structured output, or both domain lists at once. Offline
  (`AILU_LLM_MOCK=1`) the mock reports an empty outcome and makes no network call.

## 2.0.0 — 2026-09-29

### Changed (breaking)

- **The project is renamed to Ailu.** Every public name changes, with no compatibility alias:
  - npm packages publish under the `@ailu-ai` scope (`@ailu-ai/graph-sdk`, `@ailu-ai/napi`, `@ailu-ai/cli`,
    `@ailu-ai/model-core`, `@ailu-ai/contracts`, `@ailu-ai/config`, `@ailu-ai/verify`), and the project
    scaffolder is `npm create @ailu-ai` (`@ailu-ai/create`);
  - Rust crates are `ailu-*`, the CLI binary is `ailu`, the C ABI uses the `ailu_` / `AILU_`
    prefixes (`include/ailu.h`), and the Python package is `ailu` (`import ailu`) — on PyPI its
    distribution keeps the name `adriane-ai` for now (`pip install adriane-ai`);
  - environment variables use the `AILU_` prefix (e.g. `AILU_RERANK_ENDPOINT`,
    `AILU_PII_REDACTOR_URL`, `AILU_SDK_ENGINE`); the previous prefix is no longer read;
  - error codes use the `AILU_` prefix too (e.g. `AILU_RUST_ENGINE_REQUIRED`,
    `AILU_UNKNOWN_PROVIDER`); code that matches on `error.code` must be updated;
  - the DSL crates / packages are `lang-ailu` and `graph-ailu`.
- **An agent with no API key fails instead of running on a mock.** Until now a graph agent, a
  prebuilt agent or a local-server model (`model.ollama()` without `AILU_USE_OLLAMA=1`) with no
  credentials ran silently on a built-in mock, and the run reported `completed` with a canned
  answer. It now fails with an error naming the variable to set. Offline runs are explicit:
  set `AILU_LLM_MOCK=1` (tests, CI, trying the examples) and every model call without
  credentials answers from the deterministic mock, on every path (graphs, `runCatalogGraph`,
  prebuilt agents, `model.invoke()`).
- **A model that names its provider stays on that provider.** `model.anthropic.frontier` with no
  Anthropic key used to run on whichever provider had a key; it now fails with the missing
  variable. Only a tier-only model (`model.fast`) picks its provider from the keys present.
- **`councilAnonymize` no longer writes `memberId` into the reviewers' field.** The field holds
  `{ label, content }` only; the new `keyInto` parameter writes the `{ label, memberId }` key
  for the audit trail. `council()` sets it to `fieldKey`.
- **`approveAndResume` requires `resolvedBy`.** It defaulted to `"human"`, so an approval could be
  recorded without naming the approver. A missing or blank value now throws
  `ApproverRequiredError` (`AILU_APPROVER_REQUIRED`). The MCP tool `approve_and_resume` requires
  `approvedBy` for the same reason.
- **`resumeCatalogGraph` with an `approvalEngine` checks the engine before resuming.** It throws
  `ApprovalNotGrantedError` (`AILU_APPROVAL_NOT_GRANTED`) while a request the run waits on is
  pending, when a human gate was rejected, when a granted tool has no request approved by the
  approver the grant names, or when the state comes from a run started without the engine.
  Without `approvalEngine` nothing changes.
- **An unknown provider name fails.** `agentNode({ provider: "groq" })`, or a stored graph whose
  agent names an unknown provider, ran on Anthropic; it now fails (`AILU_UNKNOWN_PROVIDER` in the
  SDK, `unknown model provider` from the engine).

### Added

- `finalAnswer(result)` returns an agent's answer text (after the last `final:` marker).
- **Documentation rewritten for 1.28**: 33 pages (start, one guide per task, examples, reference)
  instead of 106, one way per task, deprecated APIs on a single Migrating page, and redirects from
  every old URL. Every TypeScript block in the docs is included from a file under
  `packages/graph-sdk/examples/` that the test suite typechecks and runs; the model-tier table and
  the component reference are generated from the code, and `llms-full.txt` is built from the pages.
- Every example in `packages/graph-sdk/examples/` runs in the test suite (offline), with new
  examples for resuming across processes, parallel agents, a deep agent and streaming. The docs
  site is built on pull requests, and CI runs the Python SDK tests.
- `agentNode({ visibleChannels })`: the only channels an agent is shown in its seed state
  (context isolation). `council()` uses it so members see only the query, reviewers the query
  and the anonymized field, and the chair the field and its ranking.
- `pnpm --filter @ailu-ai/graph-sdk run gen:docs` regenerates `llms.txt` and the component reference; a test fails
  when the committed file no longer matches the SDK.
- Tests check the Errors, Events and Environment variables reference pages against the code: an
  error code, an event or event field, or an environment variable missing from its page (or
  documented but gone from the code) fails the suite.
- `ResumeStateNotFoundError`, `ApproverRequiredError` and `ApprovalNotGrantedError` are exported.

### Fixed

- The LLM now sees each tool's own `description` and input JSON Schema (`jsonSchema`). The
  engine advertised every tool as `Tool '<name>'.` with an empty object schema.
- `agentNode({ model: "openai:gpt-4o" })` is parsed like `model("openai:gpt-4o")`. The string
  was kept as a model id and the agent ran on the default provider; an unknown provider in the
  string now fails loud.
- The agent carrier saved in `app.definition` carries every agent field (custom `baseURL` and
  `apiKeyEnv`, tool descriptions, visible channels), so `runCatalogGraph(app.definition)` runs
  the same agent as `app.run()`. A builder `mapAgents` node is also saved in the `mapAgents`
  carrier that `runCatalogGraph` reads, so it fans out there too.
- Provider keys are read from the same variables everywhere: `GEMINI_API_KEY` or
  `GOOGLE_API_KEY` for Gemini, `HF_TOKEN` or `HUGGINGFACE_API_KEY` for Hugging Face
  (`model.invoke()` read only `HUGGINGFACE_API_KEY`, graphs only `HF_TOKEN`).
- The SDK and Python READMEs no longer describe the removed TypeScript engine fallback;
  `llms.txt` names the `.component()` builder method.
- Python tests: the component-catalog test no longer expects a fixed count (#260).
- `council()` reviewers and chair now read each member's actual answer; they were given empty
  text because an agent result has no `content` field.
- On the catalog runner, a run that suspends again after a resume (a second gate or tool) files
  its new approval request in the `ApprovalEngine`; the ids of the previous suspension were kept
  and the new one was skipped.
- `app.explain(runId)` on a run waiting for a tool approval points at `approveAndResume` with the
  tool's name, instead of `resume`.
- `npm create @ailu-ai` starts new apps on the current SDK minor version (it pinned `^1.2.0`).
- The Events page lists `token_delta`'s `parentRunId` and `spawnId`, and the `tool_call` stream
  event.
- `buildDocQaReference()` no longer builds a scripted TS model that the engine ignored; its `llm`
  option is deprecated and ignored. Stale comments about the removed TypeScript engine are gone
  from the SDK's type docs.
- Every example runs on 1.28: `qa-rag.ts`, `startup-e2e.ts` and `finance-sage-optimization.ts`
  failed (a scripted TS model is ignored by the engine; an ApprovalEngine on `agentNode` is not
  supported on `app.run()`), and all examples used the deprecated `llm` option.

## 1.27.0

### Added

- **Cooperative run cancellation — a real kill switch (ADR 0044).** `runCatalogGraph` /
  `resumeCatalogGraph` accept `signal?: AbortSignal`. Abort it and the engine stops the run at the
  **next node boundary**: it finishes the node in flight, writes that node's checkpoint, emits a
  new `run_cancelled` lifecycle event and returns a terminal `CatalogRunOutcome` with the new
  `status: "cancelled"`. Until now a catalog run could not be stopped at all once started —
  `runCatalogGraph` was a single atomic call into the Rust engine and `on_event` is fire-and-forget,
  so no seam could ever answer "stop".

  Cancellation is **cooperative, never pre-emptive**: a node already executing always runs to
  completion, so the last checkpoint stays authoritative and a cancelled run remains resumable and
  replayable. Its latency is therefore the duration of the node in flight, and the engine offers no
  hard abort — a caller needing a bounded stop must impose its own deadline.

  New surface: `GraphStatus::Cancelled` / `"cancelled"` (Rust + TS `graph-core`, and
  `@ailu-ai/contracts`, where it is distinct from both `failed` and the control plane's
  `rejected`); `RunEvent::RunCancelled { runId, nodeId, timestamp }`;
  `GraphRuntime::with_cancel_check`; and an optional trailing `isCancelled` callback on
  `engine_run` / `engine_resume` / `engine_approve_and_resume` / `engine_signal`. A cancelled
  subgraph child propagates to its parent rather than reading as completed.

  **Fully backward compatible**: omit the signal (or implement none of the seam) and every run
  behaves exactly as before. `engine_replay` deliberately takes no cancellation seam — a replay
  must reproduce what happened, so a live flag must never perturb it.

## 1.18.1

### Fixed

- **`answerBuilder` / `outputParser` / any component reading an agent's output channel now unwrap the
  `AgentResult`.** `value_to_text` treated an `AgentResult` object as raw JSON, so a downstream
  component received the stringified wrapper (`{ reasoning: "thought:…\nfinal:…", approvalRequests, … }`)
  instead of the agent's answer. It now extracts the final answer — the validated `structuredOutput`
  when present, else the text after the last `final:` marker in `reasoning`. Fixes Governed Ask
  answers, co-authoring projections, and the enterprise-analysis proposal all surfacing as raw JSON.

## 1.18.0

### Changed

- **Council is now a native catalog graph.** `council(...)` returns a `GraphDefinition` whose anonymize/aggregate steps are Rust catalog components (`councilAnonymize` / `councilAggregate`) instead of JS handlers — so a council runs on the Rust engine via `runCatalogGraph`, like every other governed graph (dogfood / Rust-only, ADR 0003). BREAKING vs 1.17.0: `council(...)` returns a `GraphDefinition` (run it with `runCatalogGraph(council(...))`), not a `CompiledGraph`. The pure `anonymizeAndShuffle` / `aggregateRanks` helpers stay exported. (ADR 0061)

## 1.17.0

### Added

- **LLM Council** — `council({ members, reviewers?, chair, humanGate? })` builds a governed
  deliberation graph: dispatch → members (fan-out) → anonymized peer-review (fan-out) → Borda
  aggregate → optional human gate → chair synthesis. Native agent seats (a member never reviews its
  own answer; every seat audited), deterministic replay-faithful anonymize + aggregate. (ADR 0013/0061)

## 1.16.0

### Added

- **Cross-encoder reranking (ADR 0060 E1)** — a `reranker` node now re-scores its candidates through a
  real cross-encoder (`BAAI/bge-reranker-v2-m3`) served by a self-hostable, EU-sovereign rerank service
  (HuggingFace TEI), configured by `AILU_RERANK_ENDPOINT`. The gateway holds the HTTP call behind a
  transport seam; the runtime routes `reranker` nodes to it. **Graceful fallback**: with no endpoint the
  reranker is an identity passthrough that preserves the upstream ranking (no external call, no
  mock-cosine rescoring). Fail-open: a rerank error keeps the upstream order.

## 1.15.0

### Added

- **Per-token streaming on the catalog run path** — `runCatalogGraph({ streamTokens: true })` surfaces
  an agent node's generation as `token_delta` run events over `onEvent`, so a catalog run (e.g. the
  product's Governed Ask) can stream its answer token-by-token instead of only returning a final
  result. Reuses the streaming chain already wired for the in-process builder path (`CompiledGraph.stream`)
  — no Rust or napi change; the assembled state is byte-identical (deltas are observational, they bypass
  the checkpoint/journal). Default off. (ADR 0033, ADR 0060)

## 1.14.0

### Added

- **Dynamic `mapAgents` fan-out on the catalog path** — a `mapAgents` carrier lets a catalog graph fan a
  node out over a runtime-sized list, executed natively; a malformed carrier now warns instead of failing
  silently. (ADR 0049)

## 1.0.0

First stable engine release. The Rust runtime reaches (and extends) parity with the
TypeScript runtime, gains durable timers and external signals, and the knowledge layer
(OKF + KB/KG) descends into the engine.

### Added

- **Concurrent, deterministic fan-out** — a node's branches run in parallel off a shared
  pre-fan-out snapshot and merge in declared order (fixes the prior sequential port that
  accumulated state between branches). (ADR 0008)
- **Subgraphs** — a `subgraph` node runs a registered child graph (sharing the runtime's
  registries / checkpointer / event bus), maps channels in and out, and propagates child
  suspension; a child suspended on an internal human gate resumes across napi calls. (ADR 0008)
- **Incremental streaming** — `CompiledGraph.stream()` projects all four modes
  (`values` / `updates` / `messages` / `debug`) incrementally on the Rust engine. (ADR 0008)
- **Durable timers + external signals** — `NodeOutput.sleep_until` / `wait_for_signal`,
  `GraphRuntime::resume_with_signal`, napi `engine_signal`, and SDK
  `sleepUntil` / `waitForSignal` / `CompiledGraph.signal` / `readSuspendMeta`. Two new
  generalized suspend reasons; the engine stays clock-free (`wakeAt` is data — the control
  plane schedules the wake). (ADR 0009)
- **Dynamic-message `send` / inbox** — pre-queue per-node inputs (`RunOptions.inbox`),
  each consumed one-per-execution via the reserved `__injected` channel: the map-reduce seam.
- **`@ailu-ai/okf` + `ailu-okf`** — the Open Knowledge Format parser/serializer
  descends into the engine (byte-compatible TypeScript + Rust, no YAML/regex dependency).
- **`@ailu-ai/knowledge` + `ailu-knowledge`** — the knowledge-base + knowledge-graph
  model, pure graph ops (build-graph, depth-limited neighbors, cosine search), and the
  `KnowledgeStore` seam (+ an in-memory implementation).

### Changed

- **`RunEvent` wire fields are now camelCase** (`runId` / `nodeId`, was snake_case) to match
  the TypeScript `RunEvent` the SDK parses — `event.nodeId` was `undefined` on the JS side.
  Consumers reading `run_id` / `node_id` off a forwarded event must switch to `runId` / `nodeId`.

## 0.2.0

Additive, backward-compatible engine features.

### Added

- **Multi-provider LLM gateway** — a **native Google Gemini** adapter (`generateContent`)
  plus the OpenAI-compatible family: **OpenAI, OpenRouter, MiniMax, Hugging Face, LM Studio**
  alongside the existing Mistral and Ollama. A new provider is an enum slot + a constructor;
  selection is by which env credential is present, so a deployment brings its own model
  (BYOM) and can run fully on-premise with local models. (ADR 0005, #24)
- **`semanticRetriever` component** — genuine semantic retrieval: ranks pre-embedded chunks
  by cosine similarity to a pre-embedded query (real embeddings, e.g. Mistral), unlike the
  mock-embedding `retriever`. (#25)
- **Knowledge base as MCP resources** — the MCP server exposes a knowledge base as MCP
  `resources` (`resources/list` + `resources/read`), so any MCP client (Claude Desktop, an
  IDE, another agent) can read it through the open standard. (#26)
- **Contracts** — knowledge, compliance, and LLM-router DTOs added to `@ailu-ai/contracts`. (#26, #27)
- **ADR 0006** — sovereign deployment modes (EU cloud / private cloud / true on-premise) and
  granular per-knowledge-base permissions. (#27)

### Notes

- The deprecated TypeScript fallback gateway intentionally stays at two adapters; the
  broader provider family lives on the Rust engine (the default execution path).

## 0.1.0

Initial public release: the Rust agentic graph runtime, the TypeScript & Python SDKs over
it, the Ailu DSL compilers, the component/agent library, the CLI, and the MCP plugin.
