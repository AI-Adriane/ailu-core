import { z } from "zod/v3";

/**
 * A document stored in the knowledge base (without its embedding vector). Carries the
 * Open Knowledge Format (OKF) frontmatter so a bundle round-trips: `type` (OKF's only
 * required field), plus the recommended `title`/`description`/`resource`/`tags`/`timestamp`,
 * the bundle-relative `path`, the markdown cross-reference `links`, and any extra producer
 * keys preserved in `frontmatter`.
 */
export const KbDocumentDtoSchema = z.object({
  id: z.string().min(1),
  namespace: z.string().min(1),
  content: z.string(),
  type: z.string().min(1),
  title: z.string().optional(),
  description: z.string().optional(),
  resource: z.string().optional(),
  timestamp: z.string().optional(),
  path: z.string().optional(),
  tags: z.array(z.string()).optional(),
  links: z.array(z.string()).optional(),
  createdAt: z.string().datetime()
});

/** A single semantic-search hit: the matched document and its cosine score (1 = identical). */
export const KbSearchHitDtoSchema = z.object({
  id: z.string().min(1),
  content: z.string(),
  score: z.number(),
  type: z.string().optional(),
  title: z.string().optional()
});

/**
 * Body for ingesting raw documents into a namespace — each is embedded then stored. `.max(500)`
 * (issue #453, security audit 2026-07-20 MEDIUM finding): the array itself had no upper bound,
 * so a single request could enqueue an unbounded embedding workload. 500 comfortably covers a
 * real batch ingest without leaving the endpoint effectively unbounded.
 */
export const IngestKbDocumentsDtoSchema = z.object({
  documents: z
    .array(
      z.object({
        id: z.string().min(1).optional(),
        content: z.string().min(1)
      })
    )
    .min(1)
    .max(500)
});

/** One file of an OKF bundle: a bundle-relative path and its raw markdown contents. */
export const OkfFileSchema = z.object({
  path: z.string().min(1),
  content: z.string()
});

/** Body for ingesting an OKF bundle — a set of markdown (+frontmatter) files. */
export const IngestOkfBundleDtoSchema = z.object({
  // Same unbounded-array gap as IngestKbDocumentsDtoSchema.documents (issue #453) — capped for
  // the same reason: a client-supplied file array with no upper bound.
  files: z.array(OkfFileSchema).min(1).max(500)
});

/** An OKF bundle returned by export — markdown files reconstructed from stored documents. */
export const OkfBundleDtoSchema = z.object({
  files: z.array(OkfFileSchema)
});

/** Body for ingesting a web page by URL — fetched, stripped to text, stored with provenance. */
export const IngestUrlDtoSchema = z.object({
  url: z.string().url(),
  title: z.string().optional()
});

/**
 * Body for ingesting records from a JSON HTTP API (the generic SaaS-connector substrate:
 * Slack/Notion/HubSpot are this with their base URL + a bearer header). Fetches `url`,
 * walks `itemsPath` to an array, and maps each record's fields to an OKF document.
 */
export const IngestApiDtoSchema = z.object({
  url: z.string().url(),
  /** Dot path to the array of records (omit when the body is itself the array/object). */
  itemsPath: z.string().optional(),
  /** Field holding each record's text content. */
  contentField: z.string().min(1),
  idField: z.string().optional(),
  titleField: z.string().optional(),
  /** Extra request headers, e.g. `{ "authorization": "Bearer <token>" }`. */
  headers: z.record(z.string(), z.string()).optional()
});

/** A node of the knowledge graph (a KB document). */
export const KbGraphNodeSchema = z.object({
  id: z.string().min(1),
  title: z.string().optional(),
  type: z.string()
});

/** A typed edge of the knowledge graph: `from --type--> to`. */
export const KbGraphEdgeSchema = z.object({
  from: z.string().min(1),
  to: z.string().min(1),
  type: z.string()
});

/** The knowledge graph of a namespace: typed relations between documents. */
export const KbGraphDtoSchema = z.object({
  nodes: z.array(KbGraphNodeSchema),
  edges: z.array(KbGraphEdgeSchema)
});

export type KbGraphNode = z.infer<typeof KbGraphNodeSchema>;
export type KbGraphEdge = z.infer<typeof KbGraphEdgeSchema>;
export type KbGraphDto = z.infer<typeof KbGraphDtoSchema>;

/** Body for activation: search the KB for `query` and push the hits to a downstream webhook. */
export const ActivateDtoSchema = z.object({
  query: z.string().min(1),
  webhookUrl: z.string().url(),
  k: z.number().int().min(1).max(20).optional()
});

/** Result of an activation delivery. */
export const ActivationResultDtoSchema = z.object({
  delivered: z.boolean(),
  status: z.number().int(),
  hits: z.number().int().min(0)
});

export type ActivateDto = z.infer<typeof ActivateDtoSchema>;
export type ActivationResultDto = z.infer<typeof ActivationResultDtoSchema>;

export type KbDocumentDto = z.infer<typeof KbDocumentDtoSchema>;
export type KbSearchHitDto = z.infer<typeof KbSearchHitDtoSchema>;
export type IngestKbDocumentsDto = z.infer<typeof IngestKbDocumentsDtoSchema>;
export type OkfFile = z.infer<typeof OkfFileSchema>;
export type IngestOkfBundleDto = z.infer<typeof IngestOkfBundleDtoSchema>;
export type OkfBundleDto = z.infer<typeof OkfBundleDtoSchema>;
export type IngestUrlDto = z.infer<typeof IngestUrlDtoSchema>;
export type IngestApiDto = z.infer<typeof IngestApiDtoSchema>;
