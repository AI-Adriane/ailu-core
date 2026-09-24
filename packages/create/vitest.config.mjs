import { defineConfig } from "vitest/config";

// Scoped to this package's own test so the root vitest workspace (packages/*) finds a project
// here instead of erroring on a config-less directory.
export default defineConfig({
  test: { include: ["*.test.mjs"] }
});
