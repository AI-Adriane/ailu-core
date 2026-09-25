import assert from "node:assert/strict";

// #region example
import { createGraph, InMemoryToolRegistry, model, writeTodosTool, type TodoItem } from "@ailu-ai/graph-sdk";

// The built-in planning tool: the agent keeps its plan as a todo list.
const tools = new InMemoryToolRegistry();
tools.register(writeTodosTool.definition, writeTodosTool.handler);

const app = createGraph({ name: "report-writer" })
  .channel("brief", { type: "string", default: "" })
  .channel("plan", { type: "json", default: [] as TodoItem[] })
  .agentNode("writer", {
    model: model.anthropic("claude-sonnet-4-6"),
    prompt: { system: "Plan with writeTodos, then write the report to drafts/report.md." },
    tools,
    todosChannel: "plan", // the plan is saved here after every turn
    enableFs: true, // read_file, write_file, ls, glob, grep, edit_file, ... scoped to this run
    maxIterations: 20
  })
  // What the agent may do with files. Paths no rule matches are read-only.
  .fsPolicy([
    { glob: "drafts/**", verb: "write" },
    { glob: "published/**", verb: "gate" }, // writing here needs a human approval
    { glob: "secrets/**", verb: "deny" }
  ])
  .compile();

const out = await app.run({ brief: "Summarize Q3 incidents for the board." });
console.log(out.channels.plan); // [{ text, status }, ...]
// #endregion example

assert.equal(out.status, "completed");
