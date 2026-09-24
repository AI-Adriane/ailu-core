import { describe, expect, it } from "vitest";

import {
  EditOpDtoSchema,
  FileContentDtoSchema,
  FsBackendRequestDtoSchema,
  FsErrorDtoSchema
} from "./fs.js";

/**
 * The external fs backend wire contract (ADR 0024 phase 2e). These schemas mirror the
 * Rust `ailu-fs-backend` types 1:1 — a round-trip here guards the engine↔service
 * boundary (and Studio's rendering of fs content / diffs).
 */
describe("@ailu-ai/contracts — fs (external backend wire)", () => {
  it("parses a FileContent read result", () => {
    const file = FileContentDtoSchema.parse({
      path: "notes.md",
      content: "hi",
      mediaType: "text/markdown",
      version: 2,
      createdAt: "t"
    });
    expect(file.version).toBe(2);
  });

  it("parses the edit-op discriminated union and rejects an unknown op", () => {
    expect(EditOpDtoSchema.parse({ op: "replace", startLine: 1, endLine: 1, text: "x" }).op).toBe("replace");
    expect(EditOpDtoSchema.parse({ op: "insert", afterLine: 0, text: "x" }).op).toBe("insert");
    expect(EditOpDtoSchema.safeParse({ op: "truncate", startLine: 1, endLine: 1 }).success).toBe(false);
  });

  it("parses a tagged FsError", () => {
    const err = FsErrorDtoSchema.parse({ kind: "notFound", path: "a/b" });
    expect(err.kind).toBe("notFound");
    expect(FsErrorDtoSchema.safeParse({ kind: "bogus" }).success).toBe(false);
  });

  it("accepts a backend request envelope with op-specific args", () => {
    const req = FsBackendRequestDtoSchema.parse({ op: "write", runId: "run-1", path: "scratch/a", content: "x" });
    expect(req.op).toBe("write");
    expect(FsBackendRequestDtoSchema.safeParse({ op: "exec", runId: "r" }).success).toBe(false);
  });
});
