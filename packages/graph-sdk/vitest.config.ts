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
    environment: "node"
  },
  resolve: {
    alias: {
      // Resolve the SDK's own package name to source too, so example files (which
      // import the public `@ailu/graph-sdk` entry) and their tests exercise current
      // source instead of a possibly-stale `dist/`.
      "@ailu/graph-sdk": fromHere("./src/index.ts"),
      "@ailu/graph-core": fromHere("../graph-core/src/index.ts"),
      "@ailu/graph-runtime": fromHere("../graph-runtime/src/index.ts"),
      "@ailu/agents-core": fromHere("../agents-core/src/index.ts"),
      "@ailu/llm-gateway": fromHere("../llm-gateway/src/index.ts"),
      "@ailu/model-core": fromHere("../model-core/src/index.ts"),
      // ADR 0037: now re-exported through the index, so tests resolving the index need them too.
      "@ailu/approval-engine": fromHere("../approval-engine/src/index.ts"),
      "@ailu/artifact-store": fromHere("../artifact-store/src/index.ts"),
      "@ailu/search": fromHere("../search/src/index.ts"),
      "@ailu/memory-store": fromHere("../memory-store/src/index.ts"),
      // The deprecated DSL compilers, now re-exported through the door for browser-safe YAML compile.
      "@ailu/graph-ailu": fromHere("../graph-ailu/src/index.ts"),
      "@ailu/lang-ailu": fromHere("../lang-ailu/src/index.ts")
    }
  }
});
