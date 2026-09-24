// Ailu MCP server — runs the production TypeScript engine (@ailu/graph-sdk)
// in-process so Claude Code can EXECUTE governed agents and graphs, not just
// validate them. The full governance loop is real: an agent that reaches for a
// sensitive tool suspends the run (`status: "suspended"`), surfaces a pending
// approval, and only executes the tool after a human grants it on resume.
//
// Tools exposed over stdio:
//   - list_agents()                          — the predefined engine-native agent registry.
//   - run_agent({ agent, input })            — build the agent's graph via the SDK and run it.
//   - approve_and_resume({ runId, ... })     — grant approval and resume a suspended run.
//   - run_graph({ graph, input })            — run a predefined SDK graph by name.
//   - validate_graph({ definitionJson })     — structural validation (wraps @ailu/napi).
//   - compile_graph_yaml({ yaml })           — compile graph DSL YAML (wraps @ailu/napi).
//
// This file is TypeScript (.mts) and is launched under `tsx` so it can import the
// workspace TS sources of @ailu/graph-sdk directly (see plugin/mcp/tsconfig.json
// + tsconfig.base.json path aliases). The Rust validate/compile path stays on the
// prebuilt @ailu/napi addon.

import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  CallToolRequestSchema,
  ListResourcesRequestSchema,
  ListToolsRequestSchema,
  ReadResourceRequestSchema
} from "@modelcontextprotocol/sdk/types.js";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

// --- Run ON RUST by default, independent of the inherited-env whitelist -----------
//
// The MCP stdio transport only forwards a whitelist of env vars to this child, so an
// external `AILU_SDK_ENGINE=rust` set by the launcher does NOT reach us. We opt in
// here so agent/graph tools route to the Rust engine (Phase G already defaults agent
// graphs to Rust under `auto` when the @ailu/napi addon is present; this makes the
// choice explicit and whitelist-proof). An explicit caller value is never overridden.
process.env.AILU_SDK_ENGINE = process.env.AILU_SDK_ENGINE ?? "rust";

// --- Load the repo .env so the Rust LLM gateway sees a provider key ----------------
//
// The Rust agent path builds its own gateway from env (Mistral / Anthropic / Ollama /
// a deterministic mock). Under the stdio whitelist those keys won't be inherited, so
// we read the repo root .env with a tiny hand-rolled parser (no dotenv dependency) and
// populate any MISSING key — we never clobber a value already in the environment, and
// we NEVER log a value. `MISTRAL_ECHO_KEY` is mapped onto `MISTRAL_API_KEY` (the name
// the gateway selection reads) when the latter is unset. With no key present the Rust
// engine falls back to its deterministic offline mock, so the server still runs.
function loadRepoEnv(): void {
  // Smoke/offline guard: when set, do NOT pull provider keys from .env. The Rust engine
  // then falls back to its deterministic offline mock, so run_agent/run_graph behave
  // reproducibly (a live model's tool choices are non-deterministic). Routing is still
  // Rust — this only changes which gateway the Rust agent path builds.
  if (process.env.AILU_MCP_SMOKE_OFFLINE === "1") {
    return;
  }
  try {
    const here = dirname(fileURLToPath(import.meta.url));
    // plugin/mcp/server.mts -> repo root is two levels up.
    const envPath = join(here, "..", "..", ".env");
    const raw = readFileSync(envPath, "utf8");
    for (const rawLine of raw.split(/\r?\n/)) {
      const line = rawLine.trim();
      if (line.length === 0 || line.startsWith("#")) continue;
      const eq = line.indexOf("=");
      if (eq === -1) continue;
      const key = line.slice(0, eq).trim();
      if (key.length === 0) continue;
      let value = line.slice(eq + 1).trim();
      // Strip a single layer of matching surrounding quotes.
      if (
        value.length >= 2 &&
        ((value.startsWith('"') && value.endsWith('"')) ||
          (value.startsWith("'") && value.endsWith("'")))
      ) {
        value = value.slice(1, -1);
      }
      if (process.env[key] === undefined || process.env[key] === "") {
        process.env[key] = value;
      }
    }
    // Map the repo's Mistral echo key onto the name the gateway selection reads.
    if (
      (process.env.MISTRAL_API_KEY === undefined || process.env.MISTRAL_API_KEY === "") &&
      typeof process.env.MISTRAL_ECHO_KEY === "string" &&
      process.env.MISTRAL_ECHO_KEY.length > 0
    ) {
      process.env.MISTRAL_API_KEY = process.env.MISTRAL_ECHO_KEY;
    }
  } catch {
    // No .env (or unreadable) — fine; the Rust engine uses its deterministic mock.
  }
}
loadRepoEnv();

import {
  createGraph,
  componentSchemas,
  DefaultLLMGateway,
  generateLlmsTxt,
  MockLLMProviderAdapter,
  AnthropicProviderAdapter,
  OpenAICompatibleProviderAdapter,
  InMemoryToolRegistry,
  type AgentResult,
  type CompiledGraph,
  type LLMGateway,
  type LLMProvider,
  type LLMResponse,
  type RunId,
  type ToolId
} from "@ailu/graph-sdk";
// The in-memory approval engine and its types come from the public package index.
// The Pg adapter (and its `db`/`pg` chain) now lives in the PRIVATE `@ailu/db-adapters`
// package and is no longer re-exported here, so importing the package index is safe.
import { InMemoryApprovalEngine } from "@ailu/approval-engine";
import type { ApprovalId, ApprovalRequest } from "@ailu/approval-engine";

// --- @ailu/napi (Rust engine) — kept for validate / compile -----------------

type RustEngine = {
  validateGraphJson: (definitionJson: string) => string;
  compileGraphYamlJson: (yaml: string) => string;
};

let cachedEngine: { engine: RustEngine } | { error: string } | undefined;

/** Lazily load @ailu/napi; memoize the outcome so failure is reported, not thrown. */
async function loadEngine(): Promise<{ engine: RustEngine } | { error: string }> {
  if (cachedEngine !== undefined) return cachedEngine;
  try {
    const mod = (await import("@ailu/napi")) as unknown as {
      default?: Partial<RustEngine>;
    } & Partial<RustEngine>;
    const engine = (mod?.default ?? mod) as Partial<RustEngine>;
    if (
      typeof engine?.validateGraphJson !== "function" ||
      typeof engine?.compileGraphYamlJson !== "function"
    ) {
      cachedEngine = {
        error: "@ailu/napi loaded but is missing validateGraphJson/compileGraphYamlJson."
      };
    } else {
      cachedEngine = { engine: engine as RustEngine };
    }
  } catch (error) {
    cachedEngine = {
      error: `Failed to load @ailu/napi (the Rust engine native addon). Build it with the repo's napi build, then pnpm install. Cause: ${
        error instanceof Error ? error.message : String(error)
      }`
    };
  }
  return cachedEngine;
}

// --- MCP result helpers --------------------------------------------------------

type ToolResult = { content: Array<{ type: "text"; text: string }>; isError?: boolean };

/** Shape an MCP tool result. `isError` marks failures so the client surfaces them. */
function textResult(text: string, isError = false): ToolResult {
  return { content: [{ type: "text", text }], isError };
}

/** Serialize a JSON payload into an MCP text result. */
function jsonResult(payload: unknown, isError = false): ToolResult {
  return textResult(JSON.stringify(payload, null, 2), isError);
}

// --- Gateway: REAL when a provider is configured, deterministic MOCK otherwise --

/**
 * THE env-driven switch, in precedence order:
 *
 *   1. ANTHROPIC_API_KEY set  -> AnthropicProviderAdapter (best; reads the key via the SDK).
 *   2. MISTRAL_API_KEY set    -> OpenAICompatibleProviderAdapter.mistral (Mistral cloud).
 *   3. AILU_USE_OLLAMA=1   -> OpenAICompatibleProviderAdapter.ollama (local Ollama at
 *                                AILU_LLM_BASE_URL or http://localhost:11434/v1, keyless).
 *   4. otherwise              -> MockLLMProviderAdapter replaying the scripted sequence.
 *
 * The Mistral/Ollama paths share ONE adapter (both speak OpenAI /v1/chat/completions)
 * registered under the `mistral` provider key. To keep routing consistent, the
 * selection also reports the `provider` + `model` the agent nodes must request — an
 * agent that requested `anthropic` would never reach the `mistral`-keyed adapter. The
 * model defaults to the agent's own provider default unless AILU_LLM_MODEL is set.
 *
 * The mock adapter is STATEFUL: it advances one scripted response per `complete()`
 * call and repeats the last once exhausted. A run that suspends and later resumes
 * must therefore reuse the SAME gateway instance — callers build the gateway once
 * per run lifecycle and keep it (see the suspended-run registry below).
 */
type GatewaySelection = {
  gateway: LLMGateway;
  /** Provider the agent nodes must request so the request routes to the live adapter. */
  provider: LLMProvider;
  /** Model the agent nodes request, or undefined to use the agent's own default. */
  model?: string;
  /** Human-readable mode for list_agents. */
  mode: string;
};

const envSet = (name: string): boolean => {
  const value = process.env[name];
  return typeof value === "string" && value.trim().length > 0;
};

function hasRealKey(): boolean {
  return envSet("ANTHROPIC_API_KEY");
}

function makeGateway(script: LLMResponse[]): GatewaySelection {
  const gateway = new DefaultLLMGateway();
  const model = envSet("AILU_LLM_MODEL") ? process.env.AILU_LLM_MODEL : undefined;

  if (hasRealKey()) {
    // The adapter's default port constructs `new Anthropic({})`, which reads
    // ANTHROPIC_API_KEY from the environment. Real reasoning; scripts are ignored.
    gateway.registerAdapter(new AnthropicProviderAdapter());
    return {
      gateway,
      provider: "anthropic",
      ...(model !== undefined ? { model } : {}),
      mode: "real (ANTHROPIC_API_KEY present)"
    };
  }

  if (envSet("MISTRAL_API_KEY")) {
    // Mistral cloud over the OpenAI-compatible adapter, registered under `mistral`.
    gateway.registerAdapter(
      OpenAICompatibleProviderAdapter.mistral(process.env.MISTRAL_API_KEY, model)
    );
    return {
      gateway,
      provider: "mistral",
      ...(model !== undefined ? { model } : {}),
      mode: "real (Mistral cloud via MISTRAL_API_KEY)"
    };
  }

  if (envSet("AILU_USE_OLLAMA")) {
    // Local Ollama (keyless) — same OpenAI-compatible adapter, registered under `mistral`.
    const baseUrl = envSet("AILU_LLM_BASE_URL") ? process.env.AILU_LLM_BASE_URL : undefined;
    gateway.registerAdapter(OpenAICompatibleProviderAdapter.ollama(model, baseUrl));
    return {
      gateway,
      provider: "mistral",
      ...(model !== undefined ? { model } : {}),
      mode: `real (local Ollama at ${baseUrl ?? "http://localhost:11434/v1"})`
    };
  }

  gateway.registerAdapter(new MockLLMProviderAdapter({ provider: "anthropic", responses: script }));
  return { gateway, provider: "anthropic", mode: "mock (deterministic, offline)" };
}

/** Compute the current gateway mode string for list_agents without a live run. */
function currentMode(): string {
  if (hasRealKey()) return "real (ANTHROPIC_API_KEY present)";
  if (envSet("MISTRAL_API_KEY")) return "real (Mistral cloud via MISTRAL_API_KEY)";
  if (envSet("AILU_USE_OLLAMA")) {
    const baseUrl = envSet("AILU_LLM_BASE_URL") ? process.env.AILU_LLM_BASE_URL : undefined;
    return `real (local Ollama at ${baseUrl ?? "http://localhost:11434/v1"})`;
  }
  return "mock (deterministic, offline)";
}

// Scripted-turn builders mirror the SDK examples/tests.
let toolUseSeq = 0;
const toolTurn = (name: string, input: Record<string, unknown> = {}): LLMResponse => ({
  content: "",
  toolCalls: [{ id: `tu_${(toolUseSeq += 1)}`, name, input }],
  stopReason: "tool_use",
  usage: { promptTokens: 0, completionTokens: 0 },
  model: "mock",
  provider: "anthropic"
});
const finalTurn = (content: string): LLMResponse => ({
  content,
  usage: { promptTokens: 0, completionTokens: 0 },
  model: "mock",
  provider: "anthropic"
});

const passthrough = { parse: (value: unknown) => value };

// --- Predefined engine-native agents -------------------------------------------

/**
 * Each agent builds a real {@link CompiledGraph} from an {@link agentNode}. The
 * factory receives the live gateway (real or mock) plus a fresh ApprovalEngine so
 * that gated agents can file/resolve approvals through the engine. `script` is the
 * deterministic mock sequence consumed only when no API key is set.
 */
type AgentEntry = {
  name: string;
  description: string;
  hasGatedTools: boolean;
  /** Deterministic mock sequence (ignored when a real key drives the model). */
  script: LLMResponse[];
  build: (selection: GatewaySelection, approvalEngine: InMemoryApprovalEngine) => CompiledGraph;
};

/**
 * The provider/model fields every agent node sets so the request routes to the live
 * adapter the selected gateway registered (Anthropic vs the `mistral`-keyed
 * OpenAI-compatible adapter). Omitting `model` lets the node keep its own default.
 */
const routing = (selection: GatewaySelection): { provider: LLMProvider; model?: string } => ({
  provider: selection.provider,
  ...(selection.model !== undefined ? { model: selection.model } : {})
});

// (a) researcher — two ungated tools (search/fetch) over a tiny inline corpus; ends
// with a cited answer. No approval gate.
const CORPUS: Record<string, { title: string; body: string }> = {
  "doc-1": {
    title: "Ailu governance model",
    body: "Ailu suspends a run on a sensitive tool and resumes only after a human approves; agents never self-approve."
  },
  "doc-2": {
    title: "Checkpointing",
    body: "Every node completion is checkpointed, so any run is resumable from its latest checkpoint."
  }
};

function buildResearcher(selection: GatewaySelection): CompiledGraph {
  const gateway = selection.gateway;
  const tools = new InMemoryToolRegistry();
  tools.register(
    {
      id: "search" as ToolId,
      name: "search",
      description: "Search the inline corpus; returns matching doc ids.",
      inputSchema: passthrough,
      outputSchema: passthrough,
      permissions: [],
      jsonSchema: { type: "object", properties: { query: { type: "string" } } }
    },
    async (input: unknown) => {
      const query = String((input as { query?: unknown })?.query ?? "").toLowerCase();
      const hits = Object.entries(CORPUS)
        .filter(
          ([, doc]) =>
            doc.title.toLowerCase().includes(query) || doc.body.toLowerCase().includes(query)
        )
        .map(([id]) => id);
      return { hits: hits.length > 0 ? hits : Object.keys(CORPUS) };
    }
  );
  tools.register(
    {
      id: "fetch" as ToolId,
      name: "fetch",
      description: "Fetch a corpus document by id.",
      inputSchema: passthrough,
      outputSchema: passthrough,
      permissions: [],
      jsonSchema: { type: "object", properties: { id: { type: "string" } } }
    },
    async (input: unknown) => {
      const id = String((input as { id?: unknown })?.id ?? "");
      const doc = CORPUS[id];
      return doc ? { id, ...doc } : { id, error: "not_found" };
    }
  );

  return createGraph({ name: "researcher" })
    .agentNode("researcher", {
      llm: gateway,
      ...routing(selection),
      prompt: {
        system:
          "You are a research agent. Use `search` to find relevant docs, `fetch` to read them, " +
          "then answer with a citation of the doc id. End with FINAL: <answer> [cite: <id>]."
      },
      tools,
      maxIterations: 6
    })
    .compile();
}

// (b) refunder — a GATED `refund` tool (requiresApproval) + suspendForApproval + an
// ApprovalEngine. Demonstrates the human-gate: suspends, then executes on approval.
function buildRefunder(
  selection: GatewaySelection,
  approvalEngine: InMemoryApprovalEngine
): CompiledGraph {
  const gateway = selection.gateway;
  const tools = new InMemoryToolRegistry();
  tools.register(
    {
      id: "lookup_order" as ToolId,
      name: "lookup_order",
      description: "Look up a customer order (ungated).",
      inputSchema: passthrough,
      outputSchema: passthrough,
      permissions: [],
      jsonSchema: { type: "object", properties: { orderId: { type: "string" } } }
    },
    async (input: unknown) => ({
      orderId: String((input as { orderId?: unknown })?.orderId ?? "ord-1"),
      amount: 49.0,
      currency: "EUR"
    })
  );
  tools.register(
    {
      id: "refund" as ToolId,
      name: "refund",
      description: "Issues a customer refund. Sensitive — requires human approval.",
      inputSchema: passthrough,
      outputSchema: passthrough,
      permissions: ["payments:write"],
      requiresApproval: true,
      jsonSchema: {
        type: "object",
        properties: { orderId: { type: "string" }, amount: { type: "number" } }
      }
    },
    async (input: unknown) => {
      const { orderId, amount } = (input ?? {}) as { orderId?: string; amount?: number };
      return { refunded: true, orderId: orderId ?? "ord-1", amount: amount ?? 49.0 };
    }
  );

  return createGraph({ name: "refunder" })
    .agentNode("refunder", {
      llm: gateway,
      ...routing(selection),
      prompt: {
        system:
          "You are a billing agent. Look up the order, then issue the refund. " +
          "The refund tool is sensitive and requires human approval — never self-approve."
      },
      tools,
      suspendForApproval: true,
      approvalEngine,
      maxIterations: 6
    })
    .compile();
}

// (c) planner — a single plan-execute-ish agent: it plans with one tool, executes a
// step with another, then reports. Ungated.
function buildPlanner(selection: GatewaySelection): CompiledGraph {
  const gateway = selection.gateway;
  const tools = new InMemoryToolRegistry();
  tools.register(
    {
      id: "make_plan" as ToolId,
      name: "make_plan",
      description: "Break a goal into ordered steps.",
      inputSchema: passthrough,
      outputSchema: passthrough,
      permissions: [],
      jsonSchema: { type: "object", properties: { goal: { type: "string" } } }
    },
    async (input: unknown) => {
      const goal = String((input as { goal?: unknown })?.goal ?? "ship feature");
      return { steps: [`Clarify: ${goal}`, "Draft approach", "Execute step 1", "Review"] };
    }
  );
  tools.register(
    {
      id: "execute_step" as ToolId,
      name: "execute_step",
      description: "Execute a single plan step and report its outcome.",
      inputSchema: passthrough,
      outputSchema: passthrough,
      permissions: [],
      jsonSchema: { type: "object", properties: { step: { type: "string" } } }
    },
    async (input: unknown) => ({
      step: String((input as { step?: unknown })?.step ?? "step"),
      done: true
    })
  );

  return createGraph({ name: "planner" })
    .agentNode("planner", {
      llm: gateway,
      ...routing(selection),
      prompt: {
        system:
          "You are a planning agent. Call `make_plan` to decompose the goal, then `execute_step` " +
          "for the first step, then summarize. End with FINAL: <summary>."
      },
      tools,
      maxIterations: 6
    })
    .compile();
}

const AGENTS: Record<string, AgentEntry> = {
  researcher: {
    name: "researcher",
    description:
      "Answers questions from a tiny inline corpus using ungated search/fetch tools, then cites the source.",
    hasGatedTools: false,
    // search -> fetch -> final cited answer.
    script: [
      toolTurn("search", { query: "governance" }),
      toolTurn("fetch", { id: "doc-1" }),
      finalTurn(
        "FINAL: Ailu suspends on sensitive tools and resumes after human approval. [cite: doc-1]"
      )
    ],
    build: (selection) => buildResearcher(selection)
  },
  refunder: {
    name: "refunder",
    description:
      "Issues customer refunds behind a human-approval gate; suspends the run until a human approves.",
    hasGatedTools: true,
    // lookup_order -> refund (suspends) -> refund (post-approval re-run) -> FINAL.
    // The gated tool_use is scripted TWICE per the stateful-mock rule so the resumed
    // run actually executes the refund.
    script: [
      toolTurn("lookup_order", { orderId: "ord-1" }),
      toolTurn("refund", { orderId: "ord-1", amount: 49.0 }),
      toolTurn("refund", { orderId: "ord-1", amount: 49.0 }),
      finalTurn("FINAL: Refund of 49.00 EUR issued for order ord-1 after human approval.")
    ],
    build: (selection, approvalEngine) => buildRefunder(selection, approvalEngine)
  },
  planner: {
    name: "planner",
    description:
      "Plans then executes the first step of a goal using ungated make_plan/execute_step tools.",
    hasGatedTools: false,
    // make_plan -> execute_step -> final summary.
    script: [
      toolTurn("make_plan", { goal: "onboard a new user" }),
      toolTurn("execute_step", { step: "Clarify: onboard a new user" }),
      finalTurn("FINAL: Plan ready (4 steps); first step executed.")
    ],
    build: (selection) => buildPlanner(selection)
  }
};

// --- Predefined SDK graphs (for run_graph) -------------------------------------

/**
 * A small set of inline graphs runnable by name. `publish-flow` exercises a plain
 * human gate (write -> review -> publish): the run suspends at the gate. `greeter`
 * is a trivial single-node graph that completes.
 */
type GraphEntry = {
  name: string;
  description: string;
  build: () => CompiledGraph;
};

const GRAPHS: Record<string, GraphEntry> = {
  "publish-flow": {
    name: "publish-flow",
    description: "Human-in-the-loop: write -> human gate -> publish. Suspends at the gate.",
    build: () =>
      createGraph({ name: "publish-flow" })
        .channel("draft", { type: "string", default: "" })
        .channel("approved", { type: "boolean", default: false })
        .node("write", async () => ({ draft: "A drafted document." }))
        .humanGate("review")
        .node("publish", async () => ({ approved: true }))
        .edge("write", "review")
        .edge("review", "publish")
        .compile()
  },
  greeter: {
    name: "greeter",
    description: "Trivial single-node graph that greets the `name` channel. Completes immediately.",
    build: () =>
      createGraph({ name: "greeter" })
        .channel("name", { type: "string", default: "world" })
        .channel("greeting", { type: "string", default: "" })
        .node("hello", async (_input, state) => ({
          greeting: `Hello, ${String((state.channels as { name?: unknown }).name ?? "world")}!`
        }))
        .compile()
  }
};

// --- Suspended-run registry (the MCP server is long-lived) ----------------------

/**
 * State held for a run that suspended for approval, keyed by runId. We keep the SAME
 * CompiledGraph (and thus the SAME stateful gateway it closed over) so the mock
 * sequence continues correctly on resume; plus the ApprovalEngine to grant the
 * decision. This Map is what makes approve_and_resume work across separate tool calls.
 */
type PendingRun = {
  kind: "agent" | "graph";
  app: CompiledGraph;
  approvalEngine?: InMemoryApprovalEngine;
  agentName?: string;
  /** The AgentResult captured at suspension — its `approvalRequests` are the source of
   *  truth for pending approvals on the Rust engine, which bypasses the TS approval engine. */
  suspendedResult?: AgentResult;
};
const pendingRuns = new Map<string, PendingRun>();

const newRunId = (): RunId => {
  const random = globalThis.crypto?.randomUUID?.() ?? Math.random().toString(36).slice(2);
  return `run_${random}` as RunId;
};

const readAgentResult = (channels: Record<string, unknown>): AgentResult | undefined =>
  channels.agentResult as AgentResult | undefined;

/** Coerce an arbitrary tool argument into the channel-shaped initial data the engine expects. */
const asInitialData = (value: unknown): Record<string, unknown> =>
  value !== null && typeof value === "object" ? (value as Record<string, unknown>) : {};

type PendingApproval = { id: string; subject: string; requestedBy: string };

/**
 * Render an approval-request subject as text. The TS ApprovalEngine carries a
 * `{ description }` object; the Rust-path AgentResult serializes the subject as a plain
 * string (e.g. `"tool:refund"`). Handle both, falling back to JSON for any other shape.
 */
const subjectText = (subject: { description?: unknown } | unknown): string => {
  if (typeof subject === "string") return subject;
  if (typeof subject === "object" && subject !== null && "description" in subject) {
    return String((subject as { description: unknown }).description);
  }
  return JSON.stringify(subject);
};

/**
 * The pending approvals to surface for a suspended run.
 *
 * On the **TS** engine an {@link InMemoryApprovalEngine} files one request per gated
 * tool, so we read it (it carries stable ids the caller can grant individually). On the
 * **Rust** engine that TS engine is bypassed — the agent suspends natively and the
 * pending gated tools live in the captured {@link AgentResult.approvalRequests}. We fall
 * back to those so `pendingApprovals` is populated on either engine. Rust approvals are
 * granted by tool name (the channel path), so their id IS the gated tool name.
 */
const pendingFromEngine = async (
  engine: InMemoryApprovalEngine | undefined,
  runId: RunId,
  agentResult?: AgentResult
): Promise<PendingApproval[]> => {
  if (engine !== undefined) {
    const pending = await engine.getPending(runId);
    if (pending.length > 0) {
      return pending.map((req: ApprovalRequest) => ({
        id: String(req.id),
        subject: subjectText(req.subject),
        requestedBy: req.requestedBy
      }));
    }
  }
  // Rust path (or engine with nothing filed): derive from the AgentResult.
  const requests = agentResult?.approvalRequests ?? [];
  return requests.map((req) => {
    const subject = subjectText(req.subject);
    // The subject is `tool:<name>`; the grantable id is the bare tool name.
    const toolName = subject.startsWith("tool:") ? subject.slice("tool:".length) : subject;
    return { id: toolName, subject, requestedBy: "agent" };
  });
};

// --- Tool handlers -------------------------------------------------------------

async function listAgents(): Promise<ToolResult> {
  const agents = Object.values(AGENTS).map((a) => ({
    name: a.name,
    description: a.description,
    hasGatedTools: a.hasGatedTools
  }));
  const graphs = Object.values(GRAPHS).map((g) => ({ name: g.name, description: g.description }));
  return jsonResult({
    mode: currentMode(),
    agents,
    graphs
  });
}

async function runAgent(args: Record<string, unknown>): Promise<ToolResult> {
  const agentName = typeof args?.agent === "string" ? args.agent : undefined;
  if (agentName === undefined) {
    return textResult(
      "run_agent requires a string `agent` (one of: " + Object.keys(AGENTS).join(", ") + ").",
      true
    );
  }
  const entry = AGENTS[agentName];
  if (entry === undefined) {
    return textResult(
      `Unknown agent '${agentName}'. Known: ${Object.keys(AGENTS).join(", ")}.`,
      true
    );
  }

  try {
    // Build a fresh gateway (real or mock) + approval engine for THIS run, and keep
    // the compiled graph so a suspended run can resume against the same gateway. The
    // selection carries the provider/model the agent nodes must request so the run
    // routes to whichever live adapter was registered (Anthropic, or the mistral-keyed
    // OpenAI-compatible adapter for Mistral cloud / Ollama).
    const approvalEngine = new InMemoryApprovalEngine();
    const selection = makeGateway(entry.script);
    const app = entry.build(selection, approvalEngine);
    const runId = newRunId();

    const input = asInitialData(args?.input);
    const state = await app.run(input, { runId });

    if (state.status === "suspended") {
      const suspendedResult = readAgentResult(state.channels as Record<string, unknown>);
      pendingRuns.set(String(runId), {
        kind: "agent",
        app,
        approvalEngine: entry.hasGatedTools ? approvalEngine : undefined,
        agentName,
        suspendedResult
      });
      const pendingApprovals = await pendingFromEngine(approvalEngine, runId, suspendedResult);
      return jsonResult({
        runId: String(runId),
        status: state.status,
        result: readAgentResult(state.channels as Record<string, unknown>),
        pendingApprovals,
        note:
          "Run suspended on the engine for human approval. Call approve_and_resume with this runId " +
          (pendingApprovals.length > 0
            ? `and approvalId '${pendingApprovals[0]?.id}' (or approvedTools) to continue.`
            : "and the approvedTools to grant.")
      });
    }

    return jsonResult({
      runId: String(runId),
      status: state.status,
      result: readAgentResult(state.channels as Record<string, unknown>),
      note: `Run ${state.status} on the Ailu engine (gateway: ${selection.mode}).`
    });
  } catch (error) {
    return textResult(
      `run_agent('${agentName}') failed on the engine: ${error instanceof Error ? error.message : String(error)}`,
      true
    );
  }
}

async function approveAndResume(args: Record<string, unknown>): Promise<ToolResult> {
  const runId = typeof args?.runId === "string" ? args.runId : undefined;
  if (runId === undefined) {
    return textResult("approve_and_resume requires a string `runId`.", true);
  }
  const pending = pendingRuns.get(runId);
  if (pending === undefined) {
    return textResult(
      `No suspended run found for runId '${runId}'. It may have already resumed or never suspended.`,
      true
    );
  }

  try {
    const approvedTools = Array.isArray(args?.approvedTools)
      ? (args.approvedTools as unknown[]).filter((t): t is string => typeof t === "string")
      : undefined;
    const approvalId = typeof args?.approvalId === "string" ? args.approvalId : undefined;
    // The human principal granting the decision — distinct from the agent that
    // requested it (the engine forbids self-approval).
    const resolvedBy = typeof args?.approvedBy === "string" ? args.approvedBy : "human-operator";

    // The TS ApprovalEngine path only applies when running on the TS engine AND it has
    // requests filed (the engine-backed flow lives in the TS agent-node handler). On the
    // Rust engine that flow is bypassed — the agent suspends natively and is resumed by
    // writing the approved tool NAMES into the `__approvedTools` channel (the channel
    // path). We branch on which one is actually in effect, so the same handler works on
    // either engine without changing behavior under AILU_SDK_ENGINE=ts.
    const enginePending =
      pending.approvalEngine !== undefined && !pending.app.usesRustEngine
        ? await pending.approvalEngine.getPending(runId as RunId)
        : [];

    let state;
    if (pending.approvalEngine !== undefined && enginePending.length > 0) {
      // Engine path (TS): grant the named approval(s), then resume. The engine is the
      // source of truth; the agent re-runs and executes whatever it reports approved.
      const toResolve = approvalId
        ? [approvalId]
        : enginePending.map((r: ApprovalRequest) => String(r.id));
      for (const id of toResolve) {
        await pending.approvalEngine.approve(id as ApprovalId, resolvedBy);
      }
      state = await pending.app.resume(runId as RunId);
    } else {
      // Channel path (Rust, or channel-based runs): write the approved tool names and
      // resume. When the caller didn't pass `approvedTools` we grant the gated tools the
      // agent surfaced in its captured AgentResult (or the conservative default).
      const grant = approvedTools ?? grantedToolsForResume(pending);
      if (grant.length === 0) {
        return textResult(`No pending approvals to grant for runId '${runId}'.`, true);
      }
      state = await pending.app.approveAndResume(runId as RunId, { approvedTools: grant });
    }

    // The run is no longer suspended (or it suspended again on a further gate).
    if (state.status === "suspended") {
      const suspendedResult = readAgentResult(state.channels as Record<string, unknown>);
      pending.suspendedResult = suspendedResult;
      const pendingApprovals = await pendingFromEngine(
        pending.approvalEngine,
        runId as RunId,
        suspendedResult
      );
      return jsonResult({
        runId,
        status: state.status,
        result: readAgentResult(state.channels as Record<string, unknown>),
        pendingApprovals,
        note: "Run suspended again — another approval is required."
      });
    }

    pendingRuns.delete(runId);
    return jsonResult({
      runId,
      status: state.status,
      result: readAgentResult(state.channels as Record<string, unknown>),
      note: `Approval granted by '${resolvedBy}'. Run ${state.status} on the Ailu engine — the gated tool executed.`
    });
  } catch (error) {
    return textResult(
      `approve_and_resume('${runId}') failed on the engine: ${error instanceof Error ? error.message : String(error)}`,
      true
    );
  }
}

/**
 * The tools to grant on a channel-path resume when the caller passed no explicit
 * `approvedTools`: the gated tools the agent surfaced in its captured AgentResult
 * (`subject: "tool:<name>"`). Falls back to the predefined `refund` tool for robustness.
 */
function grantedToolsForResume(pending: PendingRun): string[] {
  const fromResult = (pending.suspendedResult?.approvalRequests ?? [])
    .map((req) => subjectText(req.subject))
    .map((subject) => (subject.startsWith("tool:") ? subject.slice("tool:".length) : subject))
    .filter((name) => name.length > 0);
  return fromResult.length > 0 ? fromResult : ["refund"];
}

async function runGraph(args: Record<string, unknown>): Promise<ToolResult> {
  const graphName = typeof args?.graph === "string" ? args.graph : undefined;
  if (graphName === undefined) {
    return textResult(
      "run_graph requires a string `graph` (one of: " + Object.keys(GRAPHS).join(", ") + ").",
      true
    );
  }
  const entry = GRAPHS[graphName];
  if (entry === undefined) {
    return textResult(
      `Unknown graph '${graphName}'. Known: ${Object.keys(GRAPHS).join(", ")}.`,
      true
    );
  }

  try {
    const app = entry.build();
    const runId = newRunId();
    const input = asInitialData(args?.input);
    const state = await app.run(input, { runId });

    if (state.status === "suspended") {
      pendingRuns.set(String(runId), { kind: "graph", app });
      return jsonResult({
        runId: String(runId),
        status: state.status,
        result: { currentNodeId: String(state.currentNodeId), channels: state.channels },
        note: `Graph '${graphName}' suspended at a human gate ('${String(state.currentNodeId)}'). Call approve_and_resume with this runId to continue.`
      });
    }

    return jsonResult({
      runId: String(runId),
      status: state.status,
      result: { channels: state.channels },
      note: `Graph '${graphName}' ${state.status} on the Ailu engine.`
    });
  } catch (error) {
    return textResult(
      `run_graph('${graphName}') failed on the engine: ${error instanceof Error ? error.message : String(error)}`,
      true
    );
  }
}

async function validateGraph(args: Record<string, unknown>): Promise<ToolResult> {
  const definitionJson = args?.definitionJson;
  if (typeof definitionJson !== "string") {
    return textResult("validate_graph requires a string `definitionJson`.", true);
  }
  const loaded = await loadEngine();
  if ("error" in loaded) return textResult(loaded.error, true);
  try {
    return textResult(loaded.engine.validateGraphJson(definitionJson));
  } catch (err) {
    return textResult(
      `validate_graph failed: ${err instanceof Error ? err.message : String(err)}`,
      true
    );
  }
}

async function compileGraphYaml(args: Record<string, unknown>): Promise<ToolResult> {
  const yaml = args?.yaml;
  if (typeof yaml !== "string") {
    return textResult("compile_graph_yaml requires a string `yaml`.", true);
  }
  const loaded = await loadEngine();
  if ("error" in loaded) return textResult(loaded.error, true);
  try {
    return textResult(loaded.engine.compileGraphYamlJson(yaml));
  } catch (err) {
    return textResult(
      `compile_graph_yaml failed: ${err instanceof Error ? err.message : String(err)}`,
      true
    );
  }
}

// --- Machine-API tools (ADR DX batch 5): discover + introspect the framework -------------

/** Discovery: every component node kind with its JSON Schema (params), category + description —
 * so an AI agent can enumerate the building blocks before authoring a graph. */
async function listComponents(): Promise<ToolResult> {
  return jsonResult(componentSchemas());
}

/** Ground truth: the generated llms.txt (install, the API surface, catalogs, error codes,
 * invariants) — what an agent reads to use Ailu without hallucinating. */
async function getLlmsTxt(): Promise<ToolResult> {
  return textResult(generateLlmsTxt());
}

/** Introspection: explain where a suspended run stands (why, what unblocks it, what failed) —
 * the run must be one this server is currently tracking (suspended) by runId. */
async function explainRunTool(args: Record<string, unknown>): Promise<ToolResult> {
  const runId = typeof args?.runId === "string" ? args.runId : undefined;
  if (runId === undefined) return textResult("explain_run requires a string `runId`.", true);
  const pending = pendingRuns.get(runId);
  if (pending === undefined) {
    return textResult(
      `No tracked run for runId '${runId}'. A run is tracked while suspended — run run_agent/run_graph first.`,
      true
    );
  }
  return jsonResult(pending.app.explain(runId as RunId));
}

// --- Tool definitions ----------------------------------------------------------

export const TOOLS = [
  {
    name: "list_components",
    description:
      "Discover every Ailu component node kind with its JSON Schema (params), category and description. " +
      "Use this to enumerate the building blocks before authoring a graph — the catalog is the engine's source of truth.",
    inputSchema: { type: "object", properties: {} }
  },
  {
    name: "get_llms_txt",
    description:
      "Return Ailu's llms.txt — the single ground-truth doc (install, the createGraph/agentNode/model API, " +
      "the component/prebuilt/tier catalogs, error codes, invariants). Read it first to use the framework accurately.",
    inputSchema: { type: "object", properties: {} }
  },
  {
    name: "explain_run",
    description:
      "Explain where a suspended run stands: its status, WHY it suspended (human gate / approval / signal / timer), " +
      "the exact next action to resume, and any failure. The run must be tracked by this server (suspended) — pass the runId from run_agent/run_graph.",
    inputSchema: {
      type: "object",
      properties: { runId: { type: "string", description: "A suspended run's id (from run_agent/run_graph)." } },
      required: ["runId"]
    }
  },
  {
    name: "list_agents",
    description:
      "List the predefined engine-native Ailu agents (and runnable graphs) this server can execute. " +
      "Reports each agent's name, description, and whether it has approval-gated tools, plus the current gateway mode (real vs mock).",
    inputSchema: { type: "object", properties: {} }
  },
  {
    name: "run_agent",
    description:
      "Execute a predefined Ailu agent ON THE ENGINE (in-process via @ailu/graph-sdk) with a fresh run. " +
      "Returns { runId, status, result?, pendingApprovals?, note }. If the agent reaches for a sensitive tool the run " +
      "SUSPENDS (status 'suspended') and surfaces pending approvals — call approve_and_resume to continue.",
    inputSchema: {
      type: "object",
      properties: {
        agent: {
          type: "string",
          description: "Agent name (see list_agents): researcher | refunder | planner."
        },
        input: { type: "object", description: "Optional input passed to the agent run." }
      },
      required: ["agent"]
    }
  },
  {
    name: "approve_and_resume",
    description:
      "Grant human approval for a suspended run and resume it ON THE ENGINE. The suspended run is held in-process by runId. " +
      "Pass `approvalId` (preferred, from pendingApprovals) and/or `approvedTools`. Returns the final { runId, status, result }.",
    inputSchema: {
      type: "object",
      properties: {
        runId: {
          type: "string",
          description: "The runId returned by run_agent/run_graph when the run suspended."
        },
        approvalId: {
          type: "string",
          description: "A specific pending approval id to grant (from pendingApprovals)."
        },
        approvedTools: {
          type: "array",
          items: { type: "string" },
          description: "Names of approval-gated tools the human grants (channel path)."
        },
        approvedBy: {
          type: "string",
          description:
            "Principal granting approval (must differ from the requester). Default 'human-operator'."
        }
      },
      required: ["runId"]
    }
  },
  {
    name: "run_graph",
    description:
      "Run a predefined Ailu graph by name ON THE ENGINE. Returns { runId, status, result, note }. Graphs with a human gate " +
      "SUSPEND at the gate — resume with approve_and_resume. See list_agents for the available graphs.",
    inputSchema: {
      type: "object",
      properties: {
        graph: { type: "string", description: "Graph name: publish-flow | greeter." },
        input: { type: "object", description: "Optional initial channel data." }
      },
      required: ["graph"]
    }
  },
  {
    name: "validate_graph",
    description:
      "Validate an Ailu GraphDefinition (JSON string) via the Rust engine. Returns a JSON array of structural validation errors " +
      "([] when sound). Errors include codes like INVALID_EDGE_REFERENCE.",
    inputSchema: {
      type: "object",
      properties: {
        definitionJson: {
          type: "string",
          description: "A GraphDefinition serialized as a JSON string."
        }
      },
      required: ["definitionJson"]
    }
  },
  {
    name: "compile_graph_yaml",
    description:
      "Compile Ailu graph DSL YAML into a validated GraphDefinition (returned as JSON) via the Rust engine. Clear error on failure.",
    inputSchema: {
      type: "object",
      properties: {
        yaml: { type: "string", description: "The graph DSL YAML document to compile." }
      },
      required: ["yaml"]
    }
  }
];

const HANDLERS: Record<string, (args: Record<string, unknown>) => Promise<ToolResult>> = {
  list_components: listComponents,
  get_llms_txt: getLlmsTxt,
  explain_run: explainRunTool,
  list_agents: listAgents,
  run_agent: runAgent,
  approve_and_resume: approveAndResume,
  run_graph: runGraph,
  validate_graph: validateGraph,
  compile_graph_yaml: compileGraphYaml
};

// --- Knowledge-base resources --------------------------------------------------
// Expose the persistent OKF knowledge base (owned by the control plane) as MCP
// RESOURCES, so any MCP client — Claude Desktop, an IDE, another agent — can read an
// organization's knowledge through the open standard. The server holds no DB; it reads
// the KB over the control-plane HTTP API. This is the "BKP extends MCP" surface.

const KB_API_BASE = process.env.NEXT_PUBLIC_API_URL ?? process.env.API_BASE_URL ?? "http://localhost:3001";
const KB_NAMESPACE = process.env.AILU_MCP_KB_NAMESPACE ?? "ailu";
const KB_URI_PREFIX = "ailu-kb://";

type KbDoc = { id: string; content: string; type: string; title?: string };

async function fetchKbDocs(): Promise<KbDoc[]> {
  try {
    const res = await fetch(`${KB_API_BASE}/knowledge/${encodeURIComponent(KB_NAMESPACE)}/documents`);
    if (!res.ok) {
      return [];
    }
    return (await res.json()) as KbDoc[];
  } catch {
    return [];
  }
}

const kbUri = (id: string): string => `${KB_URI_PREFIX}${KB_NAMESPACE}/${id}`;

// --- Server wiring -------------------------------------------------------------

export function createServer(): Server {
  const server = new Server(
    { name: "ailu", version: "0.1.0" },
    { capabilities: { tools: {}, resources: {} } }
  );

  server.setRequestHandler(ListToolsRequestSchema, async () => ({ tools: TOOLS }));

  server.setRequestHandler(ListResourcesRequestSchema, async () => {
    const docs = await fetchKbDocs();
    return {
      resources: docs.map((doc) => ({
        uri: kbUri(doc.id),
        name: doc.title ?? doc.id,
        description: `${doc.type} — Ailu knowledge base (${KB_NAMESPACE})`,
        mimeType: "text/markdown"
      }))
    };
  });

  server.setRequestHandler(ReadResourceRequestSchema, async (request) => {
    const { uri } = request.params;
    const docs = await fetchKbDocs();
    const doc = docs.find((candidate) => kbUri(candidate.id) === uri);
    if (doc === undefined) {
      return { contents: [] };
    }
    return { contents: [{ uri, mimeType: "text/markdown", text: doc.content }] };
  });

  server.setRequestHandler(CallToolRequestSchema, async (request) => {
    const handler = HANDLERS[request.params.name];
    if (handler === undefined) {
      return textResult(`Unknown tool: ${request.params.name}`, true);
    }
    try {
      return await handler((request.params.arguments ?? {}) as Record<string, unknown>);
    } catch (error) {
      // Never crash the server: surface any unexpected engine error as an MCP error result.
      return textResult(
        `${request.params.name} failed: ${error instanceof Error ? error.message : String(error)}`,
        true
      );
    }
  });

  return server;
}

async function main(): Promise<void> {
  const server = createServer();
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

// Only start the stdio server when run as the entrypoint (not when imported).
const isEntrypoint =
  process.argv[1] !== undefined && import.meta.url === new URL(`file://${process.argv[1]}`).href;
if (isEntrypoint) {
  main().catch((error: unknown) => {
    console.error(
      `ailu MCP server failed to start: ${error instanceof Error ? error.message : String(error)}`
    );
    process.exit(1);
  });
}
