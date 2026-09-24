import { z } from "zod";

/**
 * Resource-search DTOs (ADR 0011) — the API↔Studio wire shapes for the unified search over
 * graphs / agents / KB. Mirrors the engine `@ailu/search` types, minus `tenantId` (the
 * caller's tenant is implicit; never echoed to the client).
 */

export const SearchResourceTypeSchema = z.enum(["graph", "agent", "kb"]);
export type SearchResourceType = z.infer<typeof SearchResourceTypeSchema>;

/** One ranked search result. */
export const SearchHitDtoSchema = z.object({
  type: SearchResourceTypeSchema,
  id: z.string(),
  title: z.string(),
  snippet: z.string(),
  href: z.string(),
  score: z.number(),
  namespace: z.string().optional()
});
export type SearchHitDto = z.infer<typeof SearchHitDtoSchema>;

/** Response wrapper for `GET /search`. */
export const SearchResultsDtoSchema = z.object({
  query: z.string(),
  hits: z.array(SearchHitDtoSchema)
});
export type SearchResultsDto = z.infer<typeof SearchResultsDtoSchema>;
