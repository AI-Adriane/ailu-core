import { mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { compileGraphFile } from "../../../graph-ailu/src/compiler/compile-graph-file.js";
import { initCommand } from "./init.js";

describe("init command", () => {
  it("generates a valid graph template", async () => {
    const dir = await mkdtemp(join(tmpdir(), "ailu-cli-"));
    const out = join(dir, "new.graph.yaml");
    const code = await initCommand("graph", "demo", out);
    expect(code).toBe(0);
    const content = await readFile(out, "utf8");
    const compiled = compileGraphFile(content, out);
    expect(compiled.result).toBeDefined();
  });
});
