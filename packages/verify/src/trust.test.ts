import { createHash, generateKeyPairSync, sign } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { Ed25519Attestor, type ApprovalRequest } from "@ailu-ai/graph-sdk";
import { afterEach, describe, expect, it, vi } from "vitest";

import { verifyCapsule, type Capsule } from "./verify.js";

const signedCapsule = (body: Record<string, unknown> = {}): Capsule => {
  const { privateKey, publicKey } = generateKeyPairSync("ed25519");
  const digest = createHash("sha256").update(JSON.stringify(body)).digest();
  return {
    ...body,
    signature: {
      algorithm: "ed25519",
      publicKey: publicKey.export({ type: "spki", format: "der" }).toString("base64"),
      contentHash: `sha256:${digest.toString("hex")}`,
      signature: sign(null, digest, privateKey).toString("base64")
    }
  };
};

describe("capsule issuer trust", () => {
  it.each([{}, { attestation: { records: [] }, reproduction: null }])(
    "rejects self-signed evidence without a trust anchor: %j",
    async (body) => {
      const capsule = JSON.parse(JSON.stringify(signedCapsule(body))) as Capsule;
      const result = await verifyCapsule(capsule);
      expect(result.signatureValid).toBe(true);
      expect(result.chainValid).toBe(true);
      expect(result.keyPinned).toBeNull();
      expect(result.issuerTrusted).toBe(false);
      expect(result.ok).toBe(false);
    }
  );

  it("accepts an authentic pinned capsule without claiming it was replayed", async () => {
    const capsule = signedCapsule({ attestation: { records: [] } });
    const result = await verifyCapsule(capsule, { expectedPublicKey: capsule.signature.publicKey });
    expect(result.keyPinned).toBe(true);
    expect(result.signatureValid).toBe(true);
    expect(result.issuerTrusted).toBe(true);
    expect(result.reproducible).toBe(false);
    expect(result.replayValid).toBe(false);
    expect(result.ok).toBe(true);
  });

  it.each(["", signedCapsule().signature.publicKey])("rejects a different pin: %s", async (key) => {
    const result = await verifyCapsule(signedCapsule(), { expectedPublicKey: key });
    expect(result.signatureValid).toBe(true);
    expect(result.keyPinned).toBe(false);
    expect(result.issuerTrusted).toBe(false);
    expect(result.ok).toBe(false);
  });

  it("rejects tampering even when the embedded key matches the pin", async () => {
    const capsule = signedCapsule({ claim: "original" });
    capsule.claim = "forged";
    const result = await verifyCapsule(capsule, { expectedPublicKey: capsule.signature.publicKey });
    expect(result.keyPinned).toBe(true);
    expect(result.signatureValid).toBe(false);
    expect(result.issuerTrusted).toBe(false);
    expect(result.ok).toBe(false);
  });
});

describe("CLI trust verdict", () => {
  const originalArgv = process.argv;
  let directory: string | undefined;

  const runCli = async (capsule: Capsule, keyArgs: string[]) => {
    directory = mkdtempSync(join(process.cwd(), ".verify-cli-"));
    const file = join(directory, "capsule.json");
    writeFileSync(file, JSON.stringify(capsule));
    process.argv = [process.execPath, "ailu-verify", file, ...keyArgs];
    const log = vi.spyOn(console, "log").mockImplementation(() => {});
    const exit = vi.spyOn(process, "exit").mockImplementation(() => {
      throw new Error("CLI exit");
    });
    vi.resetModules();
    await expect(import("./cli.js")).rejects.toThrow("CLI exit");
    return { output: log.mock.calls.flat().join("\n"), exit };
  };

  afterEach(() => {
    process.argv = originalArgv;
    vi.restoreAllMocks();
    if (directory !== undefined) rmSync(directory, { recursive: true, force: true });
  });

  it.each(["absent", "missing value", "empty", "wrong", "trusted"])(
    "uses the trust anchor for its verdict and exit status (%s)",
    async (mode) => {
      const capsule = signedCapsule({ attestation: { records: [] } });
      const keyArgs =
        mode === "absent"
          ? []
          : mode === "missing value"
            ? ["--key"]
            : [
                "--key",
                mode === "trusted"
                  ? capsule.signature.publicKey
                  : mode === "empty"
                    ? ""
                    : signedCapsule().signature.publicKey
              ];
      const { output, exit } = await runCli(capsule, keyArgs);
      expect(exit).toHaveBeenCalledWith(mode === "trusted" ? 0 : 1);
      expect(output).toContain(mode === "trusted" ? "✓ VERIFIED" : "✗ NOT VERIFIED");
      if (mode !== "trusted") expect(output).not.toContain("✓ VERIFIED");
      expect(output).toContain(mode === "trusted" ? "✓ TRUSTED" : "✗ UNTRUSTED");
      expect(output).not.toContain("this run is what it claims to be");
      expect(output).toContain("  chain        — not applicable (no records)");
    }
  );

  it.each(["absent", "valid", "broken"])("reports the applicable chain check (%s)", async (mode) => {
    const record = new Ed25519Attestor().attest({
      id: "approval-1" as ApprovalRequest["id"],
      runId: "run-1" as ApprovalRequest["runId"],
      nodeId: "assistant" as ApprovalRequest["nodeId"],
      requestedBy: "assistant",
      subject: { description: "tool:refund" },
      status: "approved",
      resolvedBy: "alice",
      resolvedAt: new Date("2026-10-29T12:00:00.000Z"),
      createdAt: new Date("2026-10-29T11:59:00.000Z")
    });
    if (mode === "broken") record.resolvedBy = "mallory";
    const capsule = signedCapsule(mode === "absent" ? {} : { attestation: { records: [record] } });
    const { output, exit } = await runCli(capsule, ["--key", capsule.signature.publicKey]);
    expect(output).toContain(`  chain        ${mode === "absent" ? "— not applicable (no records)" : mode === "valid" ? "✓ PASS" : "✗ FAIL"}`);
    expect(exit).toHaveBeenCalledWith(mode === "broken" ? 1 : 0);
  });
});
