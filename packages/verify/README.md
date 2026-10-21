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

Exit code `0` when everything that applies passes, `1` otherwise.

> **Trust anchor.** Without `--key`, the signature is verified against the key *inside* the capsule —
> a forger could re-sign a doctored capsule with their own key. Pass `--key` with a key you trust
> (published by the issuer) to pin it.

Library use:

```ts
import { verifyCapsule } from "@ailu-ai/verify";
const result = await verifyCapsule(capsule, { expectedPublicKey });
// → { signatureValid, keyPinned, chainValid, replayValid, reproducible, ok, notes }
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

- **The decision's status is not compared.** A replay cannot reproduce a human's choice, so the
  replay check ignores `approved` / `rejected`: a chain with a `rejected` link then an `approved`
  link for the same request verifies. Read the signed links themselves for who decided what.
- **A partial signature counts as a decision.** Links carry no `kind` and no required signature
  count, so the verifier cannot tell a quorum's intermediate signature from a resolution, nor check
  that a quorum was reached. Several links of the same request (same run, request id, subject and
  call) count as one decision; a link that signs another subject or call under the same request id
  is a decision of its own and must be reproduced.
