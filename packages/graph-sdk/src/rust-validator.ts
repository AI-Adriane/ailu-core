import { createRequire } from "node:module";

import { GraphValidationError, type GraphDefinition, type GraphValidationErrorCode } from "@ailu-ai/graph-core";

/**
 * Optional bridge to the Rust engine's validator (`@ailu-ai/napi`). When the native
 * addon is present, graph validation runs in Rust — the first real consumer flipped
 * onto the Rust core per ADR 0002. When it isn't (no `.node` built), this returns
 * `null` and the SDK falls back to the TypeScript `validateGraph`. Same result either
 * way; the migration is invisible to callers.
 */
type NativeValidator = { validateGraphJson(definitionJson: string): string };
type RawValidationError = { code: string; message: string; path: (string | number)[] };

let cachedNative: NativeValidator | null | undefined;

const loadNative = (): NativeValidator | null => {
  if (cachedNative !== undefined) {
    return cachedNative;
  }
  try {
    const requireFn = createRequire(import.meta.url);
    cachedNative = requireFn("@ailu-ai/napi") as NativeValidator;
  } catch {
    cachedNative = null;
  }
  return cachedNative;
};

/** True when graph validation is being served by the Rust core. */
export const rustValidatorActive = (): boolean => loadNative() !== null;

/**
 * Validate a definition via the Rust core, or `null` if the native addon is
 * unavailable (so the caller can fall back to the TypeScript validator).
 */
export const tryRustValidate = (definition: GraphDefinition): GraphValidationError[] | null => {
  const native = loadNative();
  if (native === null) {
    return null;
  }
  try {
    const raw = native.validateGraphJson(JSON.stringify(definition));
    const parsed = JSON.parse(raw) as RawValidationError[];
    return parsed.map(
      (error) => new GraphValidationError(error.code as GraphValidationErrorCode, error.message, error.path)
    );
  } catch {
    // Any boundary hiccup (load, serialize, parse) → let the caller use TS.
    return null;
  }
};
