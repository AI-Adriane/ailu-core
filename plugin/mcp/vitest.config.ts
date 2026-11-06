import { configDefaults, defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "node",
    // Vitest 4 no longer excludes dist/ by default; the compiled test copies there must not run.
    exclude: [...configDefaults.exclude, "**/dist/**"]
  }
});
