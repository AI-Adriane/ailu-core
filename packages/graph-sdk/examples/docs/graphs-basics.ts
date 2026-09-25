import assert from "node:assert/strict";

// #region example
import { createGraph } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "greeter" })
  // Channels are the run's state: a name, a type and a default.
  .channel("name", { type: "string", default: "" })
  .channel("greeting", { type: "string", default: "" })
  // A node reads the state and returns the channels it changes.
  .node("greet", async (_input, state) => ({ greeting: `Hello, ${state.channels.name}!` }))
  .node("shout", async (_input, state) => ({ greeting: state.channels.greeting.toUpperCase() }))
  // Edges set the order. The first node added is the entry.
  .edge("greet", "shout")
  .compile();

const result = await app.run({ name: "Ada" });
console.log(result.status); // "completed"
console.log(result.channels.greeting); // "HELLO, ADA!"
// #endregion example

assert.equal(result.channels.greeting, "HELLO, ADA!");
