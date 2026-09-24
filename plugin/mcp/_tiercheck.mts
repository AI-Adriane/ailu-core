// Throwaway: prove the model policy + a fast-tier prebuilt agent on Mistral-only, on Rust.
import { ModelPolicy, prebuilt } from "@ailu/graph-sdk";

const policy = new ModelPolicy();
const available = policy.availableFromEnv();
console.log("available providers:", JSON.stringify(available));
for (const tier of ["fast", "balanced", "frontier", "creative"] as const) {
  console.log(`resolve(${tier}) ->`, JSON.stringify(policy.resolve(tier, available)));
}

const app = prebuilt.summarizer();
console.log("usesRustEngine:", app.usesRustEngine);
const result = await app.run({
  text: "Ailu is a stateful, resumable graph runtime that checkpoints after every node, emits lifecycle events for every transition, suspends cleanly at human-approval gates, and resumes from the last checkpoint."
});
console.log("status:", result.status);
console.log("channels:", JSON.stringify(result.channels).slice(0, 700));
