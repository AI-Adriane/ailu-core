# Changelog

All notable changes to the Ailu engine are documented here. The project follows
[Semantic Versioning](https://semver.org/).

## Unreleased

### Added — 2.7.0

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
