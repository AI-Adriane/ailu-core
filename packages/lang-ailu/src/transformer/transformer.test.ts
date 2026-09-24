import { describe, expect, it } from "vitest";

import { buildPromptAST } from "../parser/build-prompt-ast.js";
import { transformPrompt } from "./transform-prompt.js";

describe("transformer", () => {
  it("transforms and renders template with truncate filter", () => {
    const ast = buildPromptAST(
      {
        name: "P",
        template: "Hello {{name | truncate: 3}}",
        variables: ["name"]
      },
      "prompt.yaml"
    );
    const template = transformPrompt(ast);
    const rendered = template.render({ name: "Ailu" });
    expect(rendered.content).toBe("Hello Ail");
  });

  it("reports warning for unresolved variable", () => {
    const ast = buildPromptAST(
      {
        name: "P",
        template: "Hello {{missing}}",
        variables: []
      },
      "prompt.yaml"
    );
    const template = transformPrompt(ast);
    expect(template.diagnostics.some((d) => d.severity === "warning")).toBe(true);
  });
});
