/**
 * `@ailu-ai/graph-sdk` — the front door to the Ailu framework.
 *
 * Build, compile and run stateful, resumable agent graphs without touching the
 * lower-level engine primitives. Everything you need for the common case is
 * re-exported here.
 *
 * ```ts
 * import { createGraph } from "@ailu-ai/graph-sdk";
 *
 * const app = createGraph({ name: "greeter" })
 *   .node("hello", async (_input, state) => ({ greeting: `Hello, ${state.channels.name}!` }))
 *   .compile();
 *
 * const result = await app.run({ name: "Ada" });
 * console.log(result.channels.greeting); // "Hello, Ada!"
 * ```
 */

export { createGraph, GraphBuilder } from "./builder.js";
export type { CreateGraphOptions, ChannelInput, NodeInput } from "./builder.js";

export { CompiledGraph, RustEngineRequiredError } from "./compiled-graph.js";
export type { CompiledGraphParts, RunOptions, ApproveAndResumeOptions } from "./compiled-graph.js";

export {
  sleepUntil,
  waitForSignal,
  readSuspendMeta,
  readSignal,
  SLEEP_UNTIL_KEY,
  WAIT_FOR_SIGNAL_KEY,
  SUSPEND_META_KEY,
  SIGNALS_KEY
} from "./durable.js";
export type { SuspendMeta } from "./durable.js";

export { readInjected, INJECTED_KEY } from "./send.js";

export { exampleGraphs } from "./example-graphs.js";
export {
  anonymizeAndShuffle,
  aggregateRanks,
  parseRanking,
  council,
  type MemberAnswer,
  type AnonymizedAnswer,
  type CouncilSeat,
  type CouncilOptions
} from "./council.js";
export type { ExampleGraph } from "./example-graphs.js";

// The complete Doc-QA reference pipeline (input → output), runnable as a CompiledGraph
// or persistable as a carrier-bearing GraphDefinition for the catalog run path.
export {
  buildDocQaReference,
  docQaReferenceDefinition,
  DEFAULT_REFERENCE_CORPUS
} from "./reference-graph.js";
export type { DocQaReferenceOptions } from "./reference-graph.js";

// Pure compute components (run natively on Rust, faithful TS fallback) and the
// prebuilt, tier-tagged micro-agent graphs.
export { components } from "./components.js";
export type {
  ComponentDescriptor,
  IntegrationComponentHandler,
  ComponentKind,
  RustComponentConfig,
  PromptBuilderParams,
  JsonValidatorParams,
  OutputParserParams,
  RouterParams,
  RouterRule,
  RetrieverParams,
  RetrieverDoc,
  RerankerParams,
  TextCleanerParams,
  DocumentSplitterParams,
  HtmlToTextParams,
  CsvParserParams,
  DocumentJoinerParams,
  DeduplicatorParams,
  TruncatorParams,
  RegexExtractorParams,
  AnswerBuilderParams,
  FieldMapperParams,
  LexicalDoc,
  Bm25RetrieverParams,
  KeywordRetrieverParams,
  SentenceWindowSplitterParams,
  LanguageDetectorParams,
  PredicateOp,
  MetadataFilterParams,
  ListJoinerParams,
  MergeRankerParams,
  EvaluatorParams,
  ChatMessageSpec,
  ChatMessageBuilderParams,
  ConditionalRouterBranch,
  ConditionalRouterParams,
  DocumentWriterParams,
  HttpFetchParams,
  HttpFetchResult,
  HttpFetchImpl,
  HttpFetchRequestInit,
  HttpFetchResponseLike,
  WebSearchParams,
  WebSearchResult,
  WebSearchOutcome,
  WebSearchImpl,
  WebSearchTransport
} from "./components.js";

export { prebuilt } from "./prebuilt-agents.js";
export type { PrebuiltOptions, RagAnswererOptions } from "./prebuilt-agents.js";

// Real embeddings + a vector store + a semantic (vector-store) retrieval connector.
// Exported SDK helpers — NOT catalog component kinds — all injectable for offline tests.
export {
  createEmbeddings,
  MissingEmbeddingsKeyError,
  EmbeddingsResponseError
} from "./embeddings.js";
export type {
  Embeddings,
  EmbeddingsTransport,
  EmbeddingsRequestBody,
  EmbeddingsProvider,
  CreateEmbeddingsOptions
} from "./embeddings.js";

export { createVectorStore, cosineSimilarity } from "./vector-store.js";
export type {
  VectorStore,
  VectorStoreItem,
  VectorStoreMatch,
  CreateVectorStoreOptions
} from "./vector-store.js";

export { semanticRetriever } from "./semantic-retriever.js";
export type { SemanticRetrieverParams, SemanticRetrieverDoc } from "./semantic-retriever.js";

// Static catalog metadata backing the API's catalog endpoint (components / prebuilt
// agents / capability tiers).
export { componentCatalog, prebuiltCatalog, tierCatalog } from "./catalog.js";
export type {
  ComponentCatalogEntry,
  ComponentCategory,
  ComponentParamMeta,
  PrebuiltAgentCatalogEntry,
  ModelTierInfo
} from "./catalog.js";

// AI-readable surface (ADR DX batch 3): ground-truth for AI agents + run introspection.
export { generateLlmsTxt } from "./llms-txt-generator.js";
export { componentSchema, componentSchemas, paramTypeToJsonSchema } from "./schema-generator.js";
export type { ComponentSchema, JsonSchema } from "./schema-generator.js";
export { explainRun } from "./run-explainer.js";
export type { RunExplanation } from "./run-explainer.js";

// `ailu dev` — the local run inspector (watch a graph execute in the browser).
export { serveInspector } from "./dev-inspector.js";
export type { InspectorHandle, InspectorOptions } from "./dev-inspector.js";

export { rustValidatorActive } from "./rust-validator.js";
export { rustEngineAvailable } from "./rust-engine.js";

// Observability (ADR 0028 phase 7): export a run's lifecycle events as OTLP traces (to
// LangSmith / Langfuse / Phoenix / any OTel endpoint) + a token-usage → cost mapping.
export {
  exportTracesToOtlp,
  buildOtlpPayload,
  computeCost,
  DEFAULT_PRICE_BOOK
} from "./observability.js";
export type {
  OtelExporterOptions,
  OtlpFetch,
  PriceBook,
  ModelPrice,
  TokenUsage
} from "./observability.js";
// The approve/resume provenance wire shape `{ name, requestedBy, resolvedBy }` the Rust
// guard-rail validates — the control plane builds it from ApprovalEngine decisions.
export type { ApprovedToolWire } from "./rust-engine.js";

// Execute a *catalog* graph (a plain GraphDefinition whose nodes carry the shared
// `node.metadata.component` / `node.metadata.agent` carrier) on the Rust engine. This
// is the seam the control plane uses to run graphs authored in the Studio editor.
export {
  runCatalogGraph,
  resumeCatalogGraph,
  replayCatalogGraph,
  isCatalogGraph,
  readComponentCarrier,
  readAgentCarrier,
  readMapAgentCarrier,
  RustEngineUnavailableError,
  GATE_SUBJECT_PREFIX
} from "./run-catalog-graph.js";

// Replay-as-evidence (ADR 0038): the faithfulness check — a deterministic replay must reproduce
// the SAME governed decisions the run was attested for. Separate from `verifyChain` (tamper-evidence).
export { verifyReplayDecisions } from "./verify-replay.js";
export type { ReplayDecision, VerifyReplayResult } from "./verify-replay.js";
export type {
  CatalogRunOutcome,
  RunCatalogGraphOptions,
  ComponentCarrier,
  AgentCarrier,
  MapAgentCarrier
} from "./run-catalog-graph.js";

export type {
  ChannelValues,
  EmptyChannels,
  TypedGraphState,
  TypedNodeHandler,
  TypedCondition,
  ChannelUpdate,
  InitialData
} from "./typed.js";

export {
  createAgentNodeHandler,
  createToolNodeHandler,
  streamAgentTokens,
  toRustAgentConfig,
  toAgentApprovalBinding,
  GOVERNANCE_MIDDLEWARE_KINDS,
  DEFAULT_AGENT_OUTPUT_CHANNEL,
  APPROVED_TOOLS_CHANNEL,
  APPROVAL_IDS_CHANNEL,
  AGENT_APPROVAL_INTERRUPT
} from "./agent-node.js";
export type {
  AgentApprovalBinding,
  AgentNodeConfig,
  AgentProfile,
  AgentPromptSource,
  EfficiencyMiddlewareSpec,
  FsPermVerb,
  FsPolicyRule,
  MapAgentNodeConfig,
  RustAgentConfig,
  RustMapAgentConfig,
  RustToolBinding,
  RustToolSpec,
  SkillConfig,
  SkillRecord,
  StreamAgentConfig,
  TaskNodeConfig,
  ToolNodeConfig
} from "./agent-node.js";

// Deep-agent harness phase 1 (ADR 0022/0023): the writeTodos planning tool's shared
// shapes, re-exported so SDK consumers can declare the todos channel + read the list.
export {
  normalizeTodos,
  writeTodosTool,
  writeTodosJsonSchema,
  TODOS_CHANNEL,
  WRITE_TODOS_TOOL_NAME
} from "@ailu-ai/agents-core";
export type { TodoItem, TodoStatus, WriteTodosInput } from "@ailu-ai/agents-core";

export {
  AiluSdkError,
  GraphCompileError,
  DuplicateNodeError,
  MissingHandlerError,
  UnknownNodeError,
  GovernanceMiddlewareRejectedError,
  ResumeStateNotFoundError,
  ApproverRequiredError,
  ApprovalNotGrantedError
} from "./errors.js";
export type { Result } from "./errors.js";

// Re-export the engine types most callers reach for, so a single import suffices.
export type {
  Command,
  GraphDefinition,
  GraphState,
  GraphStatus,
  NodeId,
  RunId,
  ChannelReducer
} from "@ailu-ai/graph-core";
export type { Message, AIMessage, ToolCall, MessageId } from "@ailu-ai/graph-core";
export type { ConditionFn, NodeHandler, RunEvent } from "@ailu-ai/graph-runtime";
export type { StreamEvent, StreamMode } from "@ailu-ai/graph-runtime";

// Advanced wiring for callers who want durable checkpoints / custom buses. The
// Postgres-backed adapters live in the PRIVATE `@ailu-ai/db-adapters` package and are
// intentionally NOT re-exported here, so the public SDK bundle never embeds the DB
// schema. Bring your own `Checkpointer` (the interface is exported above) or import
// the Pg adapters from `@ailu-ai/db-adapters` in private/control-plane code.
export { InMemoryCheckpointer, DynamicInterrupt } from "@ailu-ai/graph-runtime";

// Building blocks for agent/tool nodes, re-exported so a single import suffices. ADR 0034 (16a):
// these stay re-exported (back-compat), but `@anthropic-ai/sdk` is now lazy-loaded inside the
// Anthropic adapter + dropped from this package's deps — so `pnpm add @ailu-ai/graph-sdk` no
// longer pulls a provider SDK. The Rust engine is the real execution path; this TS gateway is the
// deprecated fallback.
export {
  DefaultLLMGateway,
  MockLLMProviderAdapter,
  AnthropicProviderAdapter,
  OpenAICompatibleProviderAdapter,
  InMemoryPromptRegistry,
  ModelPolicy,
  MODEL_TIERS,
  DEFAULT_TIER_TABLE,
  DEFAULT_PREFERENCE
} from "@ailu-ai/llm-gateway";
export type {
  LLMGateway,
  LLMProvider,
  LLMResponse,
  LLMStreamChunk,
  LLMToolCall,
  PromptRegistry,
  OpenAICompatibleAdapterOptions,
  OpenAICompatibleTransportPort,
  OpenAIChatRequestBody,
  OpenAIChatResponse,
  ModelTier,
  ModelChoice,
  TierModelTable,
  ResolveOverride
} from "@ailu-ai/llm-gateway";
export { InMemoryToolRegistry } from "@ailu-ai/agents-core";
export type { ToolRegistry, ToolDefinition, ToolId, AgentResult } from "@ailu-ai/agents-core";
export { finalAnswer } from "./final-answer.js";

// ── ADR 0037: the product consumes the engine through this one door ───────────────────────────
// Additive re-exports so the control plane imports engine surface from `@ailu-ai/graph-sdk`
// instead of the (unpublished) engine internals. tsup INLINES every package below, so these add
// nothing to publish — the published residual stays {graph-sdk, contracts, napi, config}. Identity
// is preserved (the same inlined source), so `implements` in the control plane keeps type-checking.

// graph-core — graph-definition types + the validator.
export { validateGraph, GraphStateSchema, GraphValidationError } from "@ailu-ai/graph-core";
export type {
  NodeType,
  NodeDefinition,
  EdgeDefinition,
  EdgeId,
  GraphId
} from "@ailu-ai/graph-core";

// graph-runtime — engine primitives + checkpoint/interrupt types.
export {
  GraphRuntime,
  InMemoryConditionRegistry,
  InMemoryEventBus,
  InMemoryNodeRegistry
} from "@ailu-ai/graph-runtime";
export type {
  Checkpointer,
  Checkpoint,
  CheckpointId,
  InterruptConfig
} from "@ailu-ai/graph-runtime";

// agents-core — the ReAct agent (the control plane builds governed agents over it).
export { ReActAgent } from "@ailu-ai/agents-core";
export type { AgentId } from "@ailu-ai/agents-core";

// llm-gateway — adapter/request types for extraction services + custom adapters.
export type { LLMModel, LLMProviderAdapter, LLMRequest } from "@ailu-ai/llm-gateway";

// Governed seams (ADR 0037 D3) — the interfaces the control plane's Pg* adapters implement, plus the
// in-memory defaults + the Ed25519 attestor. This WIDENS the public governance/storage surface
// deliberately (mandatory-review): the engine's already-public interface set, additive, in-bundle.
export {
  InMemoryApprovalEngine,
  Ed25519Attestor,
  canonicalJson,
  verifyAttestation,
  verifyChain,
  ApprovalSelfApprovalError,
  ApprovalAlreadyResolvedError,
  ApprovalNotFoundError
} from "@ailu-ai/approval-engine";
export type {
  ApprovalEngine,
  ApprovalId,
  ApprovalRequest,
  RequestApprovalParams,
  AttestationRecord
} from "@ailu-ai/approval-engine";
export { InMemoryArtifactStore } from "@ailu-ai/artifact-store";
export type {
  ArtifactStore,
  Artifact,
  ArtifactId,
  ArtifactVersion
} from "@ailu-ai/artifact-store";

// DSL compilers (graph-ailu + lang-ailu) — re-exported as the sanctioned YAML-string compile
// entry point (their @deprecated notes, ADR 0003, say "compile via @ailu-ai/graph-sdk"). Bundled
// here (pure TS + js-yaml) so they run in the BROWSER too — the Studio compiles/previews YAML
// client-side, where the napi addon cannot run. `compileGraphFile` = graph YAML → GraphDefinition;
// `compileFile` = prompt/agent/chain YAML. (Server code may still prefer napi `compileGraphYamlJson`.)
export { compileGraphFile } from "@ailu-ai/graph-ailu";
export { compileFile } from "@ailu-ai/lang-ailu";

// search + memory-store — inlined (zero @ailu-ai deps); the control plane uses them directly.
export { InMemorySearchProvider, DEFAULT_SEARCH_LIMIT } from "@ailu-ai/search";
export type {
  SearchProvider,
  SearchDocument,
  SearchHit,
  SearchResourceType,
  SearchQueryOptions
} from "@ailu-ai/search";
export type { BaseStore, MemoryNamespace, MemoryKey, MemoryItem } from "@ailu-ai/memory-store";

// ADR 0031: per-model provider overlays. Install a provider package for the concrete classes
// (`@ailu-ai/model-openai`, `-anthropic`, `-gemini`, `-mistral`); these shared base types +
// the OpenAI-compatible escape hatch are re-exported here for convenience.
export {
  Model,
  OpenAICompatibleModel,
  openaiCompatible,
  toModelSpec,
  parseModelString,
  assertKnownProvider,
  // ADR 0034 (16d): the unified `model` surface — the DX entry point.
  model,
  models,
  SpecModel,
  TypedModel,
  resolveProviderKeys,
  DEFAULT_KEY_ENV,
  UnknownProviderError,
  MissingProviderKeyError,
  NoProviderInEnvError
} from "@ailu-ai/model-core";
export type {
  ModelSpec,
  ModelLike,
  ProviderSlug,
  ProviderEntry,
  ModelOptions,
  OutputSchema,
  ResolvedKeys,
  ChatMessage,
  ModelResponse,
  ModelUsage,
  InvokeOptions
} from "@ailu-ai/model-core";
