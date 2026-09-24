/**
 * Tutorial — From idea to shipping: a governed venture pipeline (advanced).
 *
 * What you'll learn:
 *   - a pipeline built only from agent nodes, catalog components and gates, so its definition
 *     is pure data. It runs on the catalog path (`runCatalogGraph` / `resumeCatalogGraph`),
 *     the seam a control plane uses to execute stored graphs and to resume them later, in
 *     another process
 *   - two governance seams in one run:
 *       1. a `humanGate` ("brand-review"): a structural pause in the graph
 *       2. an approval-gated tool: the security agent reaches for `deploy_to_prod`, the run
 *          suspends, and an ApprovalEngine records the request and who approves it
 *   - the run-event journal: every lifecycle transition emits an event
 *
 * Pipeline: product-spec → brand → [brand-review] → design → build-mvp
 *           → security-audit (gated deploy) → launch
 *
 * Self-verifying: every claim is checked and the first failed check throws. The checks are
 * structural (status, where the run paused, which tools ran, who approved), never the
 * model's wording.
 *
 * Run it offline (the engine's deterministic mock calls each declared tool once):
 *   AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk example:startup
 * With ANTHROPIC_API_KEY set, every agent runs on Claude.
 */
import {
  ApprovalSelfApprovalError,
  components,
  createGraph,
  finalAnswer,
  InMemoryApprovalEngine,
  InMemoryToolRegistry,
  model,
  resumeCatalogGraph,
  runCatalogGraph,
  type AgentResult,
  type GraphState,
  type RunEvent,
  type RunId,
  type ToolId
} from "@ailu-ai/graph-sdk";

// Self-check: fail loudly (throw) rather than print a wrong claim.
const check = (condition: boolean, label: string): void => {
  if (!condition) throw new Error(`Check failed: ${label}`);
  console.log(`  ✓ ${label}`);
};

/** The final answer an agent node wrote into `channel` ("" if it wrote nothing). */
const answerIn = (state: GraphState, channel: string): string =>
  finalAnswer(state.channels[channel] as AgentResult | undefined);

// ── Tools: an ungated scaffolder and an approval-gated production deploy ─────
let scaffolds = 0;
let deploys = 0;
const passthrough = { parse: (value: unknown) => value };

const mvpTools = new InMemoryToolRegistry();
mvpTools.register(
  {
    id: "scaffold_mvp" as ToolId,
    name: "scaffold_mvp",
    description: "Scaffold the MVP codebase from a template. Safe and reversible.",
    inputSchema: passthrough,
    outputSchema: passthrough,
    permissions: ["repo:write"],
    jsonSchema: { type: "object", properties: { template: { type: "string" } } }
  },
  async () => {
    scaffolds += 1;
    return { scaffolded: true, files: 12, stack: "TypeScript + Ailu" };
  }
);

const securityTools = new InMemoryToolRegistry();
securityTools.register(
  {
    id: "deploy_to_prod" as ToolId,
    name: "deploy_to_prod",
    description: "Deploy to production. Sensitive: requires human approval.",
    inputSchema: passthrough,
    outputSchema: passthrough,
    permissions: ["prod:deploy"],
    requiresApproval: true,
    jsonSchema: { type: "object", properties: { target: { type: "string" } } }
  },
  async () => {
    deploys += 1;
    return { deployed: true, url: "https://lumora.example.com" };
  }
);

// The registries tell each agent WHAT it may call. On the catalog path the graph definition
// is data (no closures), so the code behind each tool name is passed per run as bindings.
const toolBindings = [mvpTools, securityTools].flatMap((registry) =>
  registry.list().map((definition) => ({
    name: definition.name,
    execute: async (input: unknown) => registry.resolve(definition.id)?.handler(input)
  }))
);

// ── The pipeline ─────────────────────────────────────────────────────────────
const sonnet = model.anthropic("claude-sonnet-4-6");

const app = createGraph({ name: "venture-pipeline" })
  .channel("idea", { type: "string", default: "" })
  .channel("designBrief", { type: "string", default: "" })
  .agentNode("product-spec", {
    model: sonnet,
    prompt: { system: "You are a product strategist. Write a crisp positioning statement for the idea." },
    maxIterations: 2,
    outputChannel: "specResult"
  })
  .agentNode("brand", {
    model: sonnet,
    prompt: { system: "You are a brand strategist. Propose one product name with a one-line rationale." },
    maxIterations: 2,
    outputChannel: "brandResult"
  })
  // Governance seam #1: the founder signs off on the brand before anything is built.
  .humanGate("brand-review")
  // A catalog component: deterministic, no LLM call, runs natively in Rust.
  .component(
    "design",
    components.promptBuilder({
      template: "Design system for: {{idea}} Deep indigo, generous whitespace.",
      into: "designBrief"
    })
  )
  .agentNode("build-mvp", {
    model: sonnet,
    prompt: { system: "You are a builder. Scaffold the MVP with your tools, then report." },
    tools: mvpTools,
    maxIterations: 4,
    outputChannel: "mvpResult"
  })
  // Governance seam #2: deploy_to_prod requires approval, so the agent suspends the run.
  .agentNode("security-audit", {
    model: sonnet,
    prompt: { system: "You are a security auditor. Review the MVP, then deploy it with deploy_to_prod." },
    tools: securityTools,
    suspendForApproval: true,
    maxIterations: 4,
    outputChannel: "securityResult"
  })
  .agentNode("launch", {
    model: sonnet,
    prompt: { system: "Write a two-sentence launch announcement for the product." },
    maxIterations: 2,
    outputChannel: "launchResult"
  })
  .edge("product-spec", "brand")
  .edge("brand", "brand-review")
  .edge("brand-review", "design")
  .edge("design", "build-mvp")
  .edge("build-mvp", "security-audit")
  .edge("security-audit", "launch")
  .compile();

// ── Lifecycle journal: every transition emits an event ───────────────────────
const journal: string[] = [];
const onEvent = (event: RunEvent): void => {
  journal.push("nodeId" in event ? `${event.type}:${String(event.nodeId)}` : event.type);
};

const RUN_ID = "run_startup_e2e_demo" as RunId;
const IDEA = "A governance-first control plane for fleets of AI agents.";
const approvals = new InMemoryApprovalEngine(); // use a persistent ApprovalEngine in production

// ── Act 1: run until the brand-review gate ───────────────────────────────────
console.log("\nAct 1 — positioning and branding:");
const atBrandReview = await runCatalogGraph(app.definition, {
  runId: RUN_ID,
  initialData: { idea: IDEA },
  tools: toolBindings,
  onEvent
});

check(atBrandReview.status === "suspended", "the run suspended at the human gate");
check(String(atBrandReview.state.currentNodeId) === "brand-review", "it is paused at brand-review");
check(answerIn(atBrandReview.state, "brandResult").length > 0, "the brand agent proposed a name");
check(scaffolds === 0, "nothing was built before the brand sign-off");

// ── Act 2: the founder signs off; the build runs until the gated deploy ──────
// From here the run is driven with the ApprovalEngine: when an agent suspends for approval,
// the SDK files one request per gated tool call, with the agent as the requester.
console.log("\nAct 2 — brand approved, building the MVP:");
const atDeploy = await resumeCatalogGraph(app.definition, atBrandReview.state, {
  tools: toolBindings,
  approvalEngine: approvals,
  onEvent
});

check(atDeploy.status === "suspended", "the run suspended again, this time for a tool approval");
check(String(atDeploy.state.currentNodeId) === "security-audit", "it is paused at security-audit");
check(String(atDeploy.state.channels.designBrief).includes(IDEA), "the design component rendered the brief");
check(scaffolds >= 1, "scaffold_mvp ran without approval (it is not gated)");
check(deploys === 0, "deploy_to_prod did NOT run before approval");

const [request] = await approvals.getPending(RUN_ID);
if (request === undefined) throw new Error("Check failed: no pending approval request was filed");
check(
  "description" in request.subject && request.subject.description === "tool:deploy_to_prod",
  "the pending approval request is for tool:deploy_to_prod"
);
check(request.requestedBy === "security-audit", "the request was filed by the security-audit agent");

// The requesting agent can never approve its own request.
const selfApproval = await approvals.approve(request.id, request.requestedBy).catch((error: unknown) => error);
check(selfApproval instanceof ApprovalSelfApprovalError, "the agent cannot approve its own request");

// ── Act 3: the founder approves the deploy; the run ships ────────────────────
console.log("\nAct 3 — the founder approves the production deploy:");
await approvals.approve(request.id, "founder");
const shipped = await resumeCatalogGraph(app.definition, atDeploy.state, {
  tools: toolBindings,
  approvalEngine: approvals,
  onEvent,
  // The grant carries its provenance; the engine re-checks it and refuses a self-approval.
  approvedTools: [{ name: "deploy_to_prod", requestedBy: request.requestedBy, resolvedBy: "founder" }]
});

check(shipped.status === "completed", "the run completed after the approval");
check(deploys === 1, "deploy_to_prod ran exactly once, after approval");
check((await approvals.getById(request.id))?.resolvedBy === "founder", "the engine records who approved");
check(answerIn(shipped.state, "launchResult").length > 0, "the launch agent wrote the announcement");
check(journal.filter((entry) => entry.startsWith("run_suspended")).length === 2, "the journal records two suspensions");
check(journal.includes("run_completed"), "the journal records the completion");

// ── The journey, end to end ──────────────────────────────────────────────────
console.log("\nLifecycle journal:");
for (const entry of journal) {
  console.log(`  ${entry}`);
}

console.log("\nJourney summary:");
console.log(`  Idea:        ${IDEA}`);
console.log(`  Positioning: ${answerIn(shipped.state, "specResult")}`);
console.log(`  Brand:       ${answerIn(shipped.state, "brandResult")}`);
console.log(`  Design:      ${String(shipped.state.channels.designBrief)}`);
console.log(`  MVP:         ${answerIn(shipped.state, "mvpResult")}`);
console.log(`  Security:    ${answerIn(shipped.state, "securityResult")}`);
console.log(`  Launch:      ${answerIn(shipped.state, "launchResult")}`);

console.log("\nAll checks passed — the governed venture pipeline shipped.");
