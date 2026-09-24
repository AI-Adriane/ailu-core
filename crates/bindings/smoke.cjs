"use strict";

// Smoke test for the ailu-napi addon.
//
// Part 1 — the original sync JSON helpers (validate / compile / version).
// Part 2 — the async JS<->Rust run bridge: a graph with one JS action node, one
//   named JS condition, and one agent node (mock gateway) that suspends on a gated
//   tool, then is approved and completes via a JS tool callback.

const assert = require("node:assert");
const addon = require("./ailu_napi.node");

console.log("engineVersion:", addon.engineVersion());

// ---------------------------------------------------------------------------
// Part 1: sync helpers (unchanged surface).
// ---------------------------------------------------------------------------

const base = {
  id: "g",
  version: "0.0.0",
  name: "g",
  channels: {},
  entryNodeId: "a",
  nodes: [{ id: "a", type: "action", label: "a" }],
  edges: []
};
console.log("valid →", addon.validateGraphJson(JSON.stringify(base)));

const broken = { ...base, edges: [{ id: "e1", from: "a", to: "ghost", type: "default" }] };
console.log("dangling edge →", addon.validateGraphJson(JSON.stringify(broken)));

const yaml = [
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
].join("\n");
const compiled = JSON.parse(addon.compileGraphYamlJson(yaml));
console.log("compiled graph →", compiled.id, "nodes:", compiled.nodes.length);

try {
  addon.compileGraphYamlJson(
    "id: bad\nentryNodeId: ghost\nnodes:\n  - id: n1\n    type: action\n    label: N1\nedges: []"
  );
  console.log("compile failure → did not throw (unexpected)");
} catch (error) {
  console.log("compile failure →", error.message);
}

// ---------------------------------------------------------------------------
// Part 2: the async run bridge.
// ---------------------------------------------------------------------------

// The graph:
//   prep (JS action node) -- conditional "shouldHandoff" --> assistant (agent node)
// `prep` writes `{ prepared: true }`; the JS condition routes on it; `assistant`
// runs the mock ReAct agent with a gated, JS-backed `refund` tool.
const REFUND = "refund";
const OUTPUT_CHANNEL = "agentResult";
const APPROVED_TOOLS_CHANNEL = "__approvedTools";

const runGraph = {
  id: "bridge-graph",
  version: "0.0.0",
  name: "bridge graph",
  channels: {
    prepared: { type: "json", reducer: "replace" },
    [OUTPUT_CHANNEL]: { type: "json", reducer: "replace" },
    [APPROVED_TOOLS_CHANNEL]: { type: "json", reducer: "replace" }
  },
  nodes: [
    { id: "prep", type: "action", label: "Prep" },
    { id: "assistant", type: "agent", label: "Assistant" }
  ],
  edges: [
    {
      id: "e1",
      from: "prep",
      to: "assistant",
      type: "conditional",
      condition: "shouldHandoff"
    }
  ],
  entryNodeId: "prep"
};

const agentsConfig = {
  assistant: {
    provider: "anthropic",
    system: "You are a refund assistant.",
    toolNames: [REFUND],
    maxIterations: 4,
    suspendForApproval: true,
    approvalToolNames: [REFUND]
  }
};

// Observability: track what Rust called back into JS.
const invoked = {
  jsNode: 0,
  jsCondition: 0,
  jsTool: 0,
  events: [],
  lastConditionState: null,
  lastToolInput: null
};

// A tiny awaited delay, so every callback genuinely suspends on a Promise that
// Rust must drive to resolution (proving async callbacks resolve THROUGH Rust, not
// just synchronous returns dressed up as Promises).
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// on_node handles BOTH custom JS node handlers (kind:"node") and JS tool
// `execute` fns (kind:"tool"). It is **async**: it returns a Promise<string> (the
// channel update / tool result JSON) that Rust awaits.
async function onNode(payloadJson) {
  const payload = JSON.parse(payloadJson);
  if (payload.kind === "node") {
    await delay(5); // genuine async hop before resolving
    invoked.jsNode += 1;
    assert.strictEqual(payload.nodeId, "prep", "JS node should be 'prep'");
    return JSON.stringify({ prepared: true });
  }
  if (payload.kind === "tool") {
    await delay(5); // genuine async hop before resolving
    invoked.jsTool += 1;
    invoked.lastToolInput = payload.input;
    assert.strictEqual(payload.name, REFUND, "JS tool should be 'refund'");
    return JSON.stringify({ refunded: true, amount: 42 });
  }
  throw new Error(`unexpected on_node payload kind: ${payload.kind}`);
}

// on_condition: a named JS predicate. It is **async**: it returns a Promise that
// resolves to a boolean-ish string ("true"/"false") which Rust awaits and reads.
async function onCondition(payloadJson) {
  const payload = JSON.parse(payloadJson);
  await delay(5); // genuine async hop before resolving
  invoked.jsCondition += 1;
  invoked.lastConditionState = payload.state;
  assert.strictEqual(payload.name, "shouldHandoff", "condition name should round-trip");
  // Route to the agent only once `prep` set `prepared: true` (proves the JS
  // condition sees the JS-node-written state). Resolve to "true"/"false".
  const route = payload.state && payload.state.prepared === true;
  return route ? "true" : "false";
}

// on_event: fire-and-forget run-lifecycle sink.
function onEvent(payloadJson) {
  invoked.events.push(JSON.parse(payloadJson).type);
}

async function main() {
  // --- Start: should suspend on the gated refund tool. ---
  const startSpec = {
    graph: runGraph,
    runId: "smoke-run",
    initialData: {},
    agents: agentsConfig,
    jsNodeIds: ["prep"],
    jsToolNames: [REFUND]
  };

  const startOutJson = await addon.engineRun(
    JSON.stringify(startSpec),
    onNode,
    onCondition,
    onEvent
  );
  const startOut = JSON.parse(startOutJson);

  console.log("\n--- engineRun (start) ---");
  console.log("status →", startOut.status);
  console.log("currentNodeId →", startOut.state.currentNodeId);
  console.log("pendingApprovals →", JSON.stringify(startOut.pendingApprovals));
  console.log("events →", invoked.events.join(", "));
  console.log("jsNode calls →", invoked.jsNode, "| jsCondition calls →", invoked.jsCondition);

  assert.ok(invoked.jsNode >= 1, "the JS node closure must be invoked from Rust");
  assert.ok(invoked.jsCondition >= 1, "the JS condition must be invoked from Rust");
  assert.strictEqual(
    invoked.lastConditionState.prepared,
    true,
    "condition saw the JS-node-written state"
  );
  assert.strictEqual(startOut.status, "suspended", "agent must suspend on the gated tool");
  assert.strictEqual(
    startOut.state.currentNodeId,
    "assistant",
    "suspended at the agent node (routed there via the JS condition)"
  );
  assert.strictEqual(invoked.jsTool, 0, "the gated tool must NOT run before approval");
  assert.strictEqual(startOut.pendingApprovals.length, 1, "exactly one pending approval");
  assert.strictEqual(
    startOut.pendingApprovals[0].subject,
    "tool:refund",
    "the pending approval is for tool:refund"
  );
  assert.ok(
    invoked.events.includes("run_suspended"),
    "a run_suspended event was forwarded to onEvent"
  );

  // --- Approve refund and resume: should complete, running the JS tool. ---
  const approveSpec = {
    graph: runGraph,
    state: startOut.state, // the serialized suspended state, fed straight back
    approvedTools: [REFUND],
    agents: agentsConfig,
    jsNodeIds: ["prep"],
    jsToolNames: [REFUND]
  };

  const eventsBefore = invoked.events.length;
  const approveOutJson = await addon.engineApproveAndResume(
    JSON.stringify(approveSpec),
    onNode,
    onCondition,
    onEvent
  );
  const approveOut = JSON.parse(approveOutJson);

  console.log("\n--- engineApproveAndResume(['refund']) ---");
  console.log("status →", approveOut.status);
  console.log("jsTool calls →", invoked.jsTool, "| lastToolInput →", JSON.stringify(invoked.lastToolInput));
  console.log("events →", invoked.events.slice(eventsBefore).join(", "));

  assert.strictEqual(approveOut.status, "completed", "run completes after approval");
  assert.strictEqual(invoked.jsTool, 1, "the JS tool callback executed exactly once after approval");
  assert.strictEqual(approveOut.pendingApprovals.length, 0, "no pending approvals after completion");
  assert.ok(
    invoked.events.slice(eventsBefore).includes("run_completed"),
    "a run_completed event was forwarded to onEvent on resume"
  );

  console.log("\nbridge smoke: ALL ASSERTIONS PASSED");

  await componentNodeSmoke();
}

// ---------------------------------------------------------------------------
// Part 3: native component node feeding an agent node.
//
// The graph:
//   builder (promptBuilder COMPONENT node) -- default --> writer (agent node)
// `builder` declares `componentNodes.builder = { kind:"promptBuilder", params }`,
// so it runs the NATIVE Rust component (it renders `{{topic}}` from the initial
// channel into `prompt`) — NOT the JS `on_node` seam. `writer` is a tool-less mock
// agent, so it finalizes and the run completes. We assert (a) the component ran on
// Rust (its `prompt` channel is set to the rendered text), (b) the run completed on
// the mock gateway, and (c) the JS node seam was NEVER called for `builder`.
// ---------------------------------------------------------------------------
async function componentNodeSmoke() {
  console.log("\n--- engineRun (component node) ---");

  const PROMPT_CHANNEL = "prompt";
  const componentGraph = {
    id: "component-graph",
    version: "0.0.0",
    name: "component graph",
    channels: {
      topic: { type: "json", reducer: "replace" },
      [PROMPT_CHANNEL]: { type: "json", reducer: "replace" },
      [OUTPUT_CHANNEL]: { type: "json", reducer: "replace" }
    },
    nodes: [
      { id: "builder", type: "action", label: "Prompt builder" },
      { id: "writer", type: "agent", label: "Writer" }
    ],
    edges: [{ id: "e1", from: "builder", to: "writer", type: "default" }],
    entryNodeId: "builder"
  };

  // Track whether the JS node seam is (wrongly) invoked for the component node.
  const seam = { jsNodeForBuilder: 0 };
  async function onNodeComponent(payloadJson) {
    const payload = JSON.parse(payloadJson);
    if (payload.kind === "node" && payload.nodeId === "builder") {
      seam.jsNodeForBuilder += 1; // must stay 0: the component runs on Rust
    }
    return JSON.stringify({});
  }
  const noopCondition = async () => "true";
  const events = [];
  const onEventComponent = (payloadJson) => events.push(JSON.parse(payloadJson).type);

  const spec = {
    graph: componentGraph,
    runId: "component-run",
    initialData: { topic: "refunds" },
    // The component node is declared here; even though it is ALSO a plausible JS
    // node id, the bridge routes it to the native Rust component handler.
    componentNodes: {
      builder: {
        kind: "promptBuilder",
        params: { template: "Write about {{topic}}.", into: PROMPT_CHANNEL }
      }
    },
    agents: {
      writer: { provider: "anthropic", system: "You write short notes.", toolNames: [] }
    },
    jsNodeIds: ["builder"],
    jsToolNames: []
  };

  const outJson = await addon.engineRun(
    JSON.stringify(spec),
    onNodeComponent,
    noopCondition,
    onEventComponent
  );
  const out = JSON.parse(outJson);

  console.log("status →", out.status);
  console.log("prompt channel →", JSON.stringify(out.state.channels[PROMPT_CHANNEL]));
  console.log("jsNode-for-builder calls →", seam.jsNodeForBuilder, "(must be 0)");

  assert.strictEqual(
    out.state.channels[PROMPT_CHANNEL],
    "Write about refunds.",
    "the native promptBuilder component rendered {{topic}} into the prompt channel"
  );
  assert.strictEqual(
    seam.jsNodeForBuilder,
    0,
    "the component node ran on Rust, never via the JS on_node seam"
  );
  assert.strictEqual(out.status, "completed", "the run completes on the mock gateway");
  assert.ok(out.state.channels[OUTPUT_CHANNEL], "the agent wrote its result channel");
  assert.ok(events.includes("run_completed"), "a run_completed event was forwarded");

  console.log("\ncomponent smoke: ALL ASSERTIONS PASSED");
}

main().then(
  () => process.exit(0),
  (error) => {
    console.error("\nbridge smoke FAILED:", error && error.stack ? error.stack : error);
    process.exit(1);
  }
);
