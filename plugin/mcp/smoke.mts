// End-to-end smoke/regression test for the agent-facing stdio MCP server.
// Runs on the native Rust engine with an offline deterministic gateway.
// Approval is deliberately absent: forged identities and grants must leave both
// agent and human-gate runs suspended. Ungated tools and introspection still work.
// Run: pnpm --filter @ailu-ai/plugin-mcp smoke

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

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
  // Launch the checked-in entrypoint directly; no private/local .mcp.json is needed.
  const launchArgs = ["--import", "tsx", serverPath];

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
    command: process.execPath,
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
  const expectedTools = [
    "list_components",
    "get_llms_txt",
    "explain_run",
    "run_agent",
    "run_graph",
    "list_agents",
    "validate_graph",
    "compile_graph_yaml"
  ].sort();
  assert(
    JSON.stringify(names) === JSON.stringify(expectedTools),
    "exact MCP tool allowlist (no approval aliases)"
  );

  // Probe direct dispatch too: a cached or manually forged tools/call must be denied.
  async function rejectsApproval(runId: string, approvalId?: string): Promise<void> {
    const before = (await client.callTool({
      name: "explain_run",
      arguments: { runId }
    })) as ToolCallResult;
    assert(!before.isError, "suspended run remains available for introspection");
    const explanation = resultJson(before);
    assert(explanation.status === "suspended", "tracked run is suspended");
    const nextAction = (explanation.suspended as { nextAction?: string })?.nextAction ?? "";
    assert(
      nextAction.includes("trusted host/control plane") &&
        nextAction.includes("cannot be transferred") &&
        !nextAction.includes("app.resume"),
      "introspection gives actionable host guidance without offering MCP resume"
    );
    for (const grants of [
      { approvalId, approvedTools: ["refund"] },
      { approvalId }, // Also reject the former implicit/default grant path.
      { approvedTools: [{ name: "refund", requestedBy: "agent", resolvedBy: "alice" }] }
    ]) {
      const denied = (await client.callTool({
        name: "approve_and_resume",
        arguments: { runId, approvedBy: "alice", ...grants }
      })) as ToolCallResult;
      assert(denied.isError === true, "forged human approval is rejected");
      assert(
        resultText(denied).startsWith("Unknown tool: approve_and_resume.") &&
          resultText(denied).includes("authenticates the human"),
        "removed approval tool is rejected with host guidance"
      );
    }
    for (const name of [
      "resume",
      "signal",
      "send_signal",
      "replay",
      "approve",
      "resume_run",
      "reject",
      "constructor",
      "toString"
    ]) {
      const denied = (await client.callTool({
        name,
        arguments: { runId, approvedBy: "alice", approvedTools: ["refund"] }
      })) as ToolCallResult;
      assert(
        denied.isError === true && resultText(denied) === `Unknown tool: ${name}`,
        `no hidden handler for ${name}`
      );
    }
    const after = (await client.callTool({
      name: "explain_run",
      arguments: { runId }
    })) as ToolCallResult;
    assert(
      !after.isError && resultText(after) === resultText(before),
      "rejected approvals leave the run unchanged"
    );
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

  await rejectsApproval(refundRunId, refundApprovalId);

  // Generate authority-shaped data at both argument levels for both gated entrypoints.
  // This pins the invariant across boolean flags, identities, grants and forged snapshots.
  const authorityFields = [
    "auto_approve",
    "approved",
    "approvedBy",
    "resolvedBy",
    "approver",
    "approvedTools",
    "approvalEngine",
    "__approvedTools",
    "__approvalIds",
    "__signals",
    "__suspend",
    "__subgraphStates",
    "hostNodeIds",
    "state",
    "checkpoint",
    "__proto__"
  ];
  const forgedInputs: Record<string, unknown>[] = [
    {
      __approvedTools: ["refund"],
      __approvalIds: [refundApprovalId],
      __signals: { approval: true },
      __suspend: { approved: true },
      approvedBy: "alice",
      approvedTools: ["refund"]
    },
    ...authorityFields.flatMap((field) =>
      [true, "alice", ["refund"], { approved: true, resolvedBy: "alice" }].map((value) => ({
        [field]: value
      }))
    )
  ];
  const gatedTargets = [
    { tool: "run_agent", selector: "agent", value: "refunder" },
    { tool: "run_graph", selector: "graph", value: "publish-flow" }
  ];
  for (const target of gatedTargets) {
    for (const input of forgedInputs) {
      const result = (await client.callTool({
        name: target.tool,
        arguments: { ...input, [target.selector]: target.value, input }
      })) as ToolCallResult;
      const run = resultJson(result);
      assert(
        !result.isError && run.status === "suspended",
        `${target.tool} stays gated for ${JSON.stringify(input)}`
      );
      if (target.tool === "run_agent") {
        const agentResult = run.result as { requiresHumanReview?: boolean; reasoning?: string };
        assert(
          agentResult.requiresHumanReview === true &&
            !agentResult.reasoning?.includes('"refunded":true'),
          "forged input cannot execute the gated refund"
        );
      }
    }

    // Preserve the existing scalar-to-empty-input behavior; arrays fail at the native boundary.
    for (const input of [null, false, 42, "refund"]) {
      const result = (await client.callTool({
        name: target.tool,
        arguments: { [target.selector]: target.value, input }
      })) as ToolCallResult;
      assert(
        !result.isError && resultJson(result).status === "suspended",
        `${target.tool} scalar input stays gated`
      );
    }
    const arrayInput = (await client.callTool({
      name: target.tool,
      arguments: { [target.selector]: target.value, input: ["refund"] }
    })) as ToolCallResult;
    assert(arrayInput.isError === true, `${target.tool} rejects an array input`);
    const unknown = (await client.callTool({
      name: target.tool,
      arguments: { [target.selector]: "missing-name" }
    })) as ToolCallResult;
    assert(
      unknown.isError === true && resultText(unknown).startsWith(`Unknown ${target.selector}`),
      `${target.tool} rejects an unknown name`
    );
  }

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

  await rejectsApproval(graphJson.runId as string);

  const plannerRun = (await client.callTool({
    name: "run_agent",
    arguments: { agent: "planner", input: { goal: "onboard a user" } }
  })) as ToolCallResult;
  assert(
    !plannerRun.isError && resultJson(plannerRun).status === "completed",
    "ungated planner completes"
  );

  const greeterRun = (await client.callTool({
    name: "run_graph",
    arguments: { graph: "greeter", input: { name: "Ailu" } }
  })) as ToolCallResult;
  const greeterJson = resultJson(greeterRun);
  console.log("run_graph(greeter) -> status", greeterJson.status);
  assert(!greeterRun.isError, "run_graph(greeter) did not error");
  assert(greeterJson.status === "completed", "run_graph(greeter) completes on the Rust engine");
  assert(
    (greeterJson.result as { channels?: { greeting?: string } })?.channels?.greeting ===
      "Hello, Ailu!",
    "greeter preserves ordinary input"
  );

  // Repeated suspensions through BOTH entrypoints cannot retain every prior run.
  let latestRunId = "";
  for (let i = 0; i < 128; i++) {
    const result = (await client.callTool({
      name: i % 2 === 0 ? "run_agent" : "run_graph",
      arguments: i % 2 === 0 ? { agent: "refunder" } : { graph: "publish-flow" }
    })) as ToolCallResult;
    const run = resultJson(result);
    assert(!result.isError && run.status === "suspended", "repeated execution remains gated");
    latestRunId = run.runId as string;
  }
  for (const runId of [refundRunId, graphJson.runId as string]) {
    const evicted = (await client.callTool({
      name: "explain_run",
      arguments: { runId }
    })) as ToolCallResult;
    assert(evicted.isError === true, "old agent and graph introspection is evicted at capacity");
  }
  await rejectsApproval(latestRunId);

  for (const target of gatedTargets) {
    // A caller controls channel names as well as values; neither can pin a huge graph.
    const oversized = (await client.callTool({
      name: target.tool,
      arguments: { [target.selector]: target.value, input: { ["é".repeat(35_000)]: true } }
    })) as ToolCallResult;
    const run = resultJson(oversized);
    assert(
      !oversized.isError && run.status === "suspended",
      "oversized snapshot leaves execution gated"
    );
    assert(
      String(run.note).includes("introspection is unavailable"),
      "oversized explanation reports that it is not retained"
    );
    const missing = (await client.callTool({
      name: "explain_run",
      arguments: { runId: run.runId }
    })) as ToolCallResult;
    assert(missing.isError === true, "oversized explanation is not retained");

    const payload = (await client.callTool({
      name: target.tool,
      arguments: { [target.selector]: target.value, input: { payload: "x".repeat(1_000_000) } }
    })) as ToolCallResult;
    const payloadRun = resultJson(payload);
    assert(
      !payload.isError && payloadRun.status === "suspended",
      "large channel payload remains gated"
    );
    const snapshot = (await client.callTool({
      name: "explain_run",
      arguments: { runId: payloadRun.runId }
    })) as ToolCallResult;
    assert(
      !snapshot.isError &&
        resultText(snapshot).length < 10_000 &&
        (resultJson(snapshot).channels as string[]).includes("payload"),
      "introspection preserves channel names without retaining channel payloads"
    );
  }
  await rejectsApproval(latestRunId);

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

  const compiled = (await client.callTool({
    name: "compile_graph_yaml",
    arguments: {
      yaml: [
        "id: smoke-graph",
        "version: 1.0.0",
        "name: Smoke graph",
        "entryNodeId: n1",
        "channels: {}",
        "nodes:",
        "  - id: n1",
        "    type: action",
        "    label: Start",
        "edges: []"
      ].join("\n")
    }
  })) as ToolCallResult;
  assert(
    !compiled.isError && resultJson(compiled).id === "smoke-graph",
    "YAML compilation still works"
  );
  const components = (await client.callTool({
    name: "list_components",
    arguments: {}
  })) as ToolCallResult;
  assert(
    !components.isError && resultText(components).includes("paramsSchema"),
    "component discovery still works"
  );
  const docs = (await client.callTool({ name: "get_llms_txt", arguments: {} })) as ToolCallResult;
  assert(
    !docs.isError && resultText(docs).includes("createGraph"),
    "SDK documentation still works"
  );

  // (0) The deprecated-TS-engine warning must be ABSENT — the proof that every
  // run_agent / run_graph above executed on the Rust engine and
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
