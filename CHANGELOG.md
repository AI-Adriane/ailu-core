# Changelog

All notable changes to the Ailu engine are documented here. The project follows
[Semantic Versioning](https://semver.org/).

## Unreleased

### Changed (breaking)

- **The project is renamed to Ailu.** Every public name changes, with no compatibility alias:
  - npm packages publish under the `@ailu-ai` scope (`@ailu-ai/graph-sdk`, `@ailu-ai/napi`, `@ailu-ai/cli`,
    `@ailu-ai/model-core`, `@ailu-ai/contracts`, `@ailu-ai/config`, `@ailu-ai/verify`), and the project
    scaffolder is `npm create @ailu-ai` (`@ailu-ai/create`);
  - Rust crates are `ailu-*`, the CLI binary is `ailu`, the C ABI uses the `ailu_` / `AILU_`
    prefixes (`include/ailu.h`), and the Python distribution and package are `ailu`;
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
- **`councilAnonymize` no longer writes `memberId` into the reviewers' field.** The field holds
  `{ label, content }` only; the new `keyInto` parameter writes the `{ label, memberId }` key
  for the audit trail. `council()` sets it to `fieldKey`.

### Added

- `agentNode({ visibleChannels })`: the only channels an agent is shown in its seed state
  (context isolation). `council()` uses it so members see only the query, reviewers the query
  and the anonymized field, and the chair the field and its ranking.
- `pnpm --filter @ailu-ai/graph-sdk run gen:llms-txt` regenerates `llms.txt`; a test fails
  when the committed file no longer matches the SDK.

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
