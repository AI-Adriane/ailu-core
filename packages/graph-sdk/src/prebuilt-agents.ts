/**
 * Prebuilt, tier-tagged micro-agent graphs. Each factory returns a runnable
 * {@link CompiledGraph} pre-wired with its capability {@link ModelTier} (matching the
 * Rust `PrebuiltAgent` definitions in `crates/components`), so the concrete model is
 * resolved by the {@link ModelPolicy} from the providers actually available — on Rust
 * by the bridge, on the TS fallback path by the SDK.
 *
 * A prebuilt agent is a one-agent graph, except {@link prebuilt.ragAnswerer}, which
 * composes the `retriever` + `reranker` components with an agent step.
 *
 * ```ts
 * import { prebuilt } from "@ailu/graph-sdk";
 *
 * const result = await prebuilt.summarizer().run({ question: "long text…" });
 * console.log(result.channels.summary);
 * ```
 *
 * By default each agent runs on a deterministic mock gateway (registered under the
 * nominal provider) so a prebuilt graph runs end-to-end with no provider keys. Supply
 * `llm` to run against a real gateway, `model` to pin a concrete model, or
 * `tierOverride` to change the capability tier.
 */

import {
  DefaultLLMGateway,
  MockLLMProviderAdapter,
  type LLMGateway,
  type LLMProvider,
  type ModelTier
} from "@ailu/llm-gateway";
import { InMemoryToolRegistry, type ToolId, type ToolRegistry } from "@ailu/agents-core";

import { createGraph } from "./builder.js";
import { components, type RetrieverDoc } from "./components.js";
import type { CompiledGraph } from "./compiled-graph.js";

/** Light options accepted by every prebuilt-agent factory. */
export type PrebuiltOptions = {
  /**
   * The LLM gateway the agent runs on. Defaults to a deterministic mock gateway
   * registered under the nominal provider, so the graph runs with no provider keys.
   */
  llm?: LLMGateway;
  /** Override the capability tier the agent's model is resolved from. */
  tierOverride?: ModelTier;
  /**
   * Pin a concrete model, bypassing tier resolution (the explicit-override
   * precedence: an explicit model always wins over the tier).
   */
  model?: string;
  /**
   * The provider slot for the request (and the slot the default mock gateway
   * registers under). Defaults to `"anthropic"`. The actual adapter is the mock
   * unless a custom `llm` is supplied.
   */
  provider?: LLMProvider;
};

/** A prebuilt micro-agent definition mirrored from the Rust `PrebuiltAgent` table. */
type PrebuiltDef = {
  name: string;
  description: string;
  tier: ModelTier;
  systemPrompt: string;
  toolNames: string[];
  suspendForApproval: boolean;
  outputChannel: string;
};

/**
 * The prebuilt definitions, mirroring `crates/components/src/prebuilt.rs` exactly
 * (name, tier, system prompt, tools, suspend flag, output channel).
 */
const DEFS: Record<string, PrebuiltDef> = {
  summarizer: {
    name: "summarizer",
    description: "Condenses input text into a short, faithful summary.",
    tier: "fast",
    systemPrompt:
      "You are a precise summarizer. Read the user's text and produce a concise " +
      "summary that preserves the key facts and intent. Do not add information " +
      "that is not present. Respond with the summary only, no preamble.",
    toolNames: [],
    suspendForApproval: false,
    outputChannel: "summary"
  },
  classifier: {
    name: "classifier",
    description: "Assigns the input to exactly one label from a fixed set.",
    tier: "fast",
    systemPrompt:
      'You are a text classifier. Classify the user\'s input into exactly one of ' +
      'the following labels: "positive", "negative", "neutral", "question", ' +
      '"spam". Respond with the single label only, lowercase, no punctuation or ' +
      "explanation.",
    toolNames: [],
    suspendForApproval: false,
    outputChannel: "label"
  },
  extractor: {
    name: "extractor",
    description: "Extracts structured fields from unstructured text as JSON.",
    tier: "fast",
    systemPrompt:
      "You extract structured data from text. Return a single JSON object that " +
      'matches this schema: { "name": string | null, "email": string | null, ' +
      '"organization": string | null, "intent": string | null }. Use null for ' +
      "any field not present in the text. Respond with the JSON object only, no " +
      "code fences and no commentary.",
    toolNames: [],
    suspendForApproval: false,
    outputChannel: "extracted"
  },
  sqlGenerator: {
    name: "sqlGenerator",
    description: "Generates a SQL query from a natural-language request and schema.",
    tier: "balanced",
    systemPrompt:
      "You are a SQL generator. Given a natural-language request and an optional " +
      "schema description, produce a single, syntactically valid SQL query that " +
      "answers the request. Prefer standard SQL. Do not modify data unless the " +
      "request explicitly asks for it. Respond with the SQL query only, no code " +
      "fences and no explanation.",
    toolNames: [],
    suspendForApproval: false,
    outputChannel: "sql"
  },
  ragAnswerer: {
    name: "ragAnswerer",
    description:
      "Answers a question grounded in retrieved documents. Composed as a graph: " +
      "the `retriever` component fetches candidate documents, the `reranker` " +
      "component reorders them, and this agent step writes a grounded answer " +
      "citing the supplied context.",
    tier: "balanced",
    systemPrompt:
      "You are a retrieval-augmented answerer. You are given a question and a set " +
      "of retrieved context passages. Answer the question using only the " +
      "provided context. If the context does not contain the answer, say you do " +
      "not know. Cite the passage ids you relied on. Respond with the answer " +
      "only.",
    toolNames: [],
    suspendForApproval: false,
    outputChannel: "answer"
  },
  refundApprover: {
    name: "refundApprover",
    description:
      "Decides whether to issue a refund and routes the action through a human " +
      "approval gate before calling the `refund` tool.",
    tier: "balanced",
    systemPrompt:
      "You are a refund assistant. Review the customer's request and the order " +
      "details, decide whether a refund is warranted, and if so prepare a call " +
      "to the `refund` tool with the order id and amount. You may not issue a " +
      "refund without human approval. Explain your reasoning before requesting " +
      "the tool.",
    toolNames: ["refund"],
    suspendForApproval: true,
    outputChannel: "refundDecision"
  },
  translator: {
    name: "translator",
    description: "Translates the input text into a target language, preserving meaning.",
    tier: "fast",
    systemPrompt:
      "You are a translator. Translate the user's text into the requested target " +
      "language, preserving meaning, tone, and formatting. If no target language " +
      "is given, translate into English. Do not add explanations. Respond with " +
      "the translated text only.",
    toolNames: [],
    suspendForApproval: false,
    outputChannel: "translation"
  },
  sentimentAnalyzer: {
    name: "sentimentAnalyzer",
    description: "Classifies the emotional tone of the input text.",
    tier: "fast",
    systemPrompt:
      "You are a sentiment analyzer. Read the user's text and classify its " +
      'overall sentiment as exactly one of: "positive", "negative", "neutral", ' +
      '"mixed". Respond with the single label only, lowercase, no punctuation or ' +
      "explanation.",
    toolNames: [],
    suspendForApproval: false,
    outputChannel: "sentiment"
  },
  entityExtractor: {
    name: "entityExtractor",
    description: "Extracts named entities from text as a JSON array.",
    tier: "fast",
    systemPrompt:
      "You extract named entities from text. Return a single JSON array where " +
      'each element is an object { "text": string, "type": one of "person" | ' +
      '"organization" | "location" | "date" | "other" }. Return an empty array if ' +
      "there are no entities. Respond with the JSON array only, no code fences and " +
      "no commentary.",
    toolNames: [],
    suspendForApproval: false,
    outputChannel: "entities"
  },
  piiRedactor: {
    name: "piiRedactor",
    description: "Redacts personally identifiable information from the input text.",
    tier: "fast",
    systemPrompt:
      "You are a PII redactor. Rewrite the user's text replacing any personally " +
      "identifiable information (names, emails, phone numbers, addresses, " +
      "government ids, payment details) with a bracketed placeholder such as " +
      "[REDACTED_EMAIL] or [REDACTED_NAME]. Preserve all other text exactly. " +
      "Respond with the redacted text only.",
    toolNames: [],
    suspendForApproval: false,
    outputChannel: "redacted"
  },
  intentClassifier: {
    name: "intentClassifier",
    description: "Maps the input to a single conversational intent label.",
    tier: "fast",
    systemPrompt:
      "You are an intent classifier. Classify the user's message into exactly " +
      'one intent label: "question", "request", "complaint", "feedback", ' +
      '"chitchat", "other". Respond with the single label only, lowercase, no ' +
      "punctuation or explanation.",
    toolNames: [],
    suspendForApproval: false,
    outputChannel: "intent"
  },
  titleGenerator: {
    name: "titleGenerator",
    description: "Generates a short, descriptive title for the input text.",
    tier: "fast",
    systemPrompt:
      "You generate titles. Read the user's text and produce a single concise, " +
      "descriptive title of at most ten words that captures its main topic. Use " +
      "title case. Respond with the title only, no quotation marks and no " +
      "explanation.",
    toolNames: [],
    suspendForApproval: false,
    outputChannel: "title"
  },
  keywordExtractor: {
    name: "keywordExtractor",
    description: "Extracts the key terms from the input text as a JSON array.",
    tier: "fast",
    systemPrompt:
      "You extract keywords. Read the user's text and return a single JSON array " +
      "of the most important keywords or key phrases (lowercase strings), most " +
      "significant first, with no duplicates. Respond with the JSON array only, " +
      "no code fences and no commentary.",
    toolNames: [],
    suspendForApproval: false,
    outputChannel: "keywords"
  },
  questionAnswerer: {
    name: "questionAnswerer",
    description: "Answers a question directly and concisely from its own knowledge.",
    tier: "balanced",
    systemPrompt:
      "You are a question answerer. Read the user's question and answer it " +
      "directly, accurately, and concisely. If you are not certain of the " +
      "answer, say so rather than guessing. Respond with the answer only.",
    toolNames: [],
    suspendForApproval: false,
    outputChannel: "answer"
  },
  codeReviewer: {
    name: "codeReviewer",
    description: "Reviews a code snippet or diff for correctness, security, and quality.",
    tier: "frontier",
    systemPrompt:
      "You are a senior code reviewer. Review the supplied code or diff for " +
      "correctness bugs, security issues, performance problems, and readability. " +
      "Be specific and reference concrete lines or constructs. Prioritise " +
      "high-impact findings; do not invent issues. Respond with a concise, " +
      "ordered list of findings, each with a short rationale and a suggested " +
      "fix.",
    toolNames: [],
    suspendForApproval: false,
    outputChannel: "review"
  },
  copyEditor: {
    name: "copyEditor",
    description: "Polishes prose for clarity, grammar, flow, and tone.",
    tier: "creative",
    systemPrompt:
      "You are a copy editor. Rewrite the user's text to improve clarity, " +
      "grammar, flow, and tone while preserving the original meaning and voice. " +
      "Do not change the language or invent facts. Respond with the edited text " +
      "only, no commentary.",
    toolNames: [],
    suspendForApproval: false,
    outputChannel: "edited"
  }
};

/** The provider slot a prebuilt agent uses unless overridden. */
const DEFAULT_PROVIDER: LLMProvider = "anthropic";

/**
 * The gateway a prebuilt agent runs on: the caller's `llm` if given, else a
 * deterministic mock gateway registered under the nominal provider (so the graph runs
 * end-to-end with no provider keys).
 *
 * For an approval-gated agent (e.g. `refundApprover`), the default mock scripts an
 * `ACTION: <tool> {}` turn so the agent reaches for its gated tool and the
 * suspend-for-approval gate fires — matching the Rust definition's intent. Other
 * agents get a plain final-answer turn.
 */
const resolveGateway = (options: PrebuiltOptions, provider: LLMProvider, def: PrebuiltDef): LLMGateway => {
  if (options.llm !== undefined) {
    return options.llm;
  }
  const firstTool = def.toolNames[0];
  const content =
    def.suspendForApproval && firstTool !== undefined ? `ACTION: ${firstTool} {}` : "mock-response";
  const gateway = new DefaultLLMGateway();
  gateway.registerAdapter(
    new MockLLMProviderAdapter({
      provider,
      response: { content, usage: { promptTokens: 0, completionTokens: 0 }, model: "mock", provider }
    })
  );
  return gateway;
};

/**
 * A tool registry for a prebuilt agent's `toolNames`. Each tool is a no-op stub whose
 * `requiresApproval` matches the definition's `suspendForApproval` (e.g. `refund`),
 * so the suspend-for-approval gate behaves the same as the Rust definition.
 */
const buildToolRegistry = (def: PrebuiltDef): ToolRegistry | undefined => {
  if (def.toolNames.length === 0) {
    return undefined;
  }
  const passthrough = { parse: (value: unknown) => value };
  const registry = new InMemoryToolRegistry();
  for (const name of def.toolNames) {
    registry.register(
      {
        id: name as ToolId,
        name,
        description: `${name} tool for the ${def.name} prebuilt agent`,
        inputSchema: passthrough,
        outputSchema: passthrough,
        permissions: [],
        requiresApproval: def.suspendForApproval,
        jsonSchema: { type: "object" }
      },
      async () => ({ ok: true })
    );
  }
  return registry;
};

/** Build the one-agent graph for a simple prebuilt definition. */
const buildAgentGraph = (def: PrebuiltDef, options: PrebuiltOptions): CompiledGraph => {
  const provider = options.provider ?? DEFAULT_PROVIDER;
  const tier = options.tierOverride ?? def.tier;
  return createGraph({ name: `prebuilt-${def.name}` })
    .agentNode(def.name, {
      llm: resolveGateway(options, provider, def),
      prompt: { system: def.systemPrompt },
      provider,
      tier,
      model: options.model,
      tools: buildToolRegistry(def),
      suspendForApproval: def.suspendForApproval,
      name: def.name,
      description: def.description,
      outputChannel: def.outputChannel
    })
    .compile();
};

/** Default corpus for {@link prebuilt.ragAnswerer} when the caller supplies none. */
const DEFAULT_RAG_DOCS: RetrieverDoc[] = [
  { id: "d1", content: "Ailu is a stateful, resumable agent graph runtime." },
  { id: "d2", content: "The runtime checkpoints after every node and emits lifecycle events." },
  { id: "d3", content: "Human-gate nodes suspend the run cleanly for approval." }
];

/** Options for {@link prebuilt.ragAnswerer}: the simple options plus its retrieval corpus. */
export type RagAnswererOptions = PrebuiltOptions & {
  /** The corpus the retriever scores against. Defaults to a small built-in set. */
  docs?: RetrieverDoc[];
  /** How many documents the retriever keeps before reranking. Defaults to 4. */
  k?: number;
  /** Channel the question is read from (the retriever query). Defaults to `"question"`. */
  questionChannel?: string;
};

/**
 * Build the composed RAG graph: a `retriever` component fetches candidate documents,
 * a `reranker` component reorders them against the question, and an agent step writes
 * a grounded answer. Wired as `retrieve -> rerank -> answer`.
 */
const buildRagGraph = (options: RagAnswererOptions): CompiledGraph => {
  const def = DEFS.ragAnswerer!;
  const provider = options.provider ?? DEFAULT_PROVIDER;
  const tier = options.tierOverride ?? def.tier;
  const questionChannel = options.questionChannel ?? "question";
  const docs = options.docs ?? DEFAULT_RAG_DOCS;

  return createGraph({ name: "prebuilt-ragAnswerer" })
    .channel(questionChannel, { type: "string", default: "" })
    .channel("retrieved", { type: "json", default: [] })
    .channel("ranked", { type: "json", default: [] })
    .component(
      "retrieve",
      components.retriever({ query: questionChannel, into: "retrieved", k: options.k ?? 4, docs })
    )
    .component(
      "rerank",
      components.reranker({ from: "retrieved", into: "ranked", query: questionChannel })
    )
    .agentNode("answer", {
      llm: resolveGateway(options, provider, def),
      prompt: { system: def.systemPrompt },
      provider,
      tier,
      model: options.model,
      name: def.name,
      description: def.description,
      outputChannel: def.outputChannel
    })
    .edge("retrieve", "rerank")
    .edge("rerank", "answer")
    .entry("retrieve")
    .compile();
};

/**
 * The prebuilt micro-agent surface. Each factory returns a runnable
 * {@link CompiledGraph} pre-wired with the agent's tier (matching the Rust
 * `PrebuiltAgent` definitions).
 */
export const prebuilt = {
  /** Condense input text into a short, faithful summary (writes `summary`). */
  summarizer(options: PrebuiltOptions = {}): CompiledGraph {
    return buildAgentGraph(DEFS.summarizer!, options);
  },
  /** Assign the input to one label from a fixed set (writes `label`). */
  classifier(options: PrebuiltOptions = {}): CompiledGraph {
    return buildAgentGraph(DEFS.classifier!, options);
  },
  /** Extract structured fields from text as JSON (writes `extracted`). */
  extractor(options: PrebuiltOptions = {}): CompiledGraph {
    return buildAgentGraph(DEFS.extractor!, options);
  },
  /** Generate a SQL query from a natural-language request (writes `sql`). */
  sqlGenerator(options: PrebuiltOptions = {}): CompiledGraph {
    return buildAgentGraph(DEFS.sqlGenerator!, options);
  },
  /** Answer a question grounded in retrieved documents (retriever + reranker + agent). */
  ragAnswerer(options: RagAnswererOptions = {}): CompiledGraph {
    return buildRagGraph(options);
  },
  /** Decide on a refund, gated behind human approval before the `refund` tool runs. */
  refundApprover(options: PrebuiltOptions = {}): CompiledGraph {
    return buildAgentGraph(DEFS.refundApprover!, options);
  },
  /** Translate input text into a target language (fast; writes `translation`). */
  translator(options: PrebuiltOptions = {}): CompiledGraph {
    return buildAgentGraph(DEFS.translator!, options);
  },
  /** Classify the emotional tone of the input (fast; writes `sentiment`). */
  sentimentAnalyzer(options: PrebuiltOptions = {}): CompiledGraph {
    return buildAgentGraph(DEFS.sentimentAnalyzer!, options);
  },
  /** Extract named entities from text as a JSON array (fast; writes `entities`). */
  entityExtractor(options: PrebuiltOptions = {}): CompiledGraph {
    return buildAgentGraph(DEFS.entityExtractor!, options);
  },
  /** Redact personally identifiable information from text (fast; writes `redacted`). */
  piiRedactor(options: PrebuiltOptions = {}): CompiledGraph {
    return buildAgentGraph(DEFS.piiRedactor!, options);
  },
  /** Map the input to a single conversational intent label (fast; writes `intent`). */
  intentClassifier(options: PrebuiltOptions = {}): CompiledGraph {
    return buildAgentGraph(DEFS.intentClassifier!, options);
  },
  /** Generate a short, descriptive title for the input (fast; writes `title`). */
  titleGenerator(options: PrebuiltOptions = {}): CompiledGraph {
    return buildAgentGraph(DEFS.titleGenerator!, options);
  },
  /** Extract the key terms from the input as a JSON array (fast; writes `keywords`). */
  keywordExtractor(options: PrebuiltOptions = {}): CompiledGraph {
    return buildAgentGraph(DEFS.keywordExtractor!, options);
  },
  /** Answer a question directly and concisely (balanced; writes `answer`). */
  questionAnswerer(options: PrebuiltOptions = {}): CompiledGraph {
    return buildAgentGraph(DEFS.questionAnswerer!, options);
  },
  /** Review a code snippet or diff for correctness and quality (frontier; writes `review`). */
  codeReviewer(options: PrebuiltOptions = {}): CompiledGraph {
    return buildAgentGraph(DEFS.codeReviewer!, options);
  },
  /** Polish prose for clarity, grammar, flow, and tone (creative; writes `edited`). */
  copyEditor(options: PrebuiltOptions = {}): CompiledGraph {
    return buildAgentGraph(DEFS.copyEditor!, options);
  }
} as const;
