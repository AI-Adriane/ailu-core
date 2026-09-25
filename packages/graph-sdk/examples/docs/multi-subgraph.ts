import assert from "node:assert/strict";

// #region example
import { createGraph } from "@ailu-ai/graph-sdk";

// A child graph with its own channels...
const normalize = createGraph({ name: "normalize" })
  .channel("text", { type: "string", default: "" })
  .channel("clean", { type: "string", default: "" })
  .node("trim", async (_input, state) => ({ clean: state.channels.text.trim().toLowerCase() }));

// ...used as one node of a parent graph. Mappings say which channels cross the boundary,
// written destination: source.
const app = createGraph({ name: "intake" })
  .channel("raw", { type: "string", default: "" })
  .channel("normalized", { type: "string", default: "" })
  .subgraph("normalize", normalize, {
    inputMapping: { text: "raw" }, // child `text` <- parent `raw`
    outputMapping: { normalized: "clean" } // parent `normalized` <- child `clean`
  })
  .compile();

console.log((await app.run({ raw: "  HELLO World " })).channels.normalized); // "hello world"
// #endregion example

assert.equal((await app.run({ raw: "  HELLO World " })).channels.normalized, "hello world");
