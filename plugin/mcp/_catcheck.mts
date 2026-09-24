// Throwaway: count the catalog + run a new component on Rust + a new micro-agent live.
import { createGraph, components, prebuilt, ModelPolicy } from "@ailu-ai/graph-sdk";

console.log("components keys:", Object.keys(components).length, JSON.stringify(Object.keys(components)));
console.log("prebuilt keys:", Object.keys(prebuilt).length, JSON.stringify(Object.keys(prebuilt)));

// New components chained, must run natively on Rust.
const app = createGraph({ name: "catcheck" })
  .component("h", components.htmlToText({ from: "html", into: "text" }))
  .component("c", components.textCleaner({ from: "text", into: "clean", lowercase: true, collapseWhitespace: true, trim: true }))
  .edge("h", "c")
  .compile();
const r = await app.run({ html: "  <p>Hello&amp;  WORLD</p>  " });
console.log("component usesRustEngine:", app.usesRustEngine, "status:", r.status, "clean:", JSON.stringify((r.channels as Record<string, unknown>).clean));

// New fast-tier micro-agent, live on Mistral.
const policy = new ModelPolicy();
console.log("fast resolves to:", JSON.stringify(policy.resolve("fast", policy.availableFromEnv())));
const tr = prebuilt.translator({ provider: "mistral" });
const tres = await tr.run({ text: "Translate to French: hello" });
console.log("translator usesRustEngine:", tr.usesRustEngine, "status:", tres.status);
console.log("translation:", JSON.stringify((tres.channels as Record<string, { reasoning?: string }>).translation));
