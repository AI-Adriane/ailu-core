import {
  createHash,
  generateKeyPairSync,
  sign as cryptoSign,
  verify as cryptoVerify,
  type KeyObject
} from "node:crypto";

import type { ApprovalRequest } from "./types.js";

/**
 * Tamper-evident attestation of approval decisions.
 *
 * Each resolved approval is hashed (over a canonical, key-sorted view) and signed
 * with Ed25519. Records are chained — every signature covers the previous record's
 * hash — so neither a single field nor the ordering of decisions can be altered
 * after the fact without breaking verification. This is the audit primitive a
 * regulated environment needs: "what did the agents do, and who approved it" becomes
 * cryptographically verifiable.
 */
export type AttestationView = {
  approvalId: string;
  runId: string;
  status: "approved" | "rejected";
  resolvedBy: string;
  subject: string;
  /**
   * ADR 0051 D2 — the call the decision is about, `<name>#<sha256(callInput)>`, when the request's
   * subject carries one (a gated tool call filed by engine 2.7+) and the attestor signs call keys.
   * Absent otherwise, so such a record hashes and verifies exactly as before.
   */
  callKey?: string;
  /**
   * ADR 0051 D2 — what the signature unlocks, next to `callKey`: the subject's `approvalKey` when
   * the grant is the call (it then equals `callKey`), else the tool's name.
   */
  grant?: string;
  decidedAt: string;
};

export type AttestationRecord = AttestationView & {
  algorithm: "ed25519";
  /** SHA-256 (hex) of the canonical {@link AttestationView}. */
  payloadHash: string;
  /** Previous record's `payloadHash`, linking the chain. `null` for the first. */
  prevHash: string | null;
  /** Base64 SPKI/DER public key that verifies `signature`. */
  publicKey: string;
  /** Base64 Ed25519 signature over the chain hash. */
  signature: string;
};

/** Deterministic JSON: object keys sorted recursively so the hash is stable. */
export const canonicalJson = (value: unknown): string => {
  if (value === null || typeof value !== "object") {
    return JSON.stringify(value) ?? "null";
  }
  if (Array.isArray(value)) {
    return `[${value.map((entry) => canonicalJson(entry)).join(",")}]`;
  }
  const record = value as Record<string, unknown>;
  const body = Object.keys(record)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonicalJson(record[key])}`)
    .join(",");
  return `{${body}}`;
};

const sha256Hex = (input: string): string => createHash("sha256").update(input).digest("hex");

/** Hash of the decision payload — changes if any attested field changes. */
export const hashAttestationView = (view: AttestationView): string => sha256Hex(canonicalJson(view));

/** The value actually signed: binds the payload to its position in the chain. */
const chainHash = (payloadHash: string, prevHash: string | null): string =>
  sha256Hex(`${prevHash ?? ""}:${payloadHash}`);

/** A string field of a subject, when it has one. */
const subjectText = (subject: ApprovalRequest["subject"], field: string): string | undefined => {
  const value = (subject as Record<string, unknown>)[field];
  return typeof value === "string" ? value : undefined;
};

const toView = (request: ApprovalRequest, signCallKey: boolean): AttestationView => {
  if (request.status === "pending") {
    throw new Error(`Cannot attest a pending approval '${String(request.id)}'.`);
  }
  const subject =
    "description" in request.subject && typeof request.subject.description === "string"
      ? request.subject.description
      : canonicalJson(request.subject);
  const callKey = signCallKey ? subjectText(request.subject, "callKey") : undefined;
  const grant =
    callKey === undefined
      ? undefined
      : (subjectText(request.subject, "approvalKey") ??
        (subject.startsWith("tool:") ? subject.slice("tool:".length) : undefined));
  return {
    approvalId: String(request.id),
    runId: String(request.runId),
    status: request.status,
    resolvedBy: request.resolvedBy ?? "unknown",
    subject,
    ...(callKey !== undefined ? { callKey } : {}),
    ...(grant !== undefined ? { grant } : {}),
    decidedAt: (request.resolvedAt ?? request.createdAt).toISOString()
  };
};

/** How an {@link Ed25519Attestor} builds the records it signs. */
export type AttestorOptions = {
  /**
   * ADR 0051 D2 — for a request whose subject carries a `callKey`, sign that call key and the
   * grant the signature gives (`grant`: the subject's `approvalKey`, else the tool's name), so the
   * record says « this call, unlocked this way », not only `tool:<name>`. Off by default: a host
   * turns it on once it keeps both fields with the rest of the record — a record stored without
   * them no longer verifies.
   */
  signCallKey?: boolean;
};

/** Signs approval decisions with an Ed25519 key pair. */
export class Ed25519Attestor {
  private readonly privateKey: KeyObject;
  private readonly publicKeyB64: string;
  private readonly signCallKey: boolean;

  public constructor(
    keys?: { privateKey: KeyObject; publicKey: KeyObject },
    options: AttestorOptions = {}
  ) {
    const pair = keys ?? generateKeyPairSync("ed25519");
    this.privateKey = pair.privateKey;
    this.publicKeyB64 = pair.publicKey.export({ type: "spki", format: "der" }).toString("base64");
    this.signCallKey = options.signCallKey === true;
  }

  /** Sign a resolved approval, chaining it after `prevHash`. */
  public attest(request: ApprovalRequest, prevHash: string | null = null): AttestationRecord {
    const view = toView(request, this.signCallKey);
    const payloadHash = hashAttestationView(view);
    const signature = cryptoSign(null, Buffer.from(chainHash(payloadHash, prevHash)), this.privateKey);
    return {
      ...view,
      algorithm: "ed25519",
      payloadHash,
      prevHash,
      publicKey: this.publicKeyB64,
      signature: signature.toString("base64")
    };
  }
}

/** Verify a single record: its payload hash is intact and the signature is valid. */
export const verifyAttestation = (record: AttestationRecord): boolean => {
  const view: AttestationView = {
    approvalId: record.approvalId,
    runId: record.runId,
    status: record.status,
    resolvedBy: record.resolvedBy,
    subject: record.subject,
    ...(record.callKey !== undefined ? { callKey: record.callKey } : {}),
    ...(record.grant !== undefined ? { grant: record.grant } : {}),
    decidedAt: record.decidedAt
  };
  if (hashAttestationView(view) !== record.payloadHash) {
    return false;
  }
  const publicKey = {
    key: Buffer.from(record.publicKey, "base64"),
    type: "spki" as const,
    format: "der" as const
  };
  return cryptoVerify(
    null,
    Buffer.from(chainHash(record.payloadHash, record.prevHash)),
    publicKey,
    Buffer.from(record.signature, "base64")
  );
};

/** Verify a full chain: every record valid and correctly linked to its predecessor. */
export const verifyChain = (records: AttestationRecord[]): boolean => {
  let prev: string | null = null;
  for (const record of records) {
    if (record.prevHash !== prev) {
      return false;
    }
    if (!verifyAttestation(record)) {
      return false;
    }
    prev = record.payloadHash;
  }
  return true;
};
