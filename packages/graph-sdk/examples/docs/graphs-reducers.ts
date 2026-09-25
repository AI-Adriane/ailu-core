import assert from "node:assert/strict";

// #region example
import { createGraph } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "audit-log" })
  // "append" adds each update to the list instead of replacing it.
  .channel("log", { type: "string[]", reducer: "append", default: [] as string[] })
  .node("open", async () => ({ log: ["opened"] }))
  .node("check", async () => ({ log: ["checked"] }))
  .node("close", async () => ({ log: ["closed"] }))
  .edge("open", "check")
  .edge("check", "close")
  .compile();

console.log((await app.run()).channels.log); // ["opened", "checked", "closed"]
// #endregion example

assert.deepEqual((await app.run()).channels.log, ["opened", "checked", "closed"]);
