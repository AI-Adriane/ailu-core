import { z } from "zod/v3";

import { ConfigValidationError } from "./errors.js";

const EnvironmentSchema = z.object({
  NODE_ENV: z.enum(["local", "staging", "production"]),
  PORT: z.coerce.number().int().positive().default(3000),
  DATABASE_URL: z.string().min(1),
  REDIS_URL: z.string().min(1),
  JWT_SECRET: z.string().min(1),
  JWT_EXPIRY: z.string().min(1).default("1h"),
  /**
   * Disable authentication entirely (dev/seed/offline). When `true`, the global
   * JwtAuthGuard injects a system principal instead of rejecting unauthenticated
   * requests — so local boot, the seed, and offline scripts keep working without a
   * login. NEVER enable in staging/production. Accepts the usual env truthy strings.
   */
  AUTH_DISABLED: z
    .preprocess(
      (value) => (typeof value === "string" ? value.toLowerCase() === "true" || value === "1" : value),
      z.boolean()
    )
    .default(false),
  /**
   * Shared secret for machine-to-machine (worker → API) authentication. The worker sends
   * it as `Authorization: Bearer <token>` on register/heartbeat/deregister; the
   * WorkerTokenGuard compares it in constant time. Optional in NODE_ENV=local (the fleet
   * routes are reachable via AUTH_DISABLED there), but REQUIRED and non-empty everywhere
   * else — see the fail-secure superRefine below.
   */
  WORKER_TOKEN: z.preprocess((v) => (v === "" ? undefined : v), z.string().min(1).optional()),
  OPENAI_API_KEY: z.string().min(1).optional(),
  ANTHROPIC_API_KEY: z.string().min(1).optional(),
  MISTRAL_API_KEY: z.string().min(1).optional(),
  /**
   * Resource search (ADR 0011). When set, the control plane indexes/queries graphs, agents and
   * KB docs in Elasticsearch; unset = the in-memory fallback provider (dev/test/no-ES deploy).
   * `ELASTICSEARCH_API_KEY` is the optional base64 `id:api_key` for managed ES (Elastic Cloud).
   */
  ELASTICSEARCH_URL: z.preprocess((v) => (v === "" ? undefined : v), z.string().min(1).optional()),
  ELASTICSEARCH_API_KEY: z.preprocess((v) => (v === "" ? undefined : v), z.string().min(1).optional()),
  OTEL_ENDPOINT: z.string().min(1).optional(),
  LOG_LEVEL: z.enum(["debug", "info", "warn", "error"]).default("info")
}).superRefine((env, ctx) => {
  // Fail-secure: AUTH_DISABLED is a dev/offline-only escape hatch. Refuse to boot with it
  // enabled anywhere but NODE_ENV=local, so a misconfigured staging/prod can never run
  // unauthenticated even if someone sets the flag.
  if (env.AUTH_DISABLED && env.NODE_ENV !== "local") {
    ctx.addIssue({
      code: z.ZodIssueCode.custom,
      path: ["AUTH_DISABLED"],
      message: "AUTH_DISABLED must not be enabled outside NODE_ENV=local"
    });
  }
  // Fail-secure (same logic as AUTH_DISABLED): outside NODE_ENV=local the worker MUST
  // authenticate with a real shared secret. Refuse to boot staging/prod without a
  // non-empty WORKER_TOKEN, so the m2m fleet routes can never run unauthenticated.
  if (env.NODE_ENV !== "local" && (env.WORKER_TOKEN === undefined || env.WORKER_TOKEN.length === 0)) {
    ctx.addIssue({
      code: z.ZodIssueCode.custom,
      path: ["WORKER_TOKEN"],
      message: "WORKER_TOKEN is required (non-empty) outside NODE_ENV=local"
    });
  }
});

export type AppEnv = z.infer<typeof EnvironmentSchema>;

let cachedEnv: AppEnv | null = null;

export const parseEnv = (source: NodeJS.ProcessEnv = process.env): AppEnv => {
  const parsed = EnvironmentSchema.safeParse(source);
  if (!parsed.success) {
    throw new ConfigValidationError(parsed.error.issues);
  }
  return parsed.data;
};

export const getEnv = (): AppEnv => {
  if (cachedEnv === null) {
    cachedEnv = parseEnv(process.env);
  }
  return cachedEnv;
};
