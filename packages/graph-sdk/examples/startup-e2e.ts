/**
 * Tutorial — From idea to shipping: a governed venture pipeline (advanced).
 *
 * What you'll learn:
 *   - chaining action nodes and several agent nodes, each with its own output channel
 *   - two different governance seams in one run:
 *       1. a `humanGate` ("brand-review") — a structural pause in the graph
 *       2. a native agent suspension — the security agent reaches for an
 *          approval-gated tool (`deploy_to_prod`) and the run suspends through a
 *          real ApprovalEngine; a human approves; the run resumes and deploys
 *   - the run-event journal: every lifecycle transition emits an event
 *
 * Pipeline: ideation → product-spec → brand → [human gate] → design → build-mvp
 *           → security-audit (gated deploy) → ship
 *
 * Offline and self-verifying: scripted mock LLMs (no API key); every claim is
 * asserted and the process exits 1 on the first failure.
 *
 * Run it:
 *   pnpm --filter @ailu/graph-sdk example:startup
 */
import {
  createGraph,
  DefaultLLMGateway,
  InMemoryToolRegistry,
  MockLLMProviderAdapter,
  type LLMGateway,
  type LLMResponse,
  type RunId,
  type ToolId
} from "@ailu/graph-sdk";
// Import the in-memory engine directly (not the package index) so the example never
// pulls the Pg engine and its `db`/`pg` dependency chain.
import { InMemoryApprovalEngine } from "../../approval-engine/src/in-memory-approval-engine.js";

// ── Self-verification helpers ────────────────────────────────────────────────
const assert = (condition: boolean, label: string): void => {
  if (!condition) {
    console.error(`✗ ASSERTION FAILED: ${label}`);
    process.exit(1);
  }
  console.log(`  ✓ ${label}`);
};

const must = <T>(value: T | undefined, label: string): T => {
  if (value === undefined) {
    console.error(`✗ ASSERTION FAILED: ${label} (got undefined)`);
    process.exit(1);
  }
  return value;
};

// ── Scripted mock LLM turns (one gateway per agent node) ─────────────────────
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

const scripted = (responses: LLMResponse[]): LLMGateway => {
  const gateway = new DefaultLLMGateway();
  gateway.registerAdapter(new MockLLMProviderAdapter({ provider: "anthropic", responses }));
  return gateway;
};

const finalLine = (reasoning: string): string =>
  (/^final:(.*)$/m.exec(reasoning)?.[1] ?? "").trim();

const passthrough = { parse: (value: unknown) => value };

// ── Tools: an ungated scaffolder and a gated production deploy ───────────────
let scaffoldCount = 0;
const mvpTools = new InMemoryToolRegistry();
mvpTools.register(
  {
    id: "scaffold_mvp" as ToolId,
    name: "scaffold_mvp",
    description: "Scaffolds the MVP codebase from a template. Safe, reversible.",
    inputSchema: passthrough,
    outputSchema: passthrough,
    permissions: ["repo:write"],
    jsonSchema: { type: "object", properties: { template: { type: "string" } } }
  },
  async () => {
    scaffoldCount += 1;
    return { scaffolded: true, files: 12, stack: "TypeScript + Ailu" };
  }
);

let deployCount = 0;
const securityTools = new InMemoryToolRegistry();
securityTools.register(
  {
    id: "deploy_to_prod" as ToolId,
    name: "deploy_to_prod",
    description: "Deploys to production. Sensitive — requires human approval.",
    inputSchema: passthrough,
    outputSchema: passthrough,
    permissions: ["prod:deploy"],
    requiresApproval: true,
    jsonSchema: { type: "object", properties: { target: { type: "string" } } }
  },
  async () => {
    deployCount += 1;
    return { deployed: true, url: "https://lumora.example.com" };
  }
);

// ── The approval engine: the human seam for the gated deploy ─────────────────
const engine = new InMemoryApprovalEngine();

// ── The pipeline ─────────────────────────────────────────────────────────────
// KEY mock-sequencing rule: the scripted gateway is stateful across suspend/resume.
// When the security agent suspends on the gated tool and is later resumed, it
// re-runs and consumes the NEXT scripted response — so the gated tool_use is
// scripted TWICE in a row, then the FINAL turn.
const app = createGraph({ name: "venture-pipeline" })
  .channel("idea", { type: "string", default: "" })
  .channel("brandName", { type: "string", default: "" })
  .channel("designSpec", { type: "string", default: "" })
  .channel("shipped", { type: "boolean", default: false })
  .node("ideation", async () => ({
    idea: "A governance-first control plane for fleets of AI agents."
  }))
  .agentNode("product-spec", {
    llm: scripted([
      finalTurn(
        "FINAL: Positioning — the safety layer for agent operations: every sensitive action " +
          "is checkpointed, attributable and human-approved. ICP: platform teams shipping agents."
      )
    ]),
    prompt: { system: "You are a product strategist. Produce a crisp positioning statement." },
    maxIterations: 2,
    outputChannel: "specResult"
  })
  .agentNode("brand", {
    llm: scripted([
      finalTurn("FINAL: BRAND_NAME=Lumora — short, luminous, memorable; evokes clarity and oversight.")
    ]),
    prompt: { system: "You are a brand strategist. Propose one name as BRAND_NAME=<name>." },
    maxIterations: 2,
    outputChannel: "brandResult"
  })
  // Lift the proposed name out of the agent trace into a typed channel.
  .node("extract-brand", async (_input, state) => {
    const match = /BRAND_NAME=([A-Za-z0-9-]+)/.exec(state.channels.brandResult.reasoning);
    return { brandName: match?.[1] ?? "" };
  })
  // Governance seam #1: a human signs off on the brand before any build starts.
  .humanGate("brand-review")
  .node("design", async (_input, state) => ({
    designSpec: `Design system for ${state.channels.brandName}: deep indigo, generous whitespace.`
  }))
  .agentNode("build-mvp", {
    llm: scripted([
      toolTurn("scaffold_mvp", { template: "saas-starter" }),
      finalTurn("FINAL: MVP scaffolded — 12 files generated, ready for the security audit.")
    ]),
    prompt: { system: "You are a builder. Scaffold the MVP with your tools, then report." },
    tools: mvpTools,
    maxIterations: 4,
    outputChannel: "mvpResult"
  })
  // Governance seam #2: the deploy tool is approval-gated; the agent cannot
  // self-approve, so the run suspends through the ApprovalEngine.
  .agentNode("security-audit", {
    llm: scripted([
      toolTurn("deploy_to_prod", { target: "production" }),
      toolTurn("deploy_to_prod", { target: "production" }),
      finalTurn("FINAL: Security checks passed — deployed to production after human approval.")
    ]),
    prompt: { system: "You are a security auditor. Deploy only through the gated tool." },
    tools: securityTools,
    suspendForApproval: true,
    approvalEngine: engine,
    maxIterations: 4,
    outputChannel: "securityResult"
  })
  .node("ship", async () => ({ shipped: true }))
  .edge("ideation", "product-spec")
  .edge("product-spec", "brand")
  .edge("brand", "extract-brand")
  .edge("extract-brand", "brand-review")
  .edge("brand-review", "design")
  .edge("design", "build-mvp")
  .edge("build-mvp", "security-audit")
  .edge("security-audit", "ship")
  .compile();

// ── Lifecycle journal: every node transition emits an event ──────────────────
const journal: string[] = [];
app.onEvent((event) => {
  const node = "nodeId" in event ? `:${String(event.nodeId)}` : "";
  journal.push(`${event.type}${node}`);
});

const RUN_ID = "run_startup_e2e_demo" as RunId;

// ── Act 1: run until the brand-review human gate ─────────────────────────────
console.log("\nAct 1 — ideation through branding:");
const atBrandReview = await app.run({}, { runId: RUN_ID });

assert(atBrandReview.status === "suspended", "run suspended at the human gate");
assert(String(atBrandReview.currentNodeId) === "brand-review", "paused at brand-review");
assert(atBrandReview.channels.brandName === "Lumora", "brand name extracted into its channel");

// ── Act 2: the founder approves the brand; the build runs until the gated deploy
console.log("\nAct 2 — brand approved, building the MVP:");
const atSecurityGate = await app.resume(RUN_ID);

assert(atSecurityGate.status === "suspended", "run suspended again (agent interrupt)");
assert(String(atSecurityGate.currentNodeId) === "security-audit", "paused at security-audit");
assert(scaffoldCount === 1, "scaffold_mvp executed exactly once (ungated)");
assert(deployCount === 0, "deploy_to_prod did NOT execute before approval");

const pending = await engine.getPending(RUN_ID);
assert(pending.length === 1, "exactly one pending approval in the engine");
const request = must(pending[0], "the pending approval request");
assert(
  JSON.stringify(request.subject).includes("tool:deploy_to_prod"),
  "the approval subject names tool:deploy_to_prod"
);
assert(request.requestedBy === "security-audit", "the request was filed by security-audit");

// ── Act 3: a human (the founder) approves the deploy; the run ships ──────────
console.log("\nAct 3 — founder approves the production deploy:");
await engine.approve(request.id, "founder");
const shipped = await app.resume(RUN_ID);

assert(shipped.status === "completed", "run completed after the engine approval");
assert(shipped.channels.shipped, "shipped === true");
assert(deployCount === 1, "deploy_to_prod executed exactly once (post-approval)");
assert(scaffoldCount === 1, "scaffold_mvp still executed exactly once");
assert(
  journal.filter((entry) => entry.startsWith("run_suspended")).length === 2,
  "the journal records exactly two suspensions"
);
assert(journal.includes("run_completed"), "the journal records the completion");

// ── The journey, end to end ──────────────────────────────────────────────────
console.log("\nLifecycle journal:");
for (const entry of journal) {
  console.log(`  ${entry}`);
}

console.log("\nJourney summary:");
console.log(`  Idea:        ${shipped.channels.idea}`);
console.log(`  Positioning: ${finalLine(shipped.channels.specResult.reasoning)}`);
console.log(`  Brand:       ${shipped.channels.brandName}`);
console.log(`  Design:      ${shipped.channels.designSpec}`);
console.log(`  MVP:         ${finalLine(shipped.channels.mvpResult.reasoning)}`);
console.log(`  Security:    ${finalLine(shipped.channels.securityResult.reasoning)}`);
console.log(`  Shipped:     ${shipped.channels.shipped}`);

console.log("\nAll assertions passed — the governed venture pipeline shipped.");
