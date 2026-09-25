import assert from "node:assert/strict";

// #region example
import { createGraph } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "pipeline" })
  .channel("n", { type: "number", default: 0 })
  .node("a", async () => ({ n: 1 }))
  .node("b", async () => ({ n: 2 }))
  .edge("a", "b")
  .compile();

// Every run emits lifecycle events: node_started, node_completed, run_suspended, run_completed, ...
const unsubscribe = app.onEvent((event) => console.log(event.type));
await app.run();
unsubscribe();
// #endregion example

const seen: string[] = [];
app.onEvent((event) => seen.push(event.type));
await app.run();
assert.ok(seen.includes("node_completed"));
