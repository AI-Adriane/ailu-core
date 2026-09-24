import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "node",
    // The fixture run has no API key: offline mode runs its agent on the engine's
    // deterministic mock instead of failing.
    env: { AILU_LLM_MOCK: "1" }
  }
});
