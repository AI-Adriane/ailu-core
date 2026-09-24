/**
 * Streaming: watch a run while it executes.
 *
 * `app.stream(input, mode)` runs the graph and yields events as they happen:
 * - "messages": the agents' text as it is generated. With a real key you get token deltas;
 *   the offline mock sends each answer as a single delta. `messageId` groups one model turn.
 * - "updates": one event per node as it completes, with the channels that node wrote.
 * (Two more modes exist: "values" yields the full state after each node, "debug" every
 * lifecycle event.)
 *
 * Run it offline: AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/streaming.ts
 * With ANTHROPIC_API_KEY set, watch Claude's answer arrive token by token.
 */
import { createGraph, model } from "@ailu-ai/graph-sdk";

const check = (condition: boolean, label: string): void => {
  if (!condition) throw new Error(`Check failed: ${label}`);
  console.log(`  ✓ ${label}`);
};

const sonnet = model.anthropic("claude-sonnet-4-6");

// Two agents in sequence: one answers, the next titles the answer.
const app = createGraph({ name: "streaming-demo" })
  .channel("question", { type: "string", default: "" })
  .agentNode("answer", {
    model: sonnet,
    prompt: { system: "Answer the question in two sentences." },
    outputChannel: "answerResult"
  })
  .agentNode("title", {
    model: sonnet,
    prompt: { system: "Write a five-word title for the answer in state." },
    outputChannel: "titleResult"
  })
  .edge("answer", "title")
  .compile();

const input = { question: "Why do long-running agent workflows need checkpoints?" };

// ── 1. "messages": print the text as it arrives ──────────────────────────────
console.log("1. messages");
let streamed = "";
let currentNode = "";
for await (const event of app.stream(input, "messages")) {
  if (event.type !== "message_delta") continue;
  if (event.nodeId !== currentNode) {
    currentNode = event.nodeId;
    process.stdout.write(`\n[${currentNode}] `); // a new node started talking
  }
  process.stdout.write(event.delta);
  streamed += event.delta;
}
process.stdout.write("\n");
check(streamed.length > 0, "text streamed while the run executed");

// ── 2. "updates": print progress, one line per completed node ────────────────
console.log("\n2. updates");
const completed: string[] = [];
for await (const event of app.stream(input, "updates")) {
  if (event.type !== "state_update") continue;
  completed.push(event.nodeId);
  console.log(`  ${event.nodeId} done → wrote ${Object.keys(event.delta).join(", ")}`);
}
check(completed.join(" → ") === "answer → title", "one update per node, in execution order");
