// Loads the Ailu Rust engine native addon (@ailu/napi).
//
// Resolution order (first hit wins):
//   1. ./ailu_napi.node            — local dev build (scripts/build-napi.sh).
//   2. ./ailu_napi.<triple>.node   — local per-platform build (`napi build --platform`).
//   3. @ailu/napi-<triple>         — the published per-platform optional package.
//
// Covered targets: darwin (arm64/x64), linux glibc (x64/arm64), win32 x64. NOT built:
// linux musl/Alpine, win32 arm64, android, and other arches — on those this module
// THROWS. Callers (e.g. @ailu/graph-sdk) require it inside try/catch and fall back
// to the in-process TypeScript engine, so a missing addon degrades gracefully.
"use strict";

const { existsSync } = require("node:fs");
const { join } = require("node:path");

const { platform, arch } = process;

// Node cannot read libc from platform/arch alone; probe the process report so a musl
// (Alpine) host is not mistaken for a glibc target we don't actually ship.
function isGlibc() {
  try {
    return Boolean(process.report.getReport().header.glibcVersionRuntime);
  } catch {
    return false;
  }
}

// (platform, arch) -> napi triple suffix, or null when no prebuilt binary exists.
function tripleFor() {
  if (platform === "darwin") return arch === "arm64" ? "darwin-arm64" : "darwin-x64";
  if (platform === "win32") return arch === "x64" ? "win32-x64-msvc" : null;
  if (platform === "linux" && isGlibc()) {
    if (arch === "x64") return "linux-x64-gnu";
    if (arch === "arm64") return "linux-arm64-gnu";
  }
  return null;
}

function load() {
  const local = join(__dirname, "ailu_napi.node");
  if (existsSync(local)) return require(local);

  const triple = tripleFor();
  if (triple) {
    const localTriple = join(__dirname, `ailu_napi.${triple}.node`);
    if (existsSync(localTriple)) return require(localTriple);
    try {
      return require(`@ailu/napi-${triple}`);
    } catch {
      // fall through to the descriptive error below
    }
  }

  const libc =
    platform === "linux" ? (isGlibc() ? " (glibc)" : " (musl/non-glibc, unsupported)") : "";
  throw new Error(
    `@ailu/napi: no prebuilt native binary for ${platform}-${arch}${libc}. ` +
      `Prebuilt targets: darwin arm64/x64, linux glibc x64/arm64, win32 x64. ` +
      `The Rust engine is unavailable here; callers fall back to the TypeScript engine. ` +
      `To build locally: bash scripts/build-napi.sh`
  );
}

module.exports = load();
