import { z } from "zod";

/**
 * Administration DTOs (ADR 0010) — admin-managed, per-tenant config that replaces `.env` for
 * self-hosting clients. Secrets are WRITE-ONLY across the wire: set them with the `Set*` DTOs;
 * read DTOs never carry the secret, only `configured` + a last-4 hint.
 */

/** Providers whose keys can be set in the admin UI. */
export const LlmProviderSchema = z.enum([
  "openai",
  "anthropic",
  "mistral",
  "google",
  "openrouter",
  "minimax",
  "huggingface",
  "ollama",
  "lmstudio"
]);
export type LlmProvider = z.infer<typeof LlmProviderSchema>;

/** Returned: what's configured, never the key. `source` says where the key resolves from. */
export const LlmProviderKeyDtoSchema = z.object({
  provider: LlmProviderSchema,
  configured: z.boolean(),
  source: z.enum(["tenant", "env", "none"]),
  last4: z.string().optional(),
  baseUrl: z.string().optional(),
  defaultModel: z.string().optional()
});
export type LlmProviderKeyDto = z.infer<typeof LlmProviderKeyDtoSchema>;

/**
 * Body for PUT /admin/llm-providers/:provider — set/replace the tenant's key.
 *
 * `apiKey` is OPTIONAL because the local providers in {@link LlmProviderSchema} (`ollama`,
 * `lmstudio`) are KEYLESS: they are reached over a `baseUrl` with no credential, so requiring a key
 * made them impossible to configure at all. It keeps `.min(1)` when present, so a cloud provider can
 * still never store an empty secret. "Required unless the provider is local" is enforced by the API,
 * which knows which provider the route targets — the schema does not.
 */
export const SetLlmProviderKeyDtoSchema = z.object({
  apiKey: z.string().min(1).optional(),
  baseUrl: z.string().optional(),
  defaultModel: z.string().optional()
});
export type SetLlmProviderKeyDto = z.infer<typeof SetLlmProviderKeyDtoSchema>;

/** Connector OAuth *app* credential — what's configured (no secret returned). */
export const ConnectorAppCredentialDtoSchema = z.object({
  provider: z.string(),
  configured: z.boolean(),
  source: z.enum(["tenant", "env", "none"]),
  clientIdLast4: z.string().optional()
});
export type ConnectorAppCredentialDto = z.infer<typeof ConnectorAppCredentialDtoSchema>;

/** Body for PUT /admin/connector-apps/:provider — set the tenant's OAuth app creds. */
export const SetConnectorAppCredentialDtoSchema = z.object({
  clientId: z.string().min(1),
  clientSecret: z.string().min(1)
});
export type SetConnectorAppCredentialDto = z.infer<typeof SetConnectorAppCredentialDtoSchema>;

/** Result of a "test this provider key" call. */
export const ProviderTestResultDtoSchema = z.object({
  ok: z.boolean(),
  detail: z.string().optional()
});
export type ProviderTestResultDto = z.infer<typeof ProviderTestResultDtoSchema>;
