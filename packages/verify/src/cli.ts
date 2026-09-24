#!/usr/bin/env node
import { readFileSync } from "node:fs";

import { verifyCapsule, type Capsule } from "./verify.js";

const args = process.argv.slice(2);
const file = args.find((a) => !a.startsWith("-"));
const keyIdx = args.indexOf("--key");
const expectedPublicKey = keyIdx >= 0 ? args[keyIdx + 1] : undefined;

if (file === undefined) {
  console.error("Usage: ailu-verify <capsule.json> [--key <base64-spki-public-key>]");
  process.exit(2);
}

let capsule: Capsule;
try {
  capsule = JSON.parse(readFileSync(file, "utf8")) as Capsule;
} catch (error) {
  console.error(`Cannot read capsule '${file}': ${error instanceof Error ? error.message : String(error)}`);
  process.exit(2);
}

const mark = (ok: boolean): string => (ok ? "✓ PASS" : "✗ FAIL");
const result = await verifyCapsule(capsule, { expectedPublicKey });

console.log("Ailu — Certificate of Execution\n");
console.log(`  signature    ${mark(result.signatureValid)}`);
console.log(`  key pinned   ${result.keyPinned === null ? "— not pinned" : mark(result.keyPinned)}`);
console.log(`  chain        ${result.chainValid ? "✓ PASS" : "✗ FAIL"}`);
console.log(`  replay       ${result.reproducible ? mark(result.replayValid) : "— no reproduction data"}`);
console.log(`\n  signing key  ${result.publicKey}`);
if (result.notes.length > 0) {
  console.log("");
  for (const note of result.notes) console.log(`  • ${note}`);
}
console.log(`\n${result.ok ? "✓ VERIFIED — this run is what it claims to be." : "✗ NOT VERIFIED."}`);
process.exit(result.ok ? 0 : 1);
