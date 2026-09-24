import assert from "node:assert/strict";

// #region example
import { createGraph, type RunEvent } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "kyc" })
  // noLog: the value is checkpointed as usual but masked in every run event and log.
  .channel("passport", { type: "string", default: "", noLog: true })
  .channel("verified", { type: "boolean", default: false })
  .node("check", async (_input, state) => ({ verified: state.channels.passport.length > 0 }))
  .compile();

const events: RunEvent[] = [];
app.onEvent((event) => events.push(event));

const out = await app.run({ passport: "X1234567" });
console.log(out.channels.verified); // true
console.log(JSON.stringify(events).includes("X1234567")); // false
// #endregion example

assert.equal(out.channels.verified, true);
assert.equal(JSON.stringify(events).includes("X1234567"), false);
