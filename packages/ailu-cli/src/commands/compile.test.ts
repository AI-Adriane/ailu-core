import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { compileCommand } from "./compile.js";

describe("compile command", () => {
  it("writes compiled output file", async () => {
    const dir = await mkdtemp(join(tmpdir(), "ailu-cli-"));
    const file = join(dir, "flow.graph.yaml");
    await writeFile(
      file,
      `id: g1
version: 1.0.0
name: graph
entryNodeId: n1
channels:
  ctx:
    type: object
    reducer: merge
nodes:
  - id: n1
    type: action
    label: Start
edges: []
`,
      "utf8"
    );
    const outDir = join(dir, "out");
    const code = await compileCommand(file, outDir);
    expect(code).toBe(0);
    const output = await readFile(join(outDir, "flow.graph.json"), "utf8");
    expect(output).toContain('"id": "g1"');
  });
});
