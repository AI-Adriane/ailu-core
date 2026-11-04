# @ailu-ai/verify

Offline, independent verifier for an **Ailu Certificate of Execution** — the signed audit-export
bundle (`GET /runs/:id/audit-export`). It checks integrity and replay **from the capsule alone**,
without a control plane. Authenticity requires an issuer key obtained independently.

```bash
npx @ailu-ai/verify capsule.json
# The command above checks integrity only: at best INTACT (origin not proven), exit 3.
# Pin the signing key you obtained out-of-band to verify the issuer:
npx @ailu-ai/verify capsule.json --key <base64-spki-public-key>
```

Three independent checks:

1. **signature** — Ed25519 over `sha256(bundle − signature)` against the embedded key.
2. **chain** — the approval attestation chain is hash-linked and every signature is valid.
   If records are absent or empty, the CLI shows `— not applicable (no records)`.
3. **replay** — the run is **re-derived on the OSS engine** from the embedded `{ graph, entryState, journal }`
   and must reach the **same attested decisions**, in order: the tool calls its agents ask for, then
   the human gate where it stops (`replayedDecisions` from `@ailu-ai/graph-sdk`). *Don't trust us —
   re-run it yourself.*

Verdicts and exit codes (`verdict` in the library):

| Verdict | Exit | Meaning |
| --- | --- | --- |
| **VERIFIED** (`verified`) | `0` | the key is pinned, the signature holds, every record is signed by that key, and the replay reproduces the attested decisions |
| **NOT VERIFIED** (`not-verified`) | `1` | a check failed: signature, pinned key, chain, replay — or the capsule attests signed decisions but carries no reproduction (no `reproduction`, or one without `graphDefinition` or `entryState`): `replay missing` |
| usage error | `2` | no capsule, an unreadable capsule, or `--key` without a value |
| **INTACT** (`intact`) | `3` | every check that could run passed, but something is not proven: the origin (no `--key`: the issuer is **UNTRUSTED**) or the run (no decision and no reproduction: nothing re-derived) |

`ok` is `verdict === "verified"`; `issuerTrusted` says the signature holds under the pinned key.

> **Trust anchor.** Without `--key`, the signature is verified against the key *inside* the capsule —
> a forger could re-sign a doctored capsule with their own key. Pass `--key` with a key you trust
> (obtained from the issuer through a trusted channel) to pin it. Copying the embedded key from an
> untrusted capsule does not establish trust.

**Trust model.** Every attestation record must be signed by the anchoring key — the pinned key, else
the capsule's own (G2). A record signed by any other key (a retired issuer key, a third party) fails
the chain: `chainValid` is false and the capsule is NOT VERIFIED. `chainValid` does not
independently authenticate the people named in `resolvedBy`.

Runs without governed approvals may have an empty chain: no decisions were checked, and
`chainValid` remains vacuously true. A capsule without reproduction data **has not been replayed**
(`reproducible` and `replayValid` are false): with signed decisions it is NOT VERIFIED (replay
missing), without any it is at best INTACT. When reproduction data is present, replay must succeed.

Library use:

```ts
import { verifyCapsule } from "@ailu-ai/verify";
const result = await verifyCapsule(capsule, { expectedPublicKey });
// → { signatureValid, keyPinned, issuerTrusted, chainValid, replayValid, reproducible, verdict, ok, notes }
```

The replay leg needs the native engine (`@ailu-ai/napi`, pulled transitively) for your platform.

## What VERIFIED does and does not say

**VERIFIED** means: the capsule is signed by the key you pinned, its attestation chain is intact,
and re-running the run from the capsule reaches the same decisions the signed links name, in order
— for a human gate, « the replay stops at gate X and a signed link names gate X ». It does **not**
mean « gate X was approved according to its quorum ». In particular:

- **The pinned key anchors the capsule and every record.** A record signed by another key than the
  pinned one is refused, even inside a capsule the pinned key signed. `chain` does not authenticate
  the approvers.
- **The replay starts at the graph's entry node.** A capsule whose entry state starts elsewhere is
  NOT VERIFIED: the replay would skip part of the run.
- **The replay calls no LLM provider and needs no key — but it may reach the services your
  environment configures.** With `AILU_PII_REDACTOR_URL`, `AILU_SECRETS_REDACTOR_URL`,
  `AILU_LLMLINGUA_URL` or `AILU_RERANK_ENDPOINT` set, a replay of a graph that uses that service
  sends it the capsule's content; the capsule does not pin the service, and it shapes what the
  replay reproduces. The verifier warns on stderr and adds `replay may reach: <VARIABLE>` to
  `notes` (printed by the CLI): a VERIFIED that carries such a note may depend on that service.
  Verify offline with `env -i`.
- **No replay while `AILU_FS_BACKEND_URL` is set.** An fs-enabled agent's file tools are native —
  the journal does not serve them — so a replay would write the capsule's content to that backend.
  With it set (any value but the empty string, as the engine reads it — whitespace only counts as
  set) and a replay planned (the capsule carries its reproduction and starts at the graph's entry
  node), the verifier does not replay: it warns on stderr, notes
  `refused: AILU_FS_BACKEND_URL is set — unset it (env -i) for an offline verification`, and the
  capsule reads NOT VERIFIED (exit 1), its signature, key and chain still reported. With no replay
  planned the variable is ignored: the verdict is the one the capsule earns without it.

- **The decision's status is not compared.** A replay cannot reproduce a human's choice, so the
  replay check ignores `approved` / `rejected`: a chain with a `rejected` link then an `approved`
  link for the same request verifies. Read the signed links themselves for who decided what.
- **A partial signature counts as a decision.** Links carry no `kind` and no required signature
  count, so the verifier cannot tell a quorum's intermediate signature from a resolution, nor check
  that a quorum was reached. Several links of the same request (same run, request id, subject and
  call) count as one decision; a link that signs another subject or call under the same request id
  is a decision of its own and must be reproduced.
