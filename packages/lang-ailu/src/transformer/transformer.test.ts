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

  it("renders padded placeholders and stays fast on a long unterminated one", () => {
    const padded = transformPrompt(
      buildPromptAST({ name: "P", template: "Hi {{   name   }}!", variables: ["name"] }, "prompt.yaml")
    );
    expect(padded.render({ name: "Ailu" }).content).toBe("Hi Ailu!");

    const hostile = `{{${" ".repeat(50_000)}`;
    const started = Date.now();
    const rendered = transformPrompt(
      buildPromptAST({ name: "P", template: hostile, variables: [] }, "prompt.yaml")
    ).render({});
    expect(rendered.content).toBe(hostile);
    expect(Date.now() - started).toBeLessThan(1000);
  });
});
