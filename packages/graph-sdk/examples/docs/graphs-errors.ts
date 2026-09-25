import assert from "node:assert/strict";

// #region example
import { createGraph } from "@ailu-ai/graph-sdk";

let calls = 0;
const app = createGraph({ name: "flaky-call" })
  .channel("result", { type: "string", default: "" })
  // Retry a failing node up to 3 times, 100 ms apart...
  .node("fetch", {
    retryPolicy: { maxAttempts: 3, backoffMs: 100 },
    handler: async () => {
      calls += 1;
      throw new Error("upstream unavailable");
    }
  })
  // ...then take the error edge instead of failing the run.
  .node("fallback", async () => ({ result: "served from cache" }))
  .errorEdge("fetch", "fallback")
  .compile();

const out = await app.run();
console.log(out.status, out.channels.result); // "completed" "served from cache"
// #endregion example

assert.equal(out.status, "completed");
assert.equal(out.channels.result, "served from cache");
assert.equal(calls, 3);
