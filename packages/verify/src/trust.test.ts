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
      expect(result.verdict).toBe("intact");
      expect(result.ok).toBe(false);
    }
  );

  it("reads an authentic pinned capsule with nothing to replay as INTACT, never VERIFIED", async () => {
    const capsule = signedCapsule({ attestation: { records: [] } });
    const result = await verifyCapsule(capsule, { expectedPublicKey: capsule.signature.publicKey });
    expect(result.keyPinned).toBe(true);
    expect(result.signatureValid).toBe(true);
    expect(result.issuerTrusted).toBe(true);
    expect(result.reproducible).toBe(false);
    expect(result.replayValid).toBe(false);
    expect(result.verdict).toBe("intact");
    expect(result.ok).toBe(false);
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

  const gateRecord = () =>
    new Ed25519Attestor().attest({
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

  it("exits 2 on `--key` without a value: a pin was asked for, none was given", async () => {
    const { exit } = await runCli(signedCapsule({ attestation: { records: [] } }), ["--key"]);
    expect(exit).toHaveBeenCalledWith(2);
  });

  // Exit codes: 0 VERIFIED, 1 NOT VERIFIED, 2 usage, 3 INTACT (integrity holds, not proven).
  it.each([
    ["absent", 3, "◐ INTACT — origin not proven"],
    ["empty", 1, "✗ NOT VERIFIED"],
    ["wrong", 1, "✗ NOT VERIFIED"],
    ["trusted", 3, "◐ INTACT — run not re-derived"]
  ] as const)("uses the trust anchor for its verdict and exit status (%s)", async (mode, code, verdict) => {
    const capsule = signedCapsule({ attestation: { records: [] } });
    const keyArgs =
      mode === "absent"
        ? []
        : [
            "--key",
            mode === "trusted"
              ? capsule.signature.publicKey
              : mode === "empty"
                ? ""
                : signedCapsule().signature.publicKey
          ];
    const { output, exit } = await runCli(capsule, keyArgs);
    expect(exit).toHaveBeenCalledWith(code);
    expect(output).toContain(verdict);
    expect(output).not.toContain("✓ VERIFIED");
    expect(output).toContain(mode === "trusted" ? "✓ TRUSTED" : "✗ UNTRUSTED");
    expect(output).toContain("  chain        — not applicable (no records)");
  });

  it("reads an unpinned capsule that attests decisions without reproduction as NOT VERIFIED", async () => {
    const capsule = signedCapsule({ attestation: { records: [gateRecord()] }, reproduction: null });
    const { output, exit } = await runCli(capsule, []);
    expect(exit).toHaveBeenCalledWith(1);
    expect(output).toContain("✗ FAIL — replay missing");
  });

  it.each(["absent", "valid", "broken"])("reports the applicable chain check (%s)", async (mode) => {
    const record = gateRecord();
    if (mode === "broken") record.resolvedBy = "mallory";
    const capsule = signedCapsule(mode === "absent" ? {} : { attestation: { records: [record] } });
    const { output, exit } = await runCli(capsule, ["--key", capsule.signature.publicKey]);
    // The record is signed by its own key, not the issuer's (G2): the chain fails.
    expect(output).toContain(`  chain        ${mode === "absent" ? "— not applicable (no records)" : "✗ FAIL"}`);
    // Nothing to re-derive: INTACT with no record; a record without reproduction is NOT VERIFIED.
    expect(exit).toHaveBeenCalledWith(mode === "absent" ? 3 : 1);
  });
});
