import { z } from "zod/v3";

/**
 * Tool-connector DTOs (ADR 0007). A connector links a knowledge namespace to an external
 * tool: OAuth providers (Notion, Slack, Google Drive, GitHub) or any MCP server. Tokens
 * live encrypted in the control plane and never cross this boundary.
 */
export const ConnectorAuthKindSchema = z.enum(["mcp", "oauth2"]);
export type ConnectorAuthKind = z.infer<typeof ConnectorAuthKindSchema>;

/** A connectable provider in the catalogue. `available` is false when its OAuth client
 * credentials are not configured in the environment (so the UI can disable it). */
export const ConnectorProviderDtoSchema = z.object({
  id: z.string(),
  label: z.string(),
  authKind: ConnectorAuthKindSchema,
  available: z.boolean(),
  scopes: z.array(z.string()).default([]),
  /** Extra fields the UI must collect at connect time (e.g. ["subdomain"] for Zendesk). */
  configFields: z.array(z.string()).default([])
});
export type ConnectorProviderDto = z.infer<typeof ConnectorProviderDtoSchema>;

/** An established connection (no secrets — credentials stay server-side, encrypted). */
export const ConnectionDtoSchema = z.object({
  id: z.string(),
  namespace: z.string(),
  provider: z.string(),
  authKind: ConnectorAuthKindSchema,
  status: z.string(),
  scopes: z.array(z.string()),
  createdBy: z.string(),
  lastSyncAt: z.string().nullable(),
  createdAt: z.string()
});
export type ConnectionDto = z.infer<typeof ConnectionDtoSchema>;

/** Body for POST /connectors/:provider/connect. `mcpUrl` is required when provider="mcp". */
export const ConnectConnectorDtoSchema = z.object({
  namespace: z.string().min(1),
  mcpUrl: z.string().url().optional(),
  /** Provider-specific connect-time config (e.g. { subdomain } for Zendesk). */
  config: z.record(z.string(), z.string()).optional()
});
export type ConnectConnectorDto = z.infer<typeof ConnectConnectorDtoSchema>;

/** Result of connect: an MCP connection is created inline; an OAuth one returns a URL to
 * redirect the browser to. Exactly one field is set. */
export const ConnectResultDtoSchema = z.object({
  connection: ConnectionDtoSchema.optional(),
  authorizeUrl: z.string().optional()
});
export type ConnectResultDto = z.infer<typeof ConnectResultDtoSchema>;

export const SyncResultDtoSchema = z.object({
  connectionId: z.string(),
  ingested: z.number().int(),
  lastSyncAt: z.string()
});
export type SyncResultDto = z.infer<typeof SyncResultDtoSchema>;
