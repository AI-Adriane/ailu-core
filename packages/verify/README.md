# @ailu-ai/verify

Offline, independent verifier for an **Ailu Certificate of Execution** — the signed audit-export
bundle (`GET /runs/:id/audit-export`). It re-checks everything **from the capsule alone**: no control
plane, no trust in the issuer.

```bash
npx @ailu-ai/verify capsule.json
# pin the signing key you obtained out-of-band (recommended):
npx @ailu-ai/verify capsule.json --key <base64-spki-public-key>
```

Three independent checks:

1. **signature** — Ed25519 over `sha256(bundle − signature)` against the embedded key.
2. **chain** — the approval attestation chain is hash-linked and every signature is valid.
3. **replay** — the run is **re-derived on the OSS engine** from the embedded `{ graph, entryState, journal }`
   and must reach the **same attested decisions**, in order: the tool calls its agents ask for, then
   the human gate where it stops (`replayedDecisions` from `@ailu-ai/graph-sdk`). *Don't trust us —
   re-run it yourself.*

Three verdicts, three exit codes (since 2.7.3):

| Verdict | Exit | Means |
|---|---|---|
| `✓ VERIFIED` | `0` | Every check passed **and** the capsule is signed by the key you pinned with `--key`. |
| `◐ INTACT` | `3` | Every check that could run passed, but something is not proven: without `--key`, the origin; without reproduction data in a capsule that attests no decision, the run (nothing re-derived). |
| `✗ NOT VERIFIED` | `1` | A check failed — including **replay missing**: signed decisions and no reproduction data to re-derive them. |

Exit code `2` is a usage error or an unreadable capsule (`--key` without a value included).

> **Trust anchor.** Without `--key`, the signature is verified against the key *inside* the capsule,
> and anyone can sign a capsule — and its attestation chain — with their own key. Such a capsule is
> therefore never VERIFIED: it reads **INTACT — origin not proven** (exit `3`). Pass `--key` with
> the key the issuer publishes (its JWKS), obtained out of band, to get VERIFIED. Before 2.7.3 an
> unpinned capsule whose checks passed read VERIFIED (exit `0`).

Library use:

```ts
import { verifyCapsule } from "@ailu-ai/verify";
const result = await verifyCapsule(capsule, { expectedPublicKey });
// → { signatureValid, keyPinned, chainValid, replayValid, reproducible, verdict, ok, notes }
// verdict: "verified" | "intact" | "not-verified"; ok === (verdict === "verified")
```

The replay leg needs the native engine (`@ailu-ai/napi`, pulled transitively) for your platform.

## What VERIFIED does and does not say

**VERIFIED** means: the capsule is signed by the key you pinned, its attestation chain is intact,
and re-running the run from the capsule reaches the same decisions the signed links name, in order
— for a human gate, « the replay stops at gate X and a signed link names gate X ». It does **not**
mean « gate X was approved according to its quorum ». In particular:

- **One key signs everything.** Every attestation record must be signed by the pinned key, or
  without `--key` by the key that signs the capsule; a record signed by any other key fails `chain`.
- **The replay starts at the graph's entry node.** A capsule whose entry state starts elsewhere is
  NOT VERIFIED: the replay would skip part of the run.
- **The replay calls no LLM provider and needs no key — but it may reach the services your
  environment configures.** With `AILU_PII_REDACTOR_URL`, `AILU_SECRETS_REDACTOR_URL`,
  `AILU_LLMLINGUA_URL`, `AILU_RERANK_ENDPOINT` or `AILU_FS_BACKEND_URL` set, a replay of a graph
  that uses that service sends it the capsule's content (an fs-enabled agent's file writes
  included); the capsule does not pin the service, and it shapes what the replay reproduces. The
  verifier warns on stderr and adds `replay may reach: <VARIABLE>` to `notes` (printed by the CLI):
  a VERIFIED that carries such a note may depend on that service. Verify offline with `env -i`.

- **The decision's status is not compared.** A replay cannot reproduce a human's choice, so the
  replay check ignores `approved` / `rejected`: a chain with a `rejected` link then an `approved`
  link for the same request verifies. Read the signed links themselves for who decided what.
- **A partial signature counts as a decision.** Links carry no `kind` and no required signature
  count, so the verifier cannot tell a quorum's intermediate signature from a resolution, nor check
  that a quorum was reached. Several links of the same request (same run, request id, subject and
  call) count as one decision; a link that signs another subject or call under the same request id
  is a decision of its own and must be reproduced.
