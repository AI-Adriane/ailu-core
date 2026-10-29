# @ailu-ai/verify

Offline, independent verifier for an **Ailu Certificate of Execution** — the signed audit-export
bundle (`GET /runs/:id/audit-export`). It checks integrity and replay **from the capsule alone**,
without a control plane. Authenticity requires an issuer key obtained independently.

```bash
npx @ailu-ai/verify capsule.json
# The command above shows untrusted diagnostics and exits 1.
# Pin the signing key you obtained out-of-band to verify the issuer:
npx @ailu-ai/verify capsule.json --key <base64-spki-public-key>
```

Three independent checks:

1. **signature** — Ed25519 over `sha256(bundle − signature)` against the embedded key.
2. **chain** — the approval attestation chain is hash-linked and every signature is valid.
3. **replay** — the run is **re-derived on the OSS engine** from the embedded `{ graph, entryState, journal }`
   and must reach the **same attested decisions**. *Don't trust us — re-run it yourself.*

Exit code `0` only when the issuer key is pinned, the capsule signature is valid, and every
applicable chain/replay check passes; `1` otherwise. With no key, integrity and replay diagnostics
remain available, but the issuer is **UNTRUSTED**, the verdict is **NOT VERIFIED**, and exit is `1`.

> **Trust anchor.** Without `--key`, the signature is verified against the key *inside* the capsule —
> a forger could re-sign a doctored capsule with their own key. Pass `--key` with a key you trust
> (obtained from the issuer through a trusted channel) to pin it. Copying the embedded key from an
> untrusted capsule does not establish trust.

The trusted capsule signature covers all enclosed attestation records and their signer keys.
Attestation signers may differ from the capsule issuer; `chainValid` checks their signatures and
hash linkage, while `issuerTrusted` says the pinned issuer endorsed that evidence. It does not
independently authenticate the people named in `resolvedBy`.

Runs without governed approvals may have an empty chain. Capsules without reproduction data can
still authenticate their issuer, but **have not been replayed** (`reproducible` and `replayValid`
are false). When reproduction data is present, replay must succeed for the overall verdict to pass.

Library use:

```ts
import { verifyCapsule } from "@ailu-ai/verify";
const result = await verifyCapsule(capsule, { expectedPublicKey });
// → { signatureValid, keyPinned, issuerTrusted, chainValid, replayValid, reproducible, ok, notes }
```

The replay leg needs the native engine (`@ailu-ai/napi`, pulled transitively) for your platform.
