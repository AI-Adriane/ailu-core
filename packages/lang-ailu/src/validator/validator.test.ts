import { describe, expect, it } from "vitest";

import { buildAgentAST } from "../parser/build-agent-ast.js";
import { validateAgentAST } from "./validate-agent-ast.js";

describe("validator", () => {
  it("returns diagnostics with location for invalid ast", () => {
    const ast = buildAgentAST({ description: "missing id and prompt" }, "agent.yaml");
    const diagnostics = validateAgentAST(ast);
    expect(diagnostics.length).toBeGreaterThan(0);
    expect(diagnostics[0]?.loc.file).toBe("agent.yaml");
    expect(diagnostics[0]?.severity).toBe("error");
  });
});
