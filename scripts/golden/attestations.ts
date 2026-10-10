// Golden fixture for ADR 0045 D3.3: approval attestations signed by the TypeScript implementation
// (`packages/approval-engine/src/attestation.ts`), with a fixed Ed25519 seed, over inputs that stress
// what the two canonical JSONs could disagree on — numbers, key order outside the BMP, escapes. The
// Rust implementation must verify these records and produce them byte for byte.
//
//   pnpm exec tsx scripts/golden/attestations.ts > crates/approval-engine/tests/fixtures/ts-attestations.json

import { createPrivateKey, createPublicKey } from "node:crypto";

import { Ed25519Attestor } from "../../packages/approval-engine/src/attestation.ts";
import type { ApprovalRequest } from "../../packages/approval-engine/src/types.ts";

const SEED = Buffer.alloc(32, 7);
// PKCS#8 wrapping of a raw Ed25519 seed (RFC 8410).
const PKCS8_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");
const privateKey = createPrivateKey({
  key: Buffer.concat([PKCS8_PREFIX, SEED]),
  format: "der",
  type: "pkcs8"
});
const attestor = new Ed25519Attestor(
  { privateKey, publicKey: createPublicKey(privateKey) },
  { signCallKey: true }
);

/** A request as the fixture keeps it: ISO dates, and any JSON as subject. */
type Input = Omit<ApprovalRequest, "resolvedAt" | "createdAt" | "subject"> & {
  subject: unknown;
  resolvedAt: string;
  createdAt: string;
};

const inputs: Input[] = [
  {
    id: "approval-1",
    runId: "run-1",
    nodeId: "assistant",
    requestedBy: "assistant",
    subject: { description: "tool:refund — order 42" },
    status: "approved",
    resolvedBy: "alice",
    resolvedAt: "2026-10-02T10:00:00.000Z",
    createdAt: "2026-10-02T09:59:00.000Z"
  },
  {
    id: "approval-2",
    runId: "run-1",
    nodeId: "payout",
    requestedBy: "payout",
    // No description: the subject itself is attested, as canonical JSON — numbers as JS writes them.
    subject: {
      amount: 1e20,
      huge: 1e21,
      ratio: 0.1,
      tiny: 1e-7,
      small: 0.000001,
      negativeZero: -0,
      precise: 9007199254740993,
      third: 1 / 3,
      nested: { b: 1, a: [1.5, 2, -3.25e-10] }
    },
    status: "rejected",
    resolvedBy: "bob",
    resolvedAt: "2026-10-02T10:05:00.000Z",
    createdAt: "2026-10-02T10:01:00.000Z"
  },
  {
    id: "approval-3",
    runId: "run-1",
    nodeId: "notify",
    requestedBy: "notify",
    // Key order is UTF-16 code units in JS: an astral key sorts BEFORE a key at U+E000 there,
    // after it in UTF-8 byte order.
    subject: { "": 1, "😀": 2, z: 3, é: 4, A: 5 },
    status: "approved",
    resolvedBy: "carol",
    resolvedAt: "2026-10-02T10:10:00.000Z",
    createdAt: "2026-10-02T10:09:00.000Z"
  },
  {
    id: "approval-4",
    runId: "run-1",
    nodeId: "post",
    requestedBy: "post",
    subject: {
      description: 'line\nbreak "quoted" \\ tab\t ctrl\u0001 é 😀'
    },
    status: "approved",
    resolvedBy: "dave",
    resolvedAt: "2026-10-02T10:15:00.000Z",
    createdAt: "2026-10-02T10:14:00.000Z"
  },
  {
    id: "approval-5",
    runId: "run-1",
    nodeId: "assistant",
    requestedBy: "assistant",
    // ADR 0051 D2: a gated call's subject carries its call key, which the record signs.
    subject: {
      description: "tool:refund",
      input: { order: "A-7", amount: 40 },
      callKey: `refund#${"3".repeat(64)}`
    },
    status: "approved",
    resolvedBy: "erin",
    resolvedAt: "2026-10-02T10:20:00.000Z",
    createdAt: "2026-10-02T10:19:00.000Z"
  }
];

let prevHash: string | null = null;
const records = inputs.map((input) => {
  const request = {
    ...input,
    resolvedAt: new Date(input.resolvedAt),
    createdAt: new Date(input.createdAt)
  } as ApprovalRequest;
  const record = attestor.attest(request, prevHash);
  prevHash = record.payloadHash;
  return record;
});

process.stdout.write(
  `${JSON.stringify({ seedHex: SEED.toString("hex"), requests: inputs, records }, null, 2)}\n`
);
