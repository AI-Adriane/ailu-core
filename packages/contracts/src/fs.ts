import { z } from "zod";

/**
 * The wire contract for the external durable filesystem backend (ADR 0024 phase 2e).
 * When `AILU_FS_BACKEND_URL` is set, the Rust engine POSTs `{ op, runId, ... }` to the
 * service and expects either the op's result or `{ error: FsError }`. These schemas type
 * that boundary (for a service implementer and for Studio rendering of fs content/diffs);
 * they mirror the Rust `ailu-fs-backend` types 1:1 (camelCase).
 */

export const ARTIFACT_MEDIA_TYPES = [
  "application/json",
  "text/plain",
  "text/markdown",
  "application/octet-stream"
] as const;
export const ArtifactMediaTypeSchema = z.enum(ARTIFACT_MEDIA_TYPES);

/** A file's content (read result). `content` is UTF-8 text, JSON text, or base64 by mediaType. */
export const FileContentDtoSchema = z.object({
  path: z.string(),
  content: z.string(),
  mediaType: ArtifactMediaTypeSchema,
  version: z.number().int(),
  createdAt: z.string()
});
export type FileContentDto = z.infer<typeof FileContentDtoSchema>;

/** One `ls` entry; `isDir: true` is a synthetic directory over the flat keyspace. */
export const FileEntryDtoSchema = z.object({
  path: z.string(),
  isDir: z.boolean(),
  version: z.number().int().optional()
});
export type FileEntryDto = z.infer<typeof FileEntryDtoSchema>;

/** A line-based edit op (1-indexed inclusive), tagged by `op`. */
export const EditOpDtoSchema = z.discriminatedUnion("op", [
  z.object({ op: z.literal("replace"), startLine: z.number().int(), endLine: z.number().int(), text: z.string() }),
  z.object({ op: z.literal("insert"), afterLine: z.number().int(), text: z.string() }),
  z.object({ op: z.literal("delete"), startLine: z.number().int(), endLine: z.number().int() })
]);
export type EditOpDto = z.infer<typeof EditOpDtoSchema>;

/** A `grep` hit. */
export const GrepMatchDtoSchema = z.object({
  path: z.string(),
  lineNumber: z.number().int(),
  lineText: z.string()
});
export type GrepMatchDto = z.infer<typeof GrepMatchDtoSchema>;

/** A filesystem error, tagged by `kind` (mirrors the Rust `FsError`). */
export const FsErrorDtoSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("notFound"), path: z.string() }),
  z.object({ kind: z.literal("permissionDenied"), action: z.string(), path: z.string() }),
  z.object({ kind: z.literal("invalidPath"), reason: z.string() }),
  z.object({ kind: z.literal("invalidEdit"), reason: z.string() }),
  z.object({ kind: z.literal("notSupported") }),
  z.object({ kind: z.literal("serviceUnavailable"), reason: z.string() }),
  z.object({ kind: z.literal("backend"), reason: z.string() })
]);
export type FsErrorDto = z.infer<typeof FsErrorDtoSchema>;

/** The request envelope POSTed to the external backend: `{ op, runId, ...args }`. */
export const FsBackendRequestDtoSchema = z
  .object({
    op: z.enum(["read", "write", "edit", "delete", "rename", "ls", "glob", "grep"]),
    runId: z.string()
  })
  .passthrough();
export type FsBackendRequestDto = z.infer<typeof FsBackendRequestDtoSchema>;
