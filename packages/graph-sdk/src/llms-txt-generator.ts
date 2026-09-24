import { componentCatalog, prebuiltCatalog, tierCatalog } from "./catalog.js";

/**
 * Generate `llms.txt` (the [llmstxt.org](https://llmstxt.org) convention): a single, accurate
 * ground-truth file an AI coding agent reads to use Ailu **without hallucinating the API**.
 * Built from the same catalogs the engine validates against, so it cannot drift. Pure — no I/O.
 */
export function generateLlmsTxt(): string {
  const components = componentCatalog
    .map((c) => `- \`${c.kind}\` (${c.category}${c.integration ? ", integration" : ""}) — ${c.description}`)
    .join("\n");
  const prebuilts = prebuiltCatalog
    .map((p) => `- \`${p.name}\` — ${p.description} (tier: ${p.tier}${p.suspendForApproval ? ", suspends for approval" : ""})`)
    .join("\n");
  const tiers = tierCatalog
    .map((t) => `- \`${t.tier}\` — ${t.description}`)
    .join("\n");

  return `# Ailu

> The open framework for stateful, resumable, **governed** agent graphs. A Rust execution engine
> driven by a thin TypeScript SDK (\`@ailu-ai/graph-sdk\`). Every run is deterministic, checkpointed
> after each node, and resumable — including across process restarts and human approvals. There is
> **no TypeScript execution fallback**: graphs run on the Rust engine (shipped prebuilt).

## Install

\`\`\`bash
npm install @ailu-ai/graph-sdk
\`\`\`
The Rust engine ships prebuilt (macOS/Linux-glibc/Windows) — no toolchain to install.

## Core API (import from \`@ailu-ai/graph-sdk\`)

- \`createGraph({ name }) -> GraphBuilder\` — fluent, typed builder. Channel value types flow through.
- \`.channel(name, { type, default? })\` — declare a typed state channel.
- \`.node(id, async (input, state) => partialChannels)\` — an action node.
- \`.agentNode(id, { model, prompt, tools?, middleware?, maxIterations? })\` — a ReAct agent node.
- \`.humanGate(id)\` — suspends the run for human approval; \`app.resume(runId)\` continues from the checkpoint.
- \`.edge(from, to)\` and \`.conditionalEdge(from, to, name, (state) => boolean)\` — routing. Conditions are
  **named predicates**, never eval'd strings.
- \`.compile() -> CompiledGraph\` (throws \`GraphCompileError\` on invalid) / \`.safeCompile() -> Result\`.
- \`app.run(initialData?) -> GraphState\`, \`app.resume(runId)\`, \`app.signal(runId, name, payload)\`.
- \`app.stream(initialData, mode)\` — \`mode\` ∈ \`values | updates | messages | debug\`; \`messages\` streams per-token.
- \`app.explain(runId) -> RunExplanation\` — why a run suspended / what it awaits / what failed.

## Picking a model (the \`model\` surface)

\`\`\`ts
import { model } from "@ailu-ai/graph-sdk";
await model.invoke("hi");                    // zero-config: provider from env keys, fails loud if none
await model.openai("gpt-4o").invoke("hi");   // provider is the method
await model.fast.invoke("classify");         // tiers are properties: fast|balanced|frontier|creative
model.openaiCompatible({ baseURL, model });  // any OpenAI-wire endpoint
model.openai("gpt-4o").output(schema)        // typed structured output (JSON Schema → the engine)
\`\`\`
Providers: openai, anthropic, gemini, mistral, ollama, openrouter, minimax, huggingface, lmstudio.
Keys come from the environment (\`OPENAI_API_KEY\`, …); a missing key fails loud with the exact var.

## Capability tiers

${tiers}

## Component nodes (\`.component(id, components.<kind>({ ...params }))\` — run natively in Rust)

${components}

## Prebuilt agents

${prebuilts}

## Errors

Every error carries a stable \`code\`, a one-line \`hint\` (the fix), and a \`docUrl\`. SDK errors also
offer \`.format()\` (message + hint + docs). Codes include \`AILU_GRAPH_COMPILE\`, \`AILU_UNKNOWN_NODE\`,
\`AILU_GOVERNANCE_MIDDLEWARE_REJECTED\`, \`AILU_RUST_ENGINE_REQUIRED\`, \`AILU_UNKNOWN_PROVIDER\`,
\`AILU_MISSING_PROVIDER_KEY\`, \`AILU_NO_PROVIDER_IN_ENV\`.

## Invariants (governed by construction)

- Deterministic + resumable: checkpoint after every node; resume from the latest checkpoint.
- Human-in-the-loop: \`humanGate\` nodes suspend cleanly and resume on approval.
- Safe: no eval / new Function / dynamic import of user strings; conditions are named predicates;
  agents cannot approve their own outputs; sensitive actions route through approval gates.
- Governance is engine-sealed: a user may only add efficiency middleware (compress/terse/contextBudget).
`;
}
