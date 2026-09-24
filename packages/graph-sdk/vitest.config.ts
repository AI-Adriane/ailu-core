import { fileURLToPath } from "node:url";

import { defineConfig } from "vitest/config";

const fromHere = (relativePath: string): string => fileURLToPath(new URL(relativePath, import.meta.url));

/**
 * Resolve workspace dependencies to their TypeScript source rather than the built
 * `dist/` (which can be stale). This mirrors the tsconfig path aliases the rest of
 * the repo relies on, so the SDK's tests always exercise current source.
 */
export default defineConfig({
  test: {
    environment: "node",
    // Offline mode: an agent with no API key runs on the engine's deterministic mock instead of
    // failing. The tests that check the keyless failure clear it themselves.
    env: { AILU_LLM_MOCK: "1" }
  },
  resolve: {
    alias: {
      // Resolve the SDK's own package name to source too, so example files (which
      // import the public `@ailu-ai/graph-sdk` entry) and their tests exercise current
      // source instead of a possibly-stale `dist/`.
      "@ailu-ai/graph-sdk": fromHere("./src/index.ts"),
      "@ailu-ai/graph-core": fromHere("../graph-core/src/index.ts"),
      "@ailu-ai/graph-runtime": fromHere("../graph-runtime/src/index.ts"),
      "@ailu-ai/agents-core": fromHere("../agents-core/src/index.ts"),
      "@ailu-ai/llm-gateway": fromHere("../llm-gateway/src/index.ts"),
      "@ailu-ai/model-core": fromHere("../model-core/src/index.ts"),
      // ADR 0037: now re-exported through the index, so tests resolving the index need them too.
      "@ailu-ai/approval-engine": fromHere("../approval-engine/src/index.ts"),
      "@ailu-ai/artifact-store": fromHere("../artifact-store/src/index.ts"),
      "@ailu-ai/search": fromHere("../search/src/index.ts"),
      "@ailu-ai/memory-store": fromHere("../memory-store/src/index.ts"),
      // The deprecated DSL compilers, now re-exported through the door for browser-safe YAML compile.
      "@ailu-ai/graph-ailu": fromHere("../graph-ailu/src/index.ts"),
      "@ailu-ai/lang-ailu": fromHere("../lang-ailu/src/index.ts")
    }
  }
});
