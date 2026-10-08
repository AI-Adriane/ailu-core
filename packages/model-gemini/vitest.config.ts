import { fileURLToPath } from "node:url";

import { configDefaults, defineConfig } from "vitest/config";

const fromHere = (relativePath: string): string => fileURLToPath(new URL(relativePath, import.meta.url));

// Resolve the shared model base to source (mirrors the tsconfig path alias), so tests
// exercise current source rather than a possibly-stale dist/.
export default defineConfig({
  // Vitest 4 no longer excludes dist/ by default; the compiled test copies there must not run.
  test: { environment: "node", exclude: [...configDefaults.exclude, "**/dist/**"] },
  resolve: {
    alias: {
      "@ailu-ai/model-core": fromHere("../model-core/src/index.ts")
    }
  }
});
