/**
 * The operator's allow-list for the variables an `apiKeyEnv` may name — the TypeScript twin of
 * the engine's `ailu_runtime_bridge::api_key_env` (same variable, same rules). Only
 * {@link API_KEY_ENV_ALLOWLIST_ENV} and {@link ApiKeyEnvNotAllowedError} are public (re-exported
 * by the package root); the matcher is internal.
 */

/**
 * The operator variable that restricts which variables an `apiKeyEnv` may name — the same one,
 * with the same rules, as the engine's graph path. A comma-separated list of exact names and
 * prefixes ending in `*` (`AILU_ENDPOINT_*`). Unset: every name is allowed. Set, even to an empty
 * string: a name that matches no entry is refused before it is read
 * ({@link ApiKeyEnvNotAllowedError}).
 */
export const API_KEY_ENV_ALLOWLIST_ENV = "AILU_API_KEY_ENV_ALLOWLIST";

/**
 * A parsed {@link API_KEY_ENV_ALLOWLIST_ENV}: exact names, and prefixes (the `*` dropped).
 * @internal Not exported from the package root.
 */
export type ApiKeyEnvAllowlist = {
  readonly exact: readonly string[];
  readonly prefixes: readonly string[];
};

/**
 * Parse an allow-list value. Entries are trimmed, blank ones ignored; only a trailing `*` is a
 * wildcard. An empty or blank value allows no name.
 * @internal Not exported from the package root.
 */
export function parseApiKeyEnvAllowlist(raw: string): ApiKeyEnvAllowlist {
  const exact: string[] = [];
  const prefixes: string[] = [];
  for (const entry of raw.split(",").map((part) => part.trim())) {
    if (entry === "") continue;
    if (entry.endsWith("*")) prefixes.push(entry.slice(0, -1));
    else exact.push(entry);
  }
  return { exact, prefixes };
}

/**
 * Whether `name` is one of the exact names or starts with one of the prefixes (case-sensitive).
 * @internal Not exported from the package root.
 */
export function apiKeyEnvAllowed(allowlist: ApiKeyEnvAllowlist, name: string): boolean {
  return (
    allowlist.exact.includes(name) || allowlist.prefixes.some((prefix) => name.startsWith(prefix))
  );
}

/** An `apiKeyEnv` named a variable that {@link API_KEY_ENV_ALLOWLIST_ENV} does not allow. Names
 * the variable, never a value. */
export class ApiKeyEnvNotAllowedError extends Error {
  readonly code = "AILU_API_KEY_ENV_NOT_ALLOWED";
  readonly hint: string;
  constructor(readonly envVar: string) {
    super(
      `apiKeyEnv names "${envVar}", which ${API_KEY_ENV_ALLOWLIST_ENV} does not allow: ` +
        `this host only reads keys from the variables its operator listed there.`
    );
    this.name = "ApiKeyEnvNotAllowedError";
    this.hint =
      `Name a variable that ${API_KEY_ENV_ALLOWLIST_ENV} allows, ` +
      `or ask the operator to add "${envVar}".`;
  }
}

/**
 * The variable an `apiKeyEnv` names, trimmed as the engine trims it; `undefined` when it names
 * none (absent or blank — a keyless endpoint, or a provider's default variable).
 * @internal Not exported from the package root.
 */
export const namedApiKeyEnv = (apiKeyEnv: string | undefined): string | undefined => {
  const name = apiKeyEnv?.trim();
  return name === undefined || name === "" ? undefined : name;
};

/**
 * Refuses an explicit `apiKeyEnv` the operator's allow-list does not allow, before it is read.
 * @internal Not exported from the package root.
 */
export const assertApiKeyEnvAllowed = (
  name: string,
  env: Record<string, string | undefined>
): void => {
  const raw = env[API_KEY_ENV_ALLOWLIST_ENV];
  if (raw !== undefined && !apiKeyEnvAllowed(parseApiKeyEnvAllowlist(raw), name)) {
    throw new ApiKeyEnvNotAllowedError(name);
  }
};
