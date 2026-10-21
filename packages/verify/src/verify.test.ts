import { createHash, generateKeyPairSync, sign } from "node:crypto";

import {
  Ed25519Attestor,
  InMemoryApprovalEngine,
  exampleGraphs,
  runCatalogGraph,
  rustEngineAvailable,
  type ApprovalRequest,
  type AttestationRecord,
  type GraphDefinition,
  type RunId
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
 * The issuer's key: the control plane signs its attestation chain and the bundle with the same key
 * (the one its JWKS publishes), which the verifier requires (G2).
 */
const ISSUER = generateKeyPairSync("ed25519");

/** Ed25519-sign a capsule body the way the control plane does: sha256(bundle-minus-signature). */
/** The issuer's key as `--key` takes it: what a verifier obtains out of band (the JWKS). */
const PINNED = {
  expectedPublicKey: ISSUER.publicKey.export({ type: "spki", format: "der" }).toString("base64")
};

/** A forger's key: they sign the bundle AND the chain with it. */
const FORGER = generateKeyPairSync("ed25519");

const signBody = (body: Record<string, unknown>, keys = ISSUER): Capsule => {
  const { privateKey, publicKey } = keys;
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

/**
 * Every variable the engine reads to reach a model: the provider keys, the local-server flags,
 * offline mode (`AILU_LLM_MOCK`) and record mode (`AILU_LLM_RECORD`).
 */
const MODEL_ENV = [
  "ANTHROPIC_API_KEY",
  "OPENAI_API_KEY",
  "GEMINI_API_KEY",
  "GOOGLE_API_KEY",
  "MISTRAL_API_KEY",
  "OPENROUTER_API_KEY",
  "MINIMAX_API_KEY",
  "HF_TOKEN",
  "HUGGINGFACE_API_KEY",
  "AILU_USE_OLLAMA",
  "AILU_USE_LMSTUDIO",
  "AILU_LLM_MOCK",
  "AILU_LLM_RECORD"
] as const;

/**
 * Run `fn` with `values` set and every model variable unset — the list above plus ANY `AILU_*` or
 * `*_API_KEY` the host (a developer's shell, the CI) happens to carry — then restore them all.
 */
const withModelEnv = async <T>(
  values: Partial<Record<(typeof MODEL_ENV)[number], string>>,
  fn: () => Promise<T>
): Promise<T> => {
  const names = new Set<string>([
    ...MODEL_ENV,
    ...Object.keys(process.env).filter((name) => /^AILU_|_API_KEY$/.test(name))
  ]);
  const saved = [...names].map((name) => [name, process.env[name]] as const);
  for (const name of names) delete process.env[name];
  Object.assign(process.env, values);
  try {
    return await fn();
  } finally {
    for (const [name, value] of saved) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
  }
};

/**
 * Record a run the way a fixture needs it: journaled, and offline (the deterministic mock stands in
 * for the provider). The offline flag lives only while recording — never while verifying.
 */
const record = <T>(fn: () => Promise<T>): Promise<T> =>
  withModelEnv({ AILU_LLM_RECORD: "1", AILU_LLM_MOCK: "1" }, fn);

/**
 * Build a genuine capsule the way the control plane does: run the demo graph, approve and attest
 * what it filed, then Ed25519-sign the bundle.
 */
const buildCapsule = async (attesting: Attesting = { signCallKey: true }): Promise<Capsule> => {
  const def = exampleGraphs().find((g) => g.slug === "approval-demo")?.definition;
  if (def === undefined) throw new Error("approval-demo example graph not found");
  if (!def.nodes.some((node) => node.type === "agent")) {
    throw new Error("approval-demo has no agent node");
  }
  const engine = new InMemoryApprovalEngine();
  const outcome = await record(() =>
    runCatalogGraph(def, {
      initialData: { input: "Refund ORD-8830 (duplicate charge)." },
      onEvent: () => {},
      providerKeys: {},
      approvalEngine: engine
    })
  );
  const attestor = new Ed25519Attestor(ISSUER, { signCallKey: attesting.signCallKey });
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
  return signBody(body);
};

/** A component assembles a draft, then a human gate approves it: the control plane's content flow. */
const GATED_DRAFT = {
  id: "gated-draft",
  version: "0.0.0",
  name: "gated-draft",
  channels: {
    docType: { type: "string", reducer: "replace", default: "" },
    body: { type: "string", reducer: "replace", default: "" }
  },
  nodes: [
    {
      id: "assemble",
      type: "action",
      label: "assemble",
      metadata: {
        component: { kind: "promptBuilder", params: { into: "body", template: "{{body}}" } }
      }
    },
    { id: "approve", type: "human-gate", label: "Approve the draft" }
  ],
  edges: [{ id: "e_assemble_approve", from: "assemble", to: "approve", type: "default" }],
  entryNodeId: "assemble"
} as unknown as GraphDefinition;

/** Two human gates in a row: `assemble → approve → approve2`. */
const GATED_TWICE = {
  ...GATED_DRAFT,
  id: "gated-twice",
  name: "gated-twice",
  nodes: [...GATED_DRAFT.nodes, { id: "approve2", type: "human-gate", label: "Approve again" }],
  edges: [
    ...GATED_DRAFT.edges,
    { id: "e_approve_approve2", from: "approve", to: "approve2", type: "default" }
  ]
} as unknown as GraphDefinition;

/**
 * A capsule of a run that stops at a human gate (`assemble → approve`): a member approves or
 * rejects the gate's request and the decision is attested. `wrong-gate` honestly signs a decision
 * on a gate the run never reaches — the negative control. `p1` and `p2` put, before the gate's
 * genuine link, a link of the SAME request id that signs another decision (a tool call, another
 * gate): forged evidence a quorum must not hide. `p4` and `p9` move the entry state onto a gate
 * so the replay skips the path to it (P4: the gate the run reached, with an empty journal; P9: the
 * second of two gates, attesting only that one) — what the entry-state guard (G1) refuses.
 */
const buildGateCapsule = async (
  decision:
    | "approved"
    | "rejected"
    | "wrong-gate"
    | "quorum"
    | "p1"
    | "p2"
    | "p4"
    | "p9"
    | "foreign-chain"
    | "forged"
): Promise<Capsule> => {
  const runId = `verify-gate-${decision}` as RunId;
  const engine = new InMemoryApprovalEngine();
  const graph = decision === "p9" ? GATED_TWICE : GATED_DRAFT;
  const outcome = await record(() =>
    runCatalogGraph(graph, {
      runId,
      initialData: { body: "The draft.", docType: "memo" },
      approvalEngine: engine
    })
  );
  const [filed] = await engine.getPending(runId);
  if (filed === undefined) throw new Error("the gate filed no request");
  const resolved =
    decision === "rejected"
      ? await engine.reject(filed.id, "member", "not this one")
      : await engine.approve(filed.id, "member");
  const signed =
    decision === "wrong-gate"
      ? { ...resolved, subject: { description: "gate:some-other-gate" } }
      : decision === "p9"
        ? { ...resolved, subject: { description: "gate:approve2" } }
        : resolved;
  const entryState =
    decision === "p4"
      ? { ...outcome.entryState, status: "suspended", currentNodeId: "approve" }
      : decision === "p9"
        ? { ...outcome.entryState, currentNodeId: "approve2" }
        : outcome.entryState;
  // `foreign-chain`: the bundle is the issuer's, the chain is signed by another key (G2).
  // `forged`: a forger signs the chain and the bundle with their own key — every check holds.
  const keys = decision === "forged" ? FORGER : ISSUER;
  const attestor = new Ed25519Attestor(decision === "foreign-chain" ? undefined : keys, {
    signCallKey: true
  });
  // A quorum (two signatures): the signature given before the last, then the resolution — two
  // links of the same request. P1/P2: a link of the same id that signs another decision.
  const firstSigned =
    decision === "quorum"
      ? { ...signed, resolvedBy: "first-member" }
      : decision === "p1"
        ? {
            ...signed,
            subject: {
              description: "tool:wire_transfer",
              callKey: `wire_transfer#${"b".repeat(64)}`
            }
          }
        : decision === "p2"
          ? { ...signed, subject: { description: "gate:legal" } }
          : undefined;
  const first = firstSigned === undefined ? [] : [attestor.attest(firstSigned, null)];
  const records = [...first, attestor.attest(signed, first.at(-1)?.payloadHash ?? null)];
  return signBody(
    {
      schemaVersion: "1",
      run: { status: decision === "rejected" ? "rejected" : "completed" },
      reproduction: {
        entryCheckpointId: `${runId}:entry`,
        entryState,
        journal: decision === "p4" ? {} : JSON.parse(outcome.replayJournal ?? "{}"),
        graphDefinition: graph
      },
      attestation: { records },
      replay: { decisions: { attested: records.map((record) => ({ subject: record.subject })) } }
    },
    keys
  );
};

/**
 * Every case builds its capsule by running a graph, and checks the replay, on the native engine.
 * Without `@ailu-ai/napi` they are SKIPPED — explicitly, and counted by the reporter — never passed
 * in silence. With `AILU_REQUIRE_NATIVE=1` (CI), a missing engine fails the suite instead.
 */
const NATIVE = rustEngineAvailable();

it.runIf(process.env.AILU_REQUIRE_NATIVE === "1")(
  "runs on the native engine (AILU_REQUIRE_NATIVE=1)",
  () => {
    expect(NATIVE).toBe(true);
  }
);

describe.skipIf(!NATIVE)("verifyCapsule", () => {
  it("verifies a genuine capsule — signature holds, and replay reproduces the attested decisions", async () => {
    const capsule = await buildCapsule();
    const result = await verifyCapsule(capsule, PINNED);
    expect(result.signatureValid).toBe(true);
    expect(result.chainValid).toBe(true); // empty chain is vacuously valid
    // The load-bearing claim: re-derivation reproduces the attested decision (needs the native engine).
    expect(result.reproducible).toBe(true);
    expect(result.replayValid).toBe(true);
    expect(result.ok).toBe(true);
  });

  it("verifies a run with an agent node with no provider key and no AILU_LLM_MOCK (replay needs neither)", async () => {
    const capsule = await buildCapsule();
    // The verifier's environment: no key, no offline mode, no record mode — `npx` on a bare machine.
    const result = await withModelEnv({}, () => {
      // Assert the environment really is bare, whatever the CI or the shell set.
      const leaked = Object.keys(process.env).filter((name) =>
        /^AILU_|_API_KEY$|^HF_TOKEN$/.test(name)
      );
      expect(leaked).toEqual([]);
      return verifyCapsule(capsule, PINNED);
    });
    expect(result.notes).toEqual([]);
    expect(result.reproducible).toBe(true);
    expect(result.replayValid).toBe(true);
    expect(result.verdict).toBe("verified");
    expect(result.ok).toBe(true);
  });

  it("fails the same agent-node capsule with one byte altered, with no key and no AILU_LLM_MOCK", async () => {
    const capsule = await buildCapsule();
    const text = JSON.stringify(capsule);
    const altered = text.replace("ORD-8830", "ORD-8831");
    expect(altered.length).toBe(text.length);
    expect(altered).not.toBe(text);
    const result = await withModelEnv({}, () =>
      verifyCapsule(JSON.parse(altered) as Capsule, PINNED)
    );
    expect(result.signatureValid).toBe(false);
    expect(result.verdict).toBe("not-verified");
    expect(result.ok).toBe(false);
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
    expect(result.replayValid).toBe(false);
    expect(result.ok).toBe(false);
  });

  it("verifies evidence attested before call keys, by its subjects", async () => {
    const capsule = await buildCapsule({ signCallKey: false });
    expect(capsule.attestation?.records?.every((record) => record.callKey === undefined)).toBe(
      true
    );
    const result = await verifyCapsule(capsule, PINNED);
    expect(result.replayValid).toBe(true);
    expect(result.ok).toBe(true);
  });

  it("reads the attested decisions from the verified records, not the unsigned summary", async () => {
    // The capsule's unsigned `replay.decisions.attested` says nothing was decided; the signed
    // records say the refund was. The verdict follows the records (ADR 0051 D3, R8).
    const capsule = await buildCapsule({ signCallKey: true, emptySummary: true });
    const records = capsule.attestation?.records ?? [];
    expect(records.length).toBeGreaterThan(0);
    const result = await verifyCapsule(capsule);
    expect(result.replayValid).toBe(true);
  });

  it("rejects when the signing key does not match the pinned key", async () => {
    const capsule = await buildCapsule();
    const result = await verifyCapsule(capsule, { expectedPublicKey: "a-different-known-key" });
    expect(result.keyPinned).toBe(false);
    expect(result.ok).toBe(false);
  });
});

describe.skipIf(!NATIVE)("verifyCapsule on a run that stops at a human gate", () => {
  for (const decision of ["approved", "rejected"] as const) {
    it(`verifies a genuine capsule whose gate was ${decision}: the replay re-derives the gate`, async () => {
      const result = await verifyCapsule(await buildGateCapsule(decision), PINNED);
      expect(result.signatureValid).toBe(true);
      expect(result.chainValid).toBe(true);
      expect(result.reproducible).toBe(true);
      expect(result.replayValid).toBe(true);
      expect(result.ok).toBe(true);
    });
  }

  it("verifies a gate signed by a quorum: its signatures are one decision", async () => {
    const capsule = await buildGateCapsule("quorum");
    expect(capsule.attestation?.records?.length).toBe(2);
    const result = await verifyCapsule(capsule, PINNED);
    expect(result.chainValid).toBe(true);
    expect(result.replayValid).toBe(true);
    expect(result.ok).toBe(true);
  });

  for (const [forged, what] of [
    ["p1", "a signed tool call"],
    ["p2", "another gate"]
  ] as const) {
    it(`fails a chain where a link of the gate's id signs ${what} (${forged.toUpperCase()})`, async () => {
      const capsule = await buildGateCapsule(forged);
      const records = capsule.attestation?.records ?? [];
      expect(records.map((record) => record.approvalId)).toEqual([
        records[1]?.approvalId,
        records[1]?.approvalId
      ]);
      const result = await verifyCapsule(capsule, {
        expectedPublicKey: capsule.signature.publicKey
      });
      expect(result.signatureValid).toBe(true);
      expect(result.keyPinned).toBe(true);
      expect(result.chainValid).toBe(true);
      expect(result.replayValid).toBe(false);
      expect(result.ok).toBe(false);
    });
  }

  for (const [forged, what] of [
    ["p4", "on the gate it reached, with an empty journal"],
    ["p9", "on a second gate, skipping the first"]
  ] as const) {
    it(`fails an entry state moved ${what} (${forged.toUpperCase()}, G1)`, async () => {
      const capsule = await buildGateCapsule(forged);
      const result = await verifyCapsule(capsule, {
        expectedPublicKey: capsule.signature.publicKey
      });
      expect(result.signatureValid).toBe(true);
      expect(result.chainValid).toBe(true);
      expect(result.replayValid).toBe(false);
      expect(result.ok).toBe(false);
      expect(result.notes.join(" ")).toMatch(/entry node/);
    });
  }

  it("fails a chain that signs a gate the run never reaches", async () => {
    const result = await verifyCapsule(await buildGateCapsule("wrong-gate"));
    expect(result.signatureValid).toBe(true);
    expect(result.chainValid).toBe(true);
    expect(result.replayValid).toBe(false);
    expect(result.ok).toBe(false);
  });

  it("fails a capsule with one byte altered", async () => {
    const capsule = await buildGateCapsule("approved");
    const text = JSON.stringify(capsule);
    const altered = JSON.parse(
      text.replace('"status":"completed"', '"status":"completeD"')
    ) as Capsule;
    expect(JSON.stringify(altered)).not.toBe(text);
    const result = await verifyCapsule(altered);
    expect(result.signatureValid).toBe(false);
    expect(result.ok).toBe(false);
  });

  it("never reads VERIFIED without a pinned key: a genuine capsule is INTACT, origin not proven", async () => {
    const result = await verifyCapsule(await buildGateCapsule("approved"));
    expect(result.keyPinned).toBeNull();
    expect(result.signatureValid && result.chainValid && result.replayValid).toBe(true);
    expect(result.verdict).toBe("intact");
    expect(result.ok).toBe(false);
  });

  it("never reads VERIFIED for a forger who signs the bundle and the chain with their own key", async () => {
    const capsule = await buildGateCapsule("forged");
    const unpinned = await verifyCapsule(capsule);
    expect(unpinned.signatureValid && unpinned.chainValid && unpinned.replayValid).toBe(true);
    expect(unpinned.verdict).toBe("intact");
    expect(unpinned.ok).toBe(false);
    const pinned = await verifyCapsule(capsule, PINNED);
    expect(pinned.keyPinned).toBe(false);
    expect(pinned.verdict).toBe("not-verified");
    expect(pinned.ok).toBe(false);
  });

  it("fails a capsule that attests decisions but carries no reproduction (replay missing)", async () => {
    const capsule = await buildGateCapsule("approved");
    for (const reproduction of [null, undefined]) {
      const body: Record<string, unknown> = { ...capsule, reproduction };
      delete body.signature;
      const stripped = signBody(body);
      const result = await verifyCapsule(stripped, PINNED);
      expect(result.signatureValid && result.chainValid).toBe(true);
      expect(result.reproducible).toBe(false);
      expect(result.verdict).toBe("not-verified");
      expect(result.ok).toBe(false);
      expect(result.notes.join(" ")).toMatch(/Replay missing/);
    }
  });

  it("reads INTACT, not VERIFIED, a capsule with no decision and no reproduction", async () => {
    const capsule = signBody({
      schemaVersion: "1",
      attestation: { records: [] },
      reproduction: null
    });
    const result = await verifyCapsule(capsule, PINNED);
    expect(result.keyPinned).toBe(true);
    expect(result.verdict).toBe("intact");
    expect(result.ok).toBe(false);
  });

  it("fails a chain signed by another key than the bundle's (G2)", async () => {
    const capsule = await buildGateCapsule("foreign-chain");
    const unpinned = await verifyCapsule(capsule);
    expect(unpinned.signatureValid).toBe(true);
    expect(unpinned.chainValid).toBe(false);
    expect(unpinned.ok).toBe(false);
    expect(unpinned.notes.join(" ")).toMatch(/another key than the capsule's key/);
    const pinned = await verifyCapsule(capsule, { expectedPublicKey: capsule.signature.publicKey });
    expect(pinned.keyPinned).toBe(true);
    expect(pinned.chainValid).toBe(false);
    expect(pinned.ok).toBe(false);
  });

  it("fails a chain not signed by the pinned key, even when the bundle is (G2)", async () => {
    const capsule = await buildGateCapsule("approved");
    const result = await verifyCapsule(capsule, { expectedPublicKey: "a-different-known-key" });
    expect(result.keyPinned).toBe(false);
    expect(result.chainValid).toBe(false);
    expect(result.ok).toBe(false);
  });

  it("fails a capsule whose signed link was altered", async () => {
    const capsule = await buildGateCapsule("approved");
    const [record] = capsule.attestation?.records ?? [];
    if (record === undefined) throw new Error("no record");
    record.subject = "gate:some-other-gate";
    const result = await verifyCapsule(capsule);
    expect(result.signatureValid).toBe(false);
    expect(result.chainValid).toBe(false);
    expect(result.ok).toBe(false);
  });
});
