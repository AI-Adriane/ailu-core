import { z } from "zod";

import { ExampleGraphDtoSchema } from "./graphs.js";

/**
 * The component / prebuilt-agent / model-tier catalog contract: the shape the API
 * serves so Studio can render the building-block library without importing the engine
 * or the SDK. The SDK owns the source metadata (`@ailu-ai/graph-sdk` `catalog`); the
 * API validates it against these schemas and forwards it unchanged.
 */

/** A single parameter a component factory accepts. */
export const ComponentParamDtoSchema = z.object({
  name: z.string().min(1),
  type: z.string().min(1),
  required: z.boolean(),
  description: z.string().min(1)
});

/** The category buckets a component falls into in the library. */
export const ComponentCategoryDtoSchema = z.enum([
  "prompt",
  "validation",
  "parsing",
  "routing",
  "retrieval",
  "text",
  "data",
  "integration",
  // --- wave two: Haystack-gap categories ---
  "splitter",
  "generation",
  "evaluation",
  "writer"
]);

/** One entry in the component library: a `kind` plus its presentation + params. */
export const ComponentCatalogEntryDtoSchema = z.object({
  kind: z.string().min(1),
  title: z.string().min(1),
  category: ComponentCategoryDtoSchema,
  description: z.string().min(1),
  params: z.array(ComponentParamDtoSchema),
  /** `true` for vendor-I/O integration components (httpFetch / webSearch). */
  integration: z.boolean()
});

/** The four capability tiers, wire-compatible (camelCase) with the engine `ModelTier`. */
export const ModelTierDtoSchema = z.enum(["frontier", "balanced", "fast", "creative"]);

/** One entry in the prebuilt-agent catalog, mirroring the Rust `PrebuiltAgent` table. */
export const PrebuiltAgentCatalogEntryDtoSchema = z.object({
  name: z.string().min(1),
  title: z.string().min(1),
  description: z.string().min(1),
  tier: ModelTierDtoSchema,
  tools: z.array(z.string().min(1)),
  suspendForApproval: z.boolean(),
  outputChannel: z.string().min(1)
});

/** Describes one capability tier plus its recommended per-provider models. */
export const ModelTierInfoDtoSchema = z.object({
  tier: ModelTierDtoSchema,
  description: z.string().min(1),
  /** `provider -> model` recommended defaults for this tier. */
  models: z.record(z.string(), z.string())
});

/** The full catalog the API serves to Studio. */
export const CatalogDtoSchema = z.object({
  components: z.array(ComponentCatalogEntryDtoSchema),
  prebuilt: z.array(PrebuiltAgentCatalogEntryDtoSchema),
  tiers: z.array(ModelTierInfoDtoSchema),
  /** SDK-authored example graphs, served so Studio can render + import them. */
  exampleGraphs: z.array(ExampleGraphDtoSchema)
});

export type ComponentParamDto = z.infer<typeof ComponentParamDtoSchema>;
export type ComponentCategoryDto = z.infer<typeof ComponentCategoryDtoSchema>;
export type ComponentCatalogEntryDto = z.infer<typeof ComponentCatalogEntryDtoSchema>;
export type ModelTierDto = z.infer<typeof ModelTierDtoSchema>;
export type PrebuiltAgentCatalogEntryDto = z.infer<typeof PrebuiltAgentCatalogEntryDtoSchema>;
export type ModelTierInfoDto = z.infer<typeof ModelTierInfoDtoSchema>;
export type CatalogDto = z.infer<typeof CatalogDtoSchema>;
