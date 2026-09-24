# @ailu/verify

Offline, independent verifier for an **Ailu Certificate of Execution** — the signed audit-export
bundle (`GET /runs/:id/audit-export`). It re-checks everything **from the capsule alone**: no control
plane, no trust in the issuer.

```bash
npx @ailu/verify capsule.json
# pin the signing key you obtained out-of-band (recommended):
npx @ailu/verify capsule.json --key <base64-spki-public-key>
```

Three independent checks:

1. **signature** — Ed25519 over `sha256(bundle − signature)` against the embedded key.
2. **chain** — the approval attestation chain is hash-linked and every signature is valid.
3. **replay** — the run is **re-derived on the OSS engine** from the embedded `{ graph, entryState, journal }`
   and must reach the **same attested decisions**. *Don't trust us — re-run it yourself.*

Exit code `0` when everything that applies passes, `1` otherwise.

> **Trust anchor.** Without `--key`, the signature is verified against the key *inside* the capsule —
> a forger could re-sign a doctored capsule with their own key. Pass `--key` with a key you trust
> (published by the issuer) to pin it.

Library use:

```ts
import { verifyCapsule } from "@ailu/verify";
const result = await verifyCapsule(capsule, { expectedPublicKey });
// → { signatureValid, keyPinned, chainValid, replayValid, reproducible, ok, notes }
```

The replay leg needs the native engine (`@ailu/napi`, pulled transitively) for your platform.
