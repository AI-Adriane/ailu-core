import { configDefaults, defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "node",
    // Vitest 4 no longer excludes dist/ by default; the compiled test copies there must not run.
    exclude: [...configDefaults.exclude, "**/dist/**"]
    // No global AILU_LLM_MOCK: the fixtures turn offline mode on only while they RECORD a run, so
    // every verification runs the way `npx @ailu-ai/verify` does, with no key and no mock.
  }
});
