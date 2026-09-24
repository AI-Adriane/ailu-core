import { createHash, generateKeyPairSync, sign } from "node:crypto";

import {
  InMemoryApprovalEngine,
  exampleGraphs,
  runCatalogGraph,
  rustEngineAvailable
} from "@ailu-ai/graph-sdk";
import { describe, expect, it } from "vitest";

import { verifyCapsule, type Capsule } from "./verify.js";

/** Build a genuine capsule the way the control plane does: run the demo graph, then Ed25519-sign it. */
const buildCapsule = async (): Promise<Capsule> => {
  process.env.AILU_LLM_RECORD ??= "1";
  const def = exampleGraphs().find((g) => g.slug === "approval-demo")?.definition;
  if (def === undefined) throw new Error("approval-demo example graph not found");
  const outcome = await runCatalogGraph(def, {
    initialData: { input: "Refund ORD-8830 (duplicate charge)." },
    onEvent: () => {},
    providerKeys: {},
    approvalEngine: new InMemoryApprovalEngine()
  });
  const body = {
    schemaVersion: "1",
    reproduction: {
      entryCheckpointId: "verify-test-run:entry",
      entryState: outcome.entryState,
      journal: JSON.parse(outcome.replayJournal ?? "{}"),
      graphDefinition: def
    },
    attestation: { records: [] },
    replay: { decisions: { attested: (outcome.pendingApprovals ?? []).map((p) => ({ subject: p.subject })) } }
  };
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
  } as Capsule;
};

describe("verifyCapsule", () => {
  it("verifies a genuine capsule — signature holds, and replay reproduces the attested decisions", async () => {
    const capsule = await buildCapsule();
    const result = await verifyCapsule(capsule);
    expect(result.signatureValid).toBe(true);
    expect(result.chainValid).toBe(true); // empty chain is vacuously valid
    if (rustEngineAvailable()) {
      // The load-bearing claim: re-derivation reproduces the attested decision (needs the native engine).
      expect(result.reproducible).toBe(true);
      expect(result.replayValid).toBe(true);
      expect(result.ok).toBe(true);
    }
  });

  it("fails a tampered capsule — the signature breaks", async () => {
    const capsule = await buildCapsule();
    if (capsule.reproduction != null) {
      capsule.reproduction.entryState = { tampered: true };
    }
    const result = await verifyCapsule(capsule);
    expect(result.signatureValid).toBe(false);
    expect(result.ok).toBe(false);
  });

  it("rejects when the signing key does not match the pinned key", async () => {
    const capsule = await buildCapsule();
    const result = await verifyCapsule(capsule, { expectedPublicKey: "a-different-known-key" });
    expect(result.keyPinned).toBe(false);
    expect(result.ok).toBe(false);
  });
});
