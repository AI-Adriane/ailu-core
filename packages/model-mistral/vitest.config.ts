import { fileURLToPath } from "node:url";

import { defineConfig } from "vitest/config";

const fromHere = (relativePath: string): string => fileURLToPath(new URL(relativePath, import.meta.url));

// Resolve the shared model base to source (mirrors the tsconfig path alias), so tests
// exercise current source rather than a possibly-stale dist/.
export default defineConfig({
  test: { environment: "node" },
  resolve: {
    alias: {
      "@ailu/model-core": fromHere("../model-core/src/index.ts")
    }
  }
});
