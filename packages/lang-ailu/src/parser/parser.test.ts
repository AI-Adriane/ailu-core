import { describe, expect, it } from "vitest";

import { buildPromptAST } from "./build-prompt-ast.js";
import { parseYaml } from "./parse-yaml.js";

describe("parser", () => {
  it("parses valid yaml and builds prompt ast", () => {
    const raw = parseYaml(
      `
name: Greeting
template: "Hello {{name}}"
variables:
  - name
`,
      "prompt.yaml"
    );
    const ast = buildPromptAST(raw, "prompt.yaml");
    expect(ast._kind).toBe("prompt");
    expect(ast._loc.file).toBe("prompt.yaml");
    expect(ast.template).toContain("{{name}}");
  });
});
