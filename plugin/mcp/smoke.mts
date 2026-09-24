// Smoke test for the Ailu MCP server.
//
// Spawns server.mts as a real stdio MCP server via the EXACT .mcp.json launch
// command (`node --import tsx <server.mts>`) using the official MCP SDK client,
// performs initialize + tools/list, then exercises the governance loop end to end
// ON THE RUST ENGINE.
//
// Routing: the server opts into Rust by default (process.env.AILU_SDK_ENGINE ??=
// "rust"), so run_agent / approve_and_resume / run_graph all execute on the Rust
// engine via @ailu/napi. We pin AILU_MCP_SMOKE_OFFLINE=1 so the Rust agent path
// builds its DETERMINISTIC offline mock gateway instead of calling a live LLM (a live
// model's tool choices are non-deterministic, so it could skip the gated refund tool).
// The proof that execution is on Rust — not the deprecated in-process TypeScript engine
// — is assertion (0): the SDK's TS-fallback console.warn is NEVER emitted to stderr.
//
//   (0) the deprecated-TS-engine warning is ABSENT from the server's stderr (proves
//       every run executed on the Rust engine, not the TS fallback).
//   (1) tools/list includes run_agent, approve_and_resume, run_graph, list_agents,
//       validate_graph, compile_graph_yaml.
//   (2) run_agent('refunder') -> status 'suspended' with exactly one pending approval
//       whose subject mentions refund.
//   (3) approve_and_resume on that runId granting the refund -> status 'completed'
//       and the agent no longer requires human review (the gate cleared on Rust).
//   (4) run_agent('researcher') -> completes with a FINAL answer.
//   (5) run_graph('publish-flow') -> suspends at the human gate; greeter completes.
//   (6) validate_graph on a dangling-edge graph -> INVALID_EDGE_REFERENCE.
//
// Run: node --import tsx smoke.mts   (exit 0 = all assertions passed)

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { readFileSync } from "node:fs";

const here = dirname(fileURLToPath(import.meta.url));
const serverPath = join(here, "server.mts");

function assert(condition: boolean, message: string): void {
  if (!condition) {
    console.error(`FAIL: ${message}`);
    process.exit(1);
  }
  console.log(`ok: ${message}`);
}

type ToolCallResult = { content?: Array<{ type: string; text?: string }>; isError?: boolean };

/** Extract the first text block from an MCP tool result. */
function resultText(result: ToolCallResult): string {
  const block = (result.content ?? []).find((c) => c.type === "text");
  return block?.text ?? "";
}

function resultJson(result: ToolCallResult): Record<string, unknown> {
  return JSON.parse(resultText(result)) as Record<string, unknown>;
}

async function main(): Promise<void> {
  // Use the EXACT command from plugin/.mcp.json so the smoke test proves the real
  // launch path: `node --import tsx <CLAUDE_PLUGIN_ROOT>/mcp/server.mts`.
  const mcpConfig = JSON.parse(readFileSync(join(here, "..", ".mcp.json"), "utf8")) as {
    ailu: { command: string; args: string[] };
  };
  const launchArgs = mcpConfig.ailu.args.map((a) =>
    a.replace("${CLAUDE_PLUGIN_ROOT}", join(here, ".."))
  );
  console.log(`launch: ${mcpConfig.ailu.command} ${launchArgs.join(" ")}`);

  // Force the Rust engine explicitly (the server defaults to it, but we pin it so the
  // smoke proves the Rust path regardless of any inherited value), and pin the offline
  // guard so the Rust agent path uses its deterministic mock gateway. `stderr: "pipe"`
  // lets us read the child's stderr to assert the TS-fallback warning is absent.
  const childEnv: Record<string, string> = {};
  for (const [key, value] of Object.entries(process.env)) {
    if (typeof value === "string") childEnv[key] = value;
  }
  childEnv.AILU_SDK_ENGINE = "rust";
  childEnv.AILU_MCP_SMOKE_OFFLINE = "1";

  const transport = new StdioClientTransport({
    command: mcpConfig.ailu.command === "node" ? process.execPath : mcpConfig.ailu.command,
    args: launchArgs,
    env: childEnv,
    stderr: "pipe"
  });

  // Accumulate the server child's stderr so we can assert the deprecated-TS-engine
  // warning never fires (the proof that execution stayed on the Rust engine).
  let serverStderr = "";
  transport.stderr?.on("data", (chunk: Buffer) => {
    serverStderr += chunk.toString("utf8");
  });

  const client = new Client({ name: "ailu-smoke", version: "0.1.0" }, { capabilities: {} });

  await client.connect(transport);
  console.log("ok: initialize handshake completed");

  // (1) tools/list
  const { tools } = await client.listTools();
  const names = tools.map((t) => t.name).sort();
  console.log("tools/list ->", names.join(", "));
  for (const required of [
    "run_agent",
    "approve_and_resume",
    "run_graph",
    "list_agents",
    "validate_graph",
    "compile_graph_yaml"
  ]) {
    assert(names.includes(required), `tools/list includes ${required}`);
  }

  // list_agents sanity (shows the gateway mode + registry).
  const listRes = (await client.callTool({ name: "list_agents", arguments: {} })) as ToolCallResult;
  const listJson = resultJson(listRes);
  console.log("list_agents mode ->", listJson.mode);
  assert(
    Array.isArray(listJson.agents) && (listJson.agents as unknown[]).length === 3,
    "list_agents returns 3 agents"
  );

  // (2) run_agent('refunder') -> suspended with exactly one pending approval (refund).
  const refundRun = (await client.callTool({
    name: "run_agent",
    arguments: { agent: "refunder", input: { orderId: "ord-1" } }
  })) as ToolCallResult;
  const refundJson = resultJson(refundRun);
  console.log("run_agent(refunder) ->", JSON.stringify(refundJson, null, 2));
  assert(!refundRun.isError, "run_agent(refunder) did not error");
  assert(refundJson.status === "suspended", "run_agent(refunder) status is 'suspended'");
  const pending = refundJson.pendingApprovals as Array<{
    id: string;
    subject: string;
    requestedBy: string;
  }>;
  assert(Array.isArray(pending) && pending.length === 1, "exactly one pending approval");
  assert(
    typeof pending[0]?.subject === "string" && pending[0].subject.toLowerCase().includes("refund"),
    `pending approval subject mentions refund (got '${pending[0]?.subject}')`
  );
  const refundRunId = refundJson.runId as string;
  const refundApprovalId = pending[0]?.id;

  // (3) approve_and_resume granting the refund -> completed and refund executed.
  const resumeRes = (await client.callTool({
    name: "approve_and_resume",
    arguments: {
      runId: refundRunId,
      approvalId: refundApprovalId,
      approvedBy: "alice",
      approvedTools: ["refund"]
    }
  })) as ToolCallResult;
  const resumeJson = resultJson(resumeRes);
  console.log("approve_and_resume(refunder) ->", JSON.stringify(resumeJson, null, 2));
  assert(!resumeRes.isError, "approve_and_resume did not error");
  assert(resumeJson.status === "completed", "approve_and_resume status is 'completed'");
  const resumeResult = resumeJson.result as
    | { reasoning?: string; requiresHumanReview?: boolean }
    | undefined;
  assert(
    resumeResult?.requiresHumanReview === false,
    "resumed agent no longer requires human review"
  );
  assert(
    typeof resumeResult?.reasoning === "string" &&
      resumeResult.reasoning.includes('"refunded":true'),
    "the refund executed on resume (observation reflects refunded:true)"
  );

  // (4) run_agent('researcher') -> completes with a FINAL answer.
  //
  // NOTE: this runs on the Rust engine's deterministic offline mock gateway, whose
  // ReAct trace text differs from the TS mock's (only AgentResult.reasoning *text*
  // differs across engines — the structural contract is identical, per the SDK's
  // fidelity test). The Rust mock produces a FINAL answer but not the TS mock's
  // "[cite: <id>]" tag, so we assert the structural fact (completed + FINAL), not the
  // engine-specific citation string. With a real provider key (no offline guard) the
  // live model produces a genuine cited answer.
  const researchRun = (await client.callTool({
    name: "run_agent",
    arguments: { agent: "researcher", input: { question: "how does governance work?" } }
  })) as ToolCallResult;
  const researchJson = resultJson(researchRun);
  console.log("run_agent(researcher) ->", JSON.stringify(researchJson, null, 2));
  assert(!researchRun.isError, "run_agent(researcher) did not error");
  assert(researchJson.status === "completed", "run_agent(researcher) status is 'completed'");
  const researchResult = researchJson.result as { reasoning?: string } | undefined;
  assert(
    typeof researchResult?.reasoning === "string" && researchResult.reasoning.includes("FINAL:"),
    "researcher produced a FINAL answer"
  );

  // (5) run_graph('publish-flow') suspends at the human gate; greeter completes.
  const graphRun = (await client.callTool({
    name: "run_graph",
    arguments: { graph: "publish-flow" }
  })) as ToolCallResult;
  const graphJson = resultJson(graphRun);
  console.log("run_graph(publish-flow) -> status", graphJson.status);
  assert(!graphRun.isError, "run_graph(publish-flow) did not error");
  assert(graphJson.status === "suspended", "run_graph(publish-flow) suspends at the human gate");

  const greeterRun = (await client.callTool({
    name: "run_graph",
    arguments: { graph: "greeter", input: { name: "Ailu" } }
  })) as ToolCallResult;
  const greeterJson = resultJson(greeterRun);
  console.log("run_graph(greeter) -> status", greeterJson.status);
  assert(!greeterRun.isError, "run_graph(greeter) did not error");
  assert(greeterJson.status === "completed", "run_graph(greeter) completes on the Rust engine");

  // (6) validate_graph on a dangling-edge graph -> INVALID_EDGE_REFERENCE.
  const badGraph = {
    id: "g",
    version: "1.0.0",
    name: "g",
    channels: {},
    entryNodeId: "a",
    nodes: [{ id: "a", type: "action", label: "A" }],
    edges: [{ id: "e1", from: "a", to: "ghost", type: "default" }]
  };
  const validateRes = (await client.callTool({
    name: "validate_graph",
    arguments: { definitionJson: JSON.stringify(badGraph) }
  })) as ToolCallResult;
  const validateText = resultText(validateRes);
  console.log("validate_graph ->", validateText);
  assert(!validateRes.isError, "validate_graph did not error");
  assert(
    validateText.includes("INVALID_EDGE_REFERENCE"),
    "validate_graph reports INVALID_EDGE_REFERENCE for a dangling edge"
  );

  // (0) The deprecated-TS-engine warning must be ABSENT — the proof that every
  // run_agent / approve_and_resume / run_graph above executed on the Rust engine and
  // never fell back to the in-process TypeScript engine. Close the client first so the
  // child exits and flushes any remaining stderr, then give the PassThrough a tick to
  // drain before reading the buffer.
  await client.close();
  await new Promise((resolve) => setTimeout(resolve, 50));

  const TS_WARNING = "Executing on the deprecated in-process TypeScript engine";
  if (serverStderr.trim().length > 0) {
    console.log("server stderr ->", JSON.stringify(serverStderr));
  }
  assert(
    !serverStderr.includes(TS_WARNING),
    "no deprecated-TS-engine warning on stderr (all runs executed on the Rust engine)"
  );

  console.log("\nALL SMOKE ASSERTIONS PASSED");
  process.exit(0);
}

main().catch((error: unknown) => {
  console.error(`smoke failed: ${error instanceof Error ? error.stack : String(error)}`);
  process.exit(1);
});
