import { existsSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { scaffold, SDK_VERSION } from "./index.mjs";

describe("@ailu-ai/create", () => {
  it("scaffolds a runnable governed starter", () => {
    const tmp = mkdtempSync(join(tmpdir(), "adr-scaffold-"));
    const { appName, dir } = scaffold("my-app", tmp);

    expect(appName).toBe("my-app");
    expect(existsSync(join(dir, "app.ts"))).toBe(true);
    expect(existsSync(join(dir, "inspect.ts"))).toBe(true);
    expect(existsSync(join(dir, "README.md"))).toBe(true);

    const pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
    expect(pkg.dependencies["@ailu-ai/graph-sdk"]).toBeTruthy();
    expect(pkg.scripts.start).toContain("app.ts");

    // The starter leads with governance (a human gate) + the inspector.
    expect(readFileSync(join(dir, "app.ts"), "utf8")).toContain("humanGate");
    expect(readFileSync(join(dir, "inspect.ts"), "utf8")).toContain("serveInspector");
  });

  it("starts new apps on the SDK's current minor version", () => {
    const sdk = JSON.parse(readFileSync(new URL("../graph-sdk/package.json", import.meta.url), "utf8"));
    const [major, minor] = sdk.version.split(".");
    expect(SDK_VERSION).toBe(`^${major}.${minor}.0`);
  });

  it("sanitizes the name and refuses an existing directory", () => {
    const tmp = mkdtempSync(join(tmpdir(), "adr-scaffold-"));
    expect(scaffold("weird/../name", tmp).appName).toBe("weird-..-name");
    scaffold("dup", tmp);
    expect(() => scaffold("dup", tmp)).toThrow(/already exists/);
  });
});
