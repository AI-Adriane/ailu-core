import { describe, expect, it } from "vitest";

import { compileFile } from "./compile-file.js";

describe("compiler", () => {
  it("compiles prompt file through full pipeline", () => {
    const compiled = compileFile(
      `
name: Demo
template: "Hello {{name}}"
variables: [name]
`,
      "demo.prompt.yaml"
    );
    expect(compiled.result).toBeDefined();
    expect(compiled.diagnostics.filter((d) => d.severity === "error")).toHaveLength(0);
  });
});
