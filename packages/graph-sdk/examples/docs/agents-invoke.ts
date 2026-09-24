import assert from "node:assert/strict";

// #region example
import { model } from "@ailu-ai/graph-sdk";

// One call, no graph. The answer text is on `.content`.
const reply = await model.anthropic("claude-sonnet-4-6").invoke("Say hello in French.");
console.log(reply.content, reply.usage);
// #endregion example

// #region typed
// Ask for JSON that matches a schema, and get it parsed.
export const classify = async (ticket: string) => {
  const triage = model.openai("gpt-4o").output({
    jsonSchema: {
      type: "object",
      properties: { category: { type: "string", enum: ["billing", "bug", "other"] } },
      required: ["category"]
    },
    parse: (value) => value as { category: "billing" | "bug" | "other" }
  });
  const { parsed } = await triage.invoke(`Classify: ${ticket}`);
  return parsed.category;
};
// #endregion typed

assert.ok(typeof reply.content === "string");
assert.equal(typeof classify, "function");
