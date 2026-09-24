/**
 * Graph validation in Rust: `safeCompile()` checks a graph with the engine's own validator
 * (the native `@ailu-ai/napi` addon) and returns a typed error instead of throwing, so a
 * broken graph never reaches the engine.
 *
 * Run it (build the native addon first if it is missing: `bash scripts/build-napi.sh`):
 *   pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/rust-validation.ts
 */
import { createGraph, rustValidatorActive } from "@ailu-ai/graph-sdk";

console.log("Rust validator active:", rustValidatorActive());

const result = createGraph({ name: "broken" })
  .node("a", async () => ({}))
  .edge("a", "ghost") // dangling edge: there is no node "ghost"
  .safeCompile();

console.log("compile success:", result.success); // false
if (result.success) {
  throw new Error("Check failed: the dangling edge should have been rejected");
}

const codes = result.error.errors.map((error) => error.code);
console.log("errors:", codes); // [ 'INVALID_EDGE_REFERENCE' ]
if (!codes.includes("INVALID_EDGE_REFERENCE")) {
  throw new Error(`Check failed: expected INVALID_EDGE_REFERENCE, got ${codes.join(", ")}`);
}
