import { createHash, generateKeyPairSync, sign } from "node:crypto";

import {
  Ed25519Attestor,
  InMemoryApprovalEngine,
  exampleGraphs,
  runCatalogGraph,
  rustEngineAvailable,
  type ApprovalRequest,
  type AttestationRecord
} from "@ailu-ai/graph-sdk";
import { describe, expect, it } from "vitest";

import { verifyCapsule, type Capsule } from "./verify.js";

/** How the signer's decisions are attested in a capsule. */
type Attesting = {
  /** Sign each decision's call key (ADR 0051 D2); off = a record attested before ADR 0051. */
  signCallKey: boolean;
  /** Change the request before it is signed — to sign another call than the one the run asked. */
  alter?: (request: ApprovalRequest) => ApprovalRequest;
  /** Leave the capsule's unsigned decision summary (`replay.decisions.attested`) empty. */
  emptySummary?: boolean;
};

/**
 * Build a genuine capsule the way the control plane does: run the demo graph, approve and attest
 * what it filed, then Ed25519-sign the bundle.
 */
const buildCapsule = async (
  attesting: Attesting = { signCallKey: true }
): Promise<Capsule> => {
  process.env.AILU_LLM_RECORD ??= "1";
  const def = exampleGraphs().find((g) => g.slug === "approval-demo")?.definition;
  if (def === undefined) throw new Error("approval-demo example graph not found");
  const engine = new InMemoryApprovalEngine();
  const outcome = await runCatalogGraph(def, {
    initialData: { input: "Refund ORD-8830 (duplicate charge)." },
    onEvent: () => {},
    providerKeys: {},
    approvalEngine: engine
  });
  const attestor = new Ed25519Attestor(undefined, { signCallKey: attesting.signCallKey });
  const records: AttestationRecord[] = [];
  for (const pending of await engine.getPending()) {
    const approved = await engine.approve(pending.id, "alice");
    const signed = attesting.alter === undefined ? approved : attesting.alter(approved);
    records.push(attestor.attest(signed, records.at(-1)?.payloadHash ?? null));
  }
  const body = {
    schemaVersion: "1",
    reproduction: {
      entryCheckpointId: "verify-test-run:entry",
      entryState: outcome.entryState,
      journal: JSON.parse(outcome.replayJournal ?? "{}"),
      graphDefinition: def
    },
    attestation: { records },
    replay: {
      decisions: {
        attested: attesting.emptySummary
          ? []
          : (outcome.pendingApprovals ?? []).map((p) => ({ subject: p.subject }))
      }
    }
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

  it("fails a replay of another call than the one signed (ADR 0051 D3)", async () => {
    // The attested record signs a different refund than the run asks for: the chain and the
    // capsule signature hold, the replay does not reproduce the signed call.
    const capsule = await buildCapsule({
      signCallKey: true,
      alter: (request) => ({
        ...request,
        subject: { ...request.subject, callKey: `refund#${"0".repeat(64)}` } as never
      })
    });
    const result = await verifyCapsule(capsule);
    expect(result.signatureValid).toBe(true);
    expect(result.chainValid).toBe(true);
    if (rustEngineAvailable()) {
      expect(result.replayValid).toBe(false);
      expect(result.ok).toBe(false);
    }
  });

  it("verifies evidence attested before call keys, by its subjects", async () => {
    const capsule = await buildCapsule({ signCallKey: false });
    expect(capsule.attestation?.records?.every((record) => record.callKey === undefined)).toBe(
      true
    );
    const result = await verifyCapsule(capsule);
    if (rustEngineAvailable()) {
      expect(result.replayValid).toBe(true);
      expect(result.ok).toBe(true);
    }
  });

  it("reads the attested decisions from the verified records, not the unsigned summary", async () => {
    // The capsule's unsigned `replay.decisions.attested` says nothing was decided; the signed
    // records say the refund was. The verdict follows the records (ADR 0051 D3, R8).
    const capsule = await buildCapsule({ signCallKey: true, emptySummary: true });
    const records = capsule.attestation?.records ?? [];
    expect(records.length).toBeGreaterThan(0);
    const result = await verifyCapsule(capsule);
    if (rustEngineAvailable()) {
      expect(result.replayValid).toBe(true);
    }
  });

  it("rejects when the signing key does not match the pinned key", async () => {
    const capsule = await buildCapsule();
    const result = await verifyCapsule(capsule, { expectedPublicKey: "a-different-known-key" });
    expect(result.keyPinned).toBe(false);
    expect(result.ok).toBe(false);
  });
});
