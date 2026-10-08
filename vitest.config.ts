import { defineConfig } from "vitest/config";

// Root config: aggregates every workspace's vitest project so a single root
// `vitest run --coverage` (`pnpm test:coverage`) collects v8 coverage across all engine packages.
export default defineConfig({
  test: {
    projects: ["packages/*", "plugin/*"],
    coverage: {
      provider: "v8",
      reporter: ["text", "json", "html"],
      reportsDirectory: "coverage"
    }
  }
});
