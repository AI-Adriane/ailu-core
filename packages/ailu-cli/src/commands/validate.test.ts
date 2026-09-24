import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { validateCommand } from "./validate.js";

describe("validate command", () => {
  it("returns exit code 1 when diagnostics contain errors", async () => {
    const dir = await mkdtemp(join(tmpdir(), "ailu-cli-"));
    const file = join(dir, "broken.agent.yaml");
    await writeFile(file, "description: missing id and prompt", "utf8");
    const code = await validateCommand(file);
    expect(code).toBe(1);
  });
});
