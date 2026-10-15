/** Version of the bound Rust engine. */
export function engineVersion(): string;

/**
 * Validate a graph definition (JSON). Returns a JSON array of validation errors
 * (`[]` when sound). Throws on malformed JSON.
 */
export function validateGraphJson(definitionJson: string): string;

/**
 * Compile graph DSL YAML into a validated `GraphDefinition` (JSON string).
 * Throws with a clear message on parse, DSL, or structural validation failure.
 */
export function compileGraphYamlJson(yaml: string): string;

/**
 * Build the `EngineSpec` of a catalog graph (ADR 0045 D3.2) — the engine reads the
 * `component` / `agent` / `mapAgents` carriers of the graph's and its subgraphs' nodes.
 * `inputJson` is `{ graph, subgraphs?, hostNodes?, hostTools?, providerKeys?, fsPolicy?, skills? }`.
 * Returns `{ spec, warnings }` as JSON, or `{ error: { kind, message, nodeId?, reason? } }` when a
 * host node binding or a carrier cannot be used. Throws only on malformed input JSON.
 */
export function engineSpecFromCatalog(inputJson: string): string;

/**
 * What a catalog run's host files in its approval store (ADR 0045 D3.1). `inputJson` is
 * `{ graph, subgraphs?, state, previousState? }` (`previousState` for a resume). Returns
 * `{ clearApprovalIds, requests: [{ runId, nodeId, requestedBy, subject }] }` as JSON.
 */
export function engineCatalogApprovalPlan(inputJson: string): string;

/**
 * The stashed approval ids a resume of a catalog run must read back from the store (ADR 0045
 * D3.1). `stateJson` is the suspended `GraphState`; returns a JSON array of ids.
 */
export function engineCatalogApprovalsToCheck(stateJson: string): string;

/**
 * A call's identity (ADR 0051 D1), `<name>#<hex sha256 of callInputOf(inputJson)>`, from the
 * arguments' JSON text. Throws when `inputJson` is not JSON.
 */
export function callKeyOf(name: string, inputJson: string): string;

/**
 * The canonical text of a call's arguments, the exact bytes its call key hashes (ADR 0051 D1).
 * Throws when `inputJson` is not JSON.
 */
export function callInputOf(inputJson: string): string;

/**
 * Why a resume of a catalog run may not go on (ADR 0045 D3.1). `inputJson` is
 * `{ graph, subgraphs?, state, approvedTools?, approvals: { <id>: record | null } }`; returns a
 * JSON array of problems, empty when the resume may go on.
 */
export function engineCatalogResumeProblems(inputJson: string): string;

/**
 * Replay-as-evidence (ADR 0038), decided by the engine (ADR 0045 D3.4): compare the ordered
 * `{ status, subject }` decisions of an attested chain (`attestedJson`) with a replay's
 * (`replayedJson`). Returns `{ ok, attested, replayed, mismatches }` as JSON.
 */
export function engineVerifyReplayDecisions(attestedJson: string, replayedJson: string): string;

/**
 * Explain a run from its `GraphState` and, optionally, its run events (ADR 0045 D3.4). Returns the
 * explanation (`{ runId, status, currentNode, summary, channels, recentEvents?, suspended?,
 * failure? }`) as JSON.
 */
export function engineExplainRun(stateJson: string, eventsJson?: string | null): string;

/** Embed texts through the provider's API (ADR 0045 M4); resolves to the vectors as JSON. */
export function engineEmbed(optionsJson: string, textsJson: string): Promise<string>;
/** The request body (JSON) an embeddings call sends for the texts (ADR 0045 M4). */
export function engineEmbeddingsBody(optionsJson: string, textsJson: string): string;
/** Read an embeddings API response (JSON) into its vectors (JSON) (ADR 0045 M4). */
export function engineParseEmbeddingsResponse(responseJson: string): string;
/** The `k` stored items most similar to the embedding, by cosine similarity (ADR 0045 M4). */
export function engineQueryVectors(itemsJson: string, embeddingJson: string, k: number): string;
/** Cosine similarity of two vectors given as JSON arrays (ADR 0045 M4). */
export function engineCosineSimilarity(aJson: string, bJson: string): number;

/**
 * One-shot LLM completion over the Rust gateway (ADR 0031 — backs the SDK `Model.invoke()`
 * overlay). `requestJson` is a serialized `LlmRequest`; `providerKeysJson` is a
 * `{ "<provider>": "<key>" }` map (`"{}"` → env keys, else a deterministic mock). Resolves to
 * a serialized `LlmResponse`. The HTTP happens in Rust — no TS provider client.
 */
export function llmComplete(requestJson: string, providerKeysJson: string): Promise<string>;

/**
 * A JS callback invoked from Rust during a run. Receives a JSON string payload.
 *
 * `onNode` and `onCondition` may be **async**: they can return a `Promise`, and Rust
 * awaits it (the napi bridge resolves the returned promise to its JS-resolved value
 * before continuing). A synchronous return is still accepted. `onEvent` is
 * fire-and-forget — its return is never awaited.
 *
 * - `onNode(payloadJson)`: `payloadJson` is either
 *   `{ kind: "node", nodeId, input, state }` (a custom JS node handler — return the
 *   channel-update JSON, or a `Promise` resolving to it) or
 *   `{ kind: "tool", name, input }` (a JS tool `execute` fn — return the tool-result
 *   JSON, or a `Promise` resolving to it).
 * - `onCondition(payloadJson)`: `payloadJson` is `{ name, state }`; return a boolean,
 *   a boolean-ish string (`"true"`/`"false"`), or a `Promise` resolving to either.
 * - `onEvent(payloadJson)`: `payloadJson` is a serialized `RunEvent`; return nothing
 *   (fire-and-forget).
 */
export type EngineNodeCallback = (payloadJson: string) => string | Promise<string>;
export type EngineConditionCallback = (
  payloadJson: string
) => boolean | string | Promise<boolean | string>;
export type EngineEventCallback = (payloadJson: string) => void;

/**
 * Start a fresh run of a graph on the Rust engine. `specJson` is the `EngineSpec`
 * (graph, runId, initialData, agents, jsNodeIds, jsToolNames). Resolves to a JSON
 * `RunOutcome` (`{ state, status, pendingApprovals }`).
 */
export function engineRun(
  specJson: string,
  onNode: EngineNodeCallback,
  onCondition: EngineConditionCallback,
  onEvent: EngineEventCallback
): Promise<string>;

/**
 * Resume a previously suspended run from its serialized state (`specJson.state`).
 * Resolves to a JSON `RunOutcome`.
 */
export function engineResume(
  specJson: string,
  onNode: EngineNodeCallback,
  onCondition: EngineConditionCallback,
  onEvent: EngineEventCallback
): Promise<string>;

/**
 * Grant the tools in `specJson.approvedTools` (written into the `__approvedTools`
 * channel) and resume. Resolves to a JSON `RunOutcome`.
 */
export function engineApproveAndResume(
  specJson: string,
  onNode: EngineNodeCallback,
  onCondition: EngineConditionCallback,
  onEvent: EngineEventCallback
): Promise<string>;

/**
 * Deliver an external signal to a suspended run, then resume. `signalName` is the
 * signal a `waitForSignal` node is blocked on; `payloadJson` is its JSON payload
 * (injected into `__signals[signalName]`). The run advances past the waiting node.
 * `specJson.state` carries the serialized suspended `GraphState`. Resolves to a JSON
 * `RunOutcome`.
 */
export function engineSignal(
  specJson: string,
  signalName: string,
  payloadJson: string,
  onNode: EngineNodeCallback,
  onCondition: EngineConditionCallback,
  onEvent: EngineEventCallback
): Promise<string>;
