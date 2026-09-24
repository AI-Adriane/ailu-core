import { createHash, createPublicKey, verify as edVerify } from "node:crypto";

import {
  replayCatalogGraph,
  verifyChain,
  verifyReplayDecisions,
  type AttestationRecord,
  type GraphDefinition,
  type GraphState
} from "@ailu-ai/graph-sdk";

/** The detached Ed25519 signature over the bundle-minus-signature (sha256 → sign). */
interface CapsuleSignature {
  algorithm: string;
  publicKey: string;
  contentHash: string;
  signature: string;
}

/** Raw reproduction data: the run's entry state + pure journal + the graph topology it executed. */
interface CapsuleReproduction {
  entryCheckpointId: string;
  entryState: unknown;
  journal: unknown;
  graphDefinition: unknown;
}

/** A `GET /runs/:id/audit-export` bundle — the Certificate of Execution capsule. */
export interface Capsule {
  signature: CapsuleSignature;
  attestation?: { records?: AttestationRecord[] };
  reproduction?: CapsuleReproduction | null;
  replay?: { decisions?: { attested?: Array<{ subject: string }> } };
  [key: string]: unknown;
}

export interface VerifyResult {
  /** The Ed25519 signature over the bundle is intact (content not altered). */
  signatureValid: boolean;
  /** The signing key matches the one you pinned (only checked when `expectedPublicKey` is given). */
  keyPinned: boolean | null;
  /** The approval attestation chain is hash-linked + every signature valid. */
  chainValid: boolean;
  /** Re-deriving the run from {graph, entryState, journal} reproduces the attested decisions. */
  replayValid: boolean;
  /** The capsule carried enough to replay (graph + entry state + journal). */
  reproducible: boolean;
  /** The signing public key (SPKI DER, base64) — compare to a known org key to anchor trust. */
  publicKey: string;
  /** Overall verdict: every applicable check passed. */
  ok: boolean;
  /** Human-readable notes (warnings, why a check is N/A, replay mismatches). */
  notes: string[];
}

/**
 * Verify an Ailu Certificate of Execution OFFLINE, from the capsule alone — no control plane, no
 * trust in the issuer. Three independent checks:
 *  1. signature — Ed25519 over sha256(bundle-minus-signature) against the embedded key;
 *  2. chain — the approval attestation chain is hash-linked + every signature valid;
 *  3. replay — re-derive the run on the OSS engine from {graph, entryState, journal} and confirm it
 *     reaches the SAME attested decisions ("don't trust us — re-run it yourself").
 *
 * Trust anchor: the signature is checked against the key EMBEDDED in the capsule. Pass
 * `expectedPublicKey` (a key you obtained out-of-band) to PIN it — otherwise a forger could re-sign a
 * doctored capsule with their own key and it would still read "valid". When unpinned, `keyPinned` is
 * null and a note says so.
 */
export async function verifyCapsule(
  capsule: Capsule,
  opts: { expectedPublicKey?: string } = {}
): Promise<VerifyResult> {
  const notes: string[] = [];

  // 1. Signature over the canonical bundle-minus-signature (the documented recipe).
  const { signature, ...body } = capsule;
  const digest = createHash("sha256").update(JSON.stringify(body)).digest();
  const hashMatches = signature?.contentHash === `sha256:${digest.toString("hex")}`;
  let signatureValid = false;
  try {
    const key = createPublicKey({ key: Buffer.from(signature.publicKey, "base64"), format: "der", type: "spki" });
    signatureValid = hashMatches && edVerify(null, digest, key, Buffer.from(signature.signature, "base64"));
  } catch {
    signatureValid = false;
  }
  if (!signatureValid) notes.push("Signature does NOT verify — the capsule was altered or is malformed.");

  // Trust anchor: pin the key if the caller supplied a known one.
  let keyPinned: boolean | null = null;
  if (opts.expectedPublicKey !== undefined) {
    keyPinned = signature?.publicKey === opts.expectedPublicKey;
    if (!keyPinned) notes.push("Signing key does NOT match the pinned key — signed by a different party.");
  } else {
    notes.push("Key not pinned: signature checked against the capsule's own key. Pass --key <known-key> to anchor trust.");
  }

  // 2. Attestation chain.
  const records = capsule.attestation?.records ?? [];
  const chainValid = verifyChain(records);
  if (records.length === 0) notes.push("No attestation records (the run had no governed approvals).");
  else if (!chainValid) notes.push("Attestation chain is broken — records altered, dropped, or reordered.");

  // 3. Replay re-derivation on the OSS engine.
  let replayValid = false;
  let reproducible = false;
  const repro = capsule.reproduction;
  if (repro != null && repro.graphDefinition != null && repro.entryState !== undefined) {
    reproducible = true;
    try {
      const outcome = await replayCatalogGraph(
        repro.graphDefinition as GraphDefinition,
        repro.entryState as GraphState,
        repro.entryCheckpointId,
        JSON.stringify(repro.journal)
      );
      const attested = (capsule.replay?.decisions?.attested ?? []).map((d) => ({ status: "", subject: d.subject }));
      const replayed = (outcome.pendingApprovals ?? []).map((p) => ({ status: "", subject: p.subject }));
      const verdict = verifyReplayDecisions(attested, replayed);
      replayValid = verdict.ok;
      if (!replayValid) {
        notes.push(`Replay did NOT reproduce the attested decisions (${verdict.mismatches.length} mismatch(es)).`);
      }
    } catch (error) {
      notes.push(`Replay could not run: ${error instanceof Error ? error.message : String(error)} (the native engine may be unavailable for this platform).`);
    }
  } else {
    notes.push("No reproduction data (graph + entry state) in the capsule — replay skipped.");
  }

  const ok =
    signatureValid &&
    (keyPinned ?? true) &&
    chainValid &&
    (reproducible ? replayValid : true);

  return {
    signatureValid,
    keyPinned,
    chainValid,
    replayValid,
    reproducible,
    publicKey: signature?.publicKey ?? "",
    ok,
    notes
  };
}
