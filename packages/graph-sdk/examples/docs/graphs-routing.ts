import assert from "node:assert/strict";

// #region example
import { createGraph } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "triage" })
  .channel("amount", { type: "number", default: 0 })
  .channel("route", { type: "string", default: "" })
  .node("intake", async () => ({}))
  .node("auto", async () => ({ route: "auto-approved" }))
  .node("manual", async () => ({ route: "sent to a human" }))
  // A conditional edge is taken when its named predicate returns true.
  .conditionalEdge("intake", "manual", "isLarge", (state) => state.channels.amount > 1000)
  .conditionalEdge("intake", "auto", "isSmall", (state) => state.channels.amount <= 1000)
  .compile();

console.log((await app.run({ amount: 50 })).channels.route); // "auto-approved"
console.log((await app.run({ amount: 5000 })).channels.route); // "sent to a human"
// #endregion example

assert.equal((await app.run({ amount: 50 })).channels.route, "auto-approved");
assert.equal((await app.run({ amount: 5000 })).channels.route, "sent to a human");
