/**
 * @deprecated The TypeScript approval-engine is deprecated as part of the execution
 * engine. Approval gates and attestation have moved to the Rust
 * `crates/approval-engine` crate, exercised by `@ailu/graph-sdk` through the
 * `@ailu/napi` native addon; this package remains only as a fallback when that
 * native addon is absent. New code should route approvals via `@ailu/graph-sdk`,
 * not by importing this engine directly. See
 * `docs/adr/0003-ts-engine-deprecated-sdk-on-rust.md`.
 */
export * from "./types.js";
export * from "./interfaces.js";
export * from "./errors.js";
export * from "./in-memory-approval-engine.js";
export * from "./attestation.js";
