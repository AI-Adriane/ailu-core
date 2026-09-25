/**
 * A deep agent: it plans, works in files, and delegates.
 *
 * - Planning: the `writeTodos` tool. Each call replaces the agent's todo list, and the engine
 *   saves the latest list to a channel in the same checkpoint as the agent's result, so later
 *   nodes (and your UI) can read the plan.
 * - A governed filesystem: `enableFs: true` gives the agent read_file, write_file, ls, glob,
 *   grep, edit_file, delete_file and move_file over a virtual filesystem scoped to the run.
 *   `.fsPolicy()` decides what it may do per path; an unmatched path is read-only.
 * - Delegation: `taskNode` runs a sub-agent in an isolated context. Only the objective goes in
 *   and only the sub-agent's report comes back.
 *
 * Offline, the mock calls writeTodos once with an empty plan and never touches the files (it
 * only calls the tools you declare). With ANTHROPIC_API_KEY set, the lead agent writes a real
 * plan and keeps its notes under notes/.
 *
 * Run it offline: AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/deep-agent.ts
 */
import {
  createGraph,
  finalAnswer,
  InMemoryToolRegistry,
  model,
  TODOS_CHANNEL,
  writeTodosTool,
  type TodoItem
} from "@ailu-ai/graph-sdk";

const check = (condition: boolean, label: string): void => {
  if (!condition) throw new Error(`Check failed: ${label}`);
  console.log(`  ✓ ${label}`);
};

const sonnet = model.anthropic("claude-sonnet-4-6");

const tools = new InMemoryToolRegistry();
tools.register(writeTodosTool.definition, writeTodosTool.handler);

const app = createGraph({ name: "deep-agent" })
  .channel("objective", { type: "string", default: "" })
  // The durable plan: null until the agent first calls writeTodos.
  .channel(TODOS_CHANNEL, { type: "json", default: null as TodoItem[] | null })
  .fsPolicy([
    { glob: "notes/**", verb: "write" }, // the agent's scratch space
    { glob: "secrets/**", verb: "deny" } // never readable, never writable
  ])
  .agentNode("lead", {
    model: sonnet,
    prompt: {
      system:
        "Plan the work with writeTodos before anything else and keep it up to date. " +
        "Keep your working notes in files under notes/."
    },
    tools,
    todosChannel: TODOS_CHANNEL, // where the engine saves the latest plan
    enableFs: true,
    maxIterations: 8,
    outputChannel: "leadResult"
  })
  // Delegate: reads the `objective` channel, writes its report to the `report` channel.
  .taskNode("research", {
    subAgent: {
      model: sonnet,
      prompt: { system: "Research the objective and report back in five bullet points." }
    }
  })
  .edge("lead", "research")
  .compile();

const out = await app.run({ objective: "Compare three open-source vector databases for a small team." });
const plan = out.channels[TODOS_CHANNEL];

check(out.status === "completed", "the run completed");
check(Array.isArray(plan), "the plan was saved to the durable todos channel");
check(finalAnswer(out.channels.leadResult).length > 0, "the lead agent answered");
check(finalAnswer(out.channels.report).length > 0, "the delegated sub-agent returned its report");

console.log("\nPlan:");
for (const todo of plan ?? []) console.log(`  [${todo.status}] ${todo.text}`);
if (plan?.length === 0) console.log("  (empty: the offline mock calls writeTodos with no items)");
console.log(`\nResearch report: ${finalAnswer(out.channels.report)}`);
