import { z } from "zod";

/**
 * Skills DTOs (ADR 0035 phase 12) — the SKILL.md surface shared across the API ↔ Studio boundary.
 *
 * A skill is procedural know-how loaded **progressively**: YAML frontmatter (`name` + `description`
 * — a cheap, always-resident index) plus an opaque markdown **body** loaded on demand. A skill is
 * **data, never code**: the body is prompt/context, never executed. The engine `ailu-skills`
 * crate is the source of truth for behaviour; these schemas type the registry/catalog surface the
 * control plane exposes and validate the SKILL.md frontmatter on registration.
 */

/** An L3 resource a skill body references, resolved on demand. `kind` ∈ `artifact | kb | path`. */
export const SkillResourceDtoSchema = z.object({
  kind: z.enum(["artifact", "kb", "path"]),
  /** The reference (an artifact ref, a KB doc id, or a relative path) — resolved lazily. */
  ref: z.string().min(1)
});

/** Who registered a skill version — provenance stamped on every registration (approver+, ADR 0035 D4). */
export const SkillProvenanceDtoSchema = z.object({
  principal: z.string().min(1).optional(),
  attestationId: z.string().min(1).optional(),
  registeredAt: z.string().datetime().optional(),
  status: z.enum(["asserted", "verified", "rejected"]).optional()
});

/**
 * SKILL.md frontmatter (the always-resident index). `name@version` is the pin reference; a
 * `requires`-bearing skill grants capability and is approval-gated on selection. `embeddingModel`
 * + `embeddingDim` pin the determinism anchor (anti-drift; re-embed only on a model change).
 */
export const SkillFrontmatterDtoSchema = z.object({
  /** Stable kebab id. */
  name: z.string().min(1),
  /** Semver; referenced as `name@major.minor.patch`. */
  version: z.string().min(1),
  /** The load-bearing, embeddable task-match paragraph. */
  description: z.string().min(1),
  /** Namespace convention: `skill:{tenant}:org` shared + `skill:{tenant}:agent:{id}`. */
  scope: z.string().min(1).optional(),
  /** Tools / profiles the body assumes — the governance trigger (approval-gated on selection). */
  requires: z.array(z.string().min(1)).optional(),
  /** L3 bundled resource references (resolved on demand). */
  resources: z.array(SkillResourceDtoSchema).optional(),
  embeddingModel: z.string().min(1).optional(),
  embeddingDim: z.number().int().min(1).optional()
});

/**
 * The agent-node `skills` overlay (mirrors the engine `SkillSpec`): `required` pins + an advisory
 * vector-selection cap, scoped to a tenant namespace. Identical to the `metadata.agent.skills`
 * carrier in {@link AgentNodeMetadataSchema} — re-exported here for catalog/registry consumers.
 */
export const SkillSpecDtoSchema = z.object({
  namespace: z.string().min(1),
  required: z.array(z.string().min(1)).optional(),
  advisoryK: z.number().int().min(0).optional()
});

/** One entry in a tenant's skill catalog: the frontmatter + provenance + lifecycle state. */
export const SkillCatalogEntryDtoSchema = SkillFrontmatterDtoSchema.extend({
  namespace: z.string().min(1),
  provenance: SkillProvenanceDtoSchema,
  /** Tombstoned versions are never selectable (versions are immutable — tombstone-not-mutate). */
  tombstoned: z.boolean().default(false),
  createdAt: z.string().datetime()
});

export type SkillResourceDto = z.infer<typeof SkillResourceDtoSchema>;
export type SkillProvenanceDto = z.infer<typeof SkillProvenanceDtoSchema>;
export type SkillFrontmatterDto = z.infer<typeof SkillFrontmatterDtoSchema>;
export type SkillSpecDto = z.infer<typeof SkillSpecDtoSchema>;
export type SkillCatalogEntryDto = z.infer<typeof SkillCatalogEntryDtoSchema>;
