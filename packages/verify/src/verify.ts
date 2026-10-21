import { createHash, createPublicKey, verify as edVerify } from "node:crypto";

import {
  attestedDecisions,
  replayCatalogGraph,
  replayedDecisions,
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

/**
 * The overall verdict:
 * - `verified` — every check passed AND the signing key is the one you pinned: the run is what it
 *   claims to be, from the issuer you trust;
 * - `intact` — every check that could run passed, but something is not proven: without a pinned
 *   key, the origin (anyone can sign a capsule and its chain with their own key); without
 *   reproduction data in a capsule that attests no decision, the run itself (nothing re-derived);
 * - `not-verified` — a check failed.
 */
export type Verdict = "verified" | "intact" | "not-verified";

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
  /** The overall verdict — see {@link Verdict}. */
  verdict: Verdict;
  /**
   * `verdict === "verified"`. Since 2.7.3 it needs a pinned key: a capsule checked against its own
   * embedded key reads `intact`, `ok: false`.
   */
  ok: boolean;
  /** Human-readable notes (warnings, why a check is N/A, replay mismatches). */
  notes: string[];
}

/**
 * The deploy-level services the engine builds from the process environment, whatever the run mode,
 * and that a replay may reach as a live run would: the external PII and secrets redactors and the
 * LLMLingua compressor rewrite each model request before the replay matches it against the journal,
 * and the rerank endpoint re-scores a `reranker` node's candidates. None is pinned by the capsule: a
 * service the verifier's environment points to may receive the capsule's content and shape what the
 * replay reproduces. A replay never calls the LLM provider (2.7.4); it may call these. The HTTP
 * filesystem backend is not among them: a replay is refused outright while it is set
 * ({@link FS_BACKEND_ENV}).
 */
const REPLAY_REACHABLE_ENV = [
  "AILU_PII_REDACTOR_URL",
  "AILU_SECRETS_REDACTOR_URL",
  "AILU_LLMLINGUA_URL",
  "AILU_RERANK_ENDPOINT"
] as const;

/**
 * The HTTP filesystem backend the engine gives an fs-enabled agent whatever the run mode. The fs
 * tools are native — the journal does not serve them — so a replay would WRITE the capsule's
 * content to the service this variable names: a side effect, not a read. While it is set, the
 * verifier refuses to replay and the capsule reads NOT VERIFIED.
 */
const FS_BACKEND_ENV = "AILU_FS_BACKEND_URL";

/**
 * Whether {@link FS_BACKEND_ENV} is set, read as the engine reads it: any value but the empty
 * string, whitespace only included (the engine then builds a client on that value).
 */
const fsBackendSet = (): boolean => (process.env[FS_BACKEND_ENV] ?? "") !== "";

/**
 * The services of {@link REPLAY_REACHABLE_ENV} set in this process's environment, read as the
 * engine reads them: any value but the empty string, whitespace only included — the engine then
 * builds a client on that value, and a replay may diverge on it. (The rerank endpoint ignores a
 * blank value; it is noted all the same, an over-warning, never a missed one.)
 */
const replayReachableServices = (): string[] =>
  REPLAY_REACHABLE_ENV.filter((name) => (process.env[name] ?? "") !== "");

/**
 * Verify an Ailu Certificate of Execution OFFLINE, from the capsule alone — no control plane, no
 * trust in the issuer. Three independent checks:
 *  1. signature — Ed25519 over sha256(bundle-minus-signature) against the embedded key;
 *  2. chain — the approval attestation chain is hash-linked + every signature valid;
 *  3. replay — re-derive the run on the OSS engine from {graph, entryState, journal} and confirm it
 *     reaches the SAME attested decisions ("don't trust us — re-run it yourself").
 *
 * Every attestation record must be signed by that same key (G2): a chain is not trusted on the key
 * each record carries.
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
  const linked = verifyChain(records);
  // G2: every link is signed by the key that anchors trust — the pinned key, else the bundle's.
  // `verifyChain` checks each link against the key it carries, which a forger chooses.
  const anchorKey = opts.expectedPublicKey ?? signature?.publicKey;
  const foreignLinks = records.filter((record) => record.publicKey !== anchorKey).length;
  const chainValid = linked && foreignLinks === 0;
  if (records.length === 0) notes.push("No attestation records (the run had no governed approvals).");
  else if (!linked) notes.push("Attestation chain is broken — records altered, dropped, or reordered.");
  if (foreignLinks > 0) {
    notes.push(
      `${foreignLinks} attestation record(s) signed by another key than the ${opts.expectedPublicKey !== undefined ? "pinned" : "capsule's"} key.`
    );
  }

  // 3. Replay re-derivation on the OSS engine.
  let replayValid = false;
  const repro = capsule.reproduction;
  const reproducible =
    repro != null && repro.graphDefinition != null && repro.entryState !== undefined;
  // G1: a replay starts where the run started — the graph's entry node. An entry state moved onto a
  // later node (a gate) would skip the path to it and still reach the attested gate.
  const entryNodeId = (repro?.graphDefinition as { entryNodeId?: unknown } | null | undefined)
    ?.entryNodeId;
  const startNodeId = (repro?.entryState as { currentNodeId?: unknown } | null | undefined)
    ?.currentNodeId;
  const entryRefused =
    reproducible && (typeof entryNodeId !== "string" || startNodeId !== entryNodeId);
  // The replay runs exactly when the capsule carries its reproduction and starts at the entry node.
  const replayPlanned = reproducible && !entryRefused;
  // An HTTP filesystem backend in the environment would receive the replay's file writes: refuse
  // that replay — the signature, key and chain above are still reported. With no replay planned,
  // the variable is irrelevant: no note, no warning, and the verdict does not depend on it.
  const fsRefused = fsBackendSet() && replayPlanned;
  if (entryRefused) {
    notes.push(
      "Replay refused: the entry state does not start at the graph's entry node — it would skip part of the run."
    );
  } else if (fsRefused) {
    process.stderr.write(
      `ailu-verify: warning: ${FS_BACKEND_ENV} is set — a replay would write the capsule's ` +
        `content to that service. The replay is refused and the capsule is NOT VERIFIED; unset it ` +
        `(env -i) for an offline verification.\n`
    );
    notes.push(`refused: ${FS_BACKEND_ENV} is set — unset it (env -i) for an offline verification`);
  } else if (repro != null && replayPlanned) {
    // A replay may reach the services the environment configures (REPLAY_REACHABLE_ENV) — when the
    // graph uses them: say so on stderr before it runs, and qualify the verdict with one note each.
    for (const name of replayReachableServices()) {
      process.stderr.write(
        `ailu-verify: warning: ${name} is set — if the graph uses that service, the replay sends ` +
          `it the capsule's content; the capsule does not pin it, and it may shape what the replay ` +
          `reproduces. Unset it (env -i) for an offline verification.\n`
      );
      notes.push(`replay may reach: ${name}`);
    }
    try {
      const graph = repro.graphDefinition as GraphDefinition;
      const outcome = await replayCatalogGraph(
        graph,
        repro.entryState as GraphState,
        repro.entryCheckpointId,
        JSON.stringify(repro.journal)
      );
      // ADR 0051 D3: the attested side is the signed records `verifyChain` just checked (not the
      // capsule's unsigned summary), each with the call it signed when it signed one — one
      // decision per request, its last link, so a quorum's signatures read once. The replayed
      // side is what a run files where the replay stopped (`replayedDecisions`, the engine's
      // filing plan): each tool request with the call it asks for, then the human gate the replay
      // stopped at — which a replay's `pendingApprovals` never name.
      const attested = attestedDecisions(records);
      const replayed = replayedDecisions(outcome, graph);
      const verdict = verifyReplayDecisions(attested, replayed);
      replayValid = verdict.ok;
      if (!replayValid) {
        notes.push(`Replay did NOT reproduce the attested decisions (${verdict.mismatches.length} mismatch(es)).`);
      }
    } catch (error) {
      notes.push(`Replay could not run: ${error instanceof Error ? error.message : String(error)} (the native engine may be unavailable for this platform).`);
    }
  } else if (records.length > 0) {
    notes.push(
      "Replay missing: the capsule attests signed decisions but carries no reproduction data (graph + entry state) to re-derive them."
    );
  } else {
    notes.push(
      "No reproduction data (graph + entry state) in the capsule — nothing was re-derived."
    );
  }

  // Signed decisions with nothing to re-derive them from are not reproduced: a capsule stripped of
  // its reproduction would otherwise skip the replay leg.
  const replayMissing = !reproducible && records.length > 0;
  const checksHold =
    signatureValid &&
    (keyPinned ?? true) &&
    chainValid &&
    (reproducible ? replayValid : !replayMissing);
  // Without a pinned key nothing ties the capsule to its issuer: a forger who signs the bundle and
  // the chain with their own key passes every check. Without reproduction data (and no decision
  // to reproduce) nothing was re-derived. Neither is ever VERIFIED.
  // A replay refused because FS_BACKEND_ENV is set is NOT VERIFIED.
  const verdict: Verdict = !checksHold || fsRefused
    ? "not-verified"
    : keyPinned === true && reproducible
      ? "verified"
      : "intact";
  const ok = verdict === "verified";

  return {
    signatureValid,
    keyPinned,
    chainValid,
    replayValid,
    reproducible,
    publicKey: signature?.publicKey ?? "",
    verdict,
    ok,
    notes
  };
}
