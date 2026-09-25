/**
 * Resume a run in another process.
 *
 * `app.resume(runId)` only works on the CompiledGraph instance that started the run: its
 * checkpoints live in memory. To suspend a run in one process and finish it in another (a web
 * request that stops at a human gate, a worker that resumes it hours later), run the graph's
 * definition on the catalog path and store the suspended state yourself. It is plain JSON.
 *
 * Process 1 drafts a reply and suspends at a human gate; its state is saved as if written to a
 * database. A reviewer approves. Process 2 loads it and finishes the run; the resume checks the
 * approval first.
 *
 * Run it offline: AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/resume-across-processes.ts
 * With ANTHROPIC_API_KEY set, the agent drafts a real reply.
 */
import {
  createGraph,
  finalAnswer,
  InMemoryApprovalEngine,
  model,
  resumeCatalogGraph,
  runCatalogGraph,
  type AgentResult,
  type GraphState
} from "@ailu-ai/graph-sdk";

const check = (condition: boolean, label: string): void => {
  if (!condition) throw new Error(`Check failed: ${label}`);
  console.log(`  ✓ ${label}`);
};

// An agent drafts a reply; a human reviews it before the run completes.
const app = createGraph({ name: "support-reply" })
  .channel("ticket", { type: "string", default: "" })
  .agentNode("draft", {
    model: model.anthropic("claude-sonnet-4-6"),
    prompt: { system: "Draft a short, polite reply to the support ticket." },
    outputChannel: "reply"
  })
  .humanGate("review")
  .edge("draft", "review")
  .compile();

// Shared by both processes. In production both are your database: a persistent ApprovalEngine
// and a table of suspended run states.
const approvals = new InMemoryApprovalEngine();
const database = new Map<string, string>();

// ── Process 1: start the run; it suspends at the review gate ─────────────────
const started = await runCatalogGraph(app.definition, {
  initialData: { ticket: "My invoice shows the wrong billing address." },
  approvalEngine: approvals // files one approval request for the gate
});
check(started.status === "suspended", "process 1: the run suspended at the review gate");
database.set("run-42", JSON.stringify(started.state)); // the whole state is plain JSON

// ── Out of band: a reviewer approves. Their identity is recorded on the request. ─
const [request] = await approvals.getPending(started.state.runId);
if (request === undefined) throw new Error("Check failed: no approval request was filed");
await approvals.approve(request.id, "alice@example.com");

// ── Process 2: load the state and resume ─────────────────────────────────────
const state = JSON.parse(database.get("run-42") ?? "{}") as GraphState;

// With the approval engine, the resume checks it first: it throws while a request the run waits
// on is pending or was rejected. For an approval-gated TOOL (`requiresApproval: true`), also pass
// the grant: approvedTools: [{ name: "refund", requestedBy: "draft", resolvedBy: "alice@example.com" }].
// It must match a request the engine recorded as approved by that same person.
const finished = await resumeCatalogGraph(app.definition, state, { approvalEngine: approvals });
check(finished.status === "completed", "process 2: the run completed");
check((await approvals.getById(request.id))?.resolvedBy === "alice@example.com", "the engine records who approved");

const reply = finalAnswer(finished.state.channels.reply as AgentResult | undefined);
check(reply.length > 0, "the drafted reply survived the round trip");
console.log(`\nReply: ${reply}`);
