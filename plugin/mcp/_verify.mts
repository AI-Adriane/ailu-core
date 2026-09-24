// Throwaway: prove the SDK defaults to Rust (NO AILU_SDK_ENGINE set) + live Mistral.
import { createGraph } from "@ailu-ai/graph-sdk";

const warnings: string[] = [];
const origWarn = console.warn.bind(console);
console.warn = (...a: unknown[]) => {
  warnings.push(a.map(String).join(" "));
};

const app = createGraph({ name: "default-rust" })
  .agentNode("assistant", {
    llm: undefined as never,
    provider: "mistral",
    model: "mistral-small-latest",
    prompt: { system: "Answer in one short sentence, then a new line: FINAL: <answer>" }
  })
  .compile();

console.warn = origWarn;
const events: string[] = [];
app.onEvent((e) => events.push(e.type));
const result = await app.run({ question: "In one sentence: what is a checkpoint in a workflow engine?" });
const ar = (result.channels as Record<string, { reasoning?: string }>).agentResult;

console.log("AILU_SDK_ENGINE env:", JSON.stringify(process.env.AILU_SDK_ENGINE ?? "(unset)"));
console.log("usesRustEngine:", app.usesRustEngine);
console.log("fallback-warning fired:", warnings.some((w) => w.includes("TypeScript engine")));
console.log("status:", result.status);
console.log("events:", events.join(","));
console.log("reasoning:", ar?.reasoning ?? "(none)");
