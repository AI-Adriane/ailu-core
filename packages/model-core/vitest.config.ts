import { configDefaults, defineConfig } from "vitest/config";

// A local config so the per-package `vitest run` (turbo `pnpm test`) uses it rather than
// resolving the root vitest workspace. model-core's tests only need a node environment.
export default defineConfig({
  // Vitest 4 no longer excludes dist/ by default; the compiled test copies there must not run.
  test: { environment: "node", exclude: [...configDefaults.exclude, "**/dist/**"] }
});
