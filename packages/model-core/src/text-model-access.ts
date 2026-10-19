/** Admission policy supplied by the trusted control plane. Never an agent/model instruction. */
export type TextModelAccessSnapshot = Readonly<{
  schemaVersion: 1;
  tenantId: string;
  runId: string;
  policyId: string;
  policyVersion: number;
  catalogRevision: string;
  policyHash: string;
  allowedModels: readonly Readonly<{ provider: string; modelId: string }>[];
}>;

/** Older native addons ignore unknown JSON fields, so sending the policy alone is unsafe. */
export class TextModelAccessUnsupportedError extends Error {
  readonly code = "AILU_TEXT_MODEL_ACCESS_UNSUPPORTED";
  constructor() {
    super("The native engine does not support text-model access schema 1; no request was sent.");
    this.name = "TextModelAccessUnsupportedError";
  }
}

export const assertTextModelAccessSupport = (
  native: { textModelAccessVersion?: () => number },
  snapshot: TextModelAccessSnapshot | undefined
): void => {
  if (
    snapshot !== undefined &&
    (typeof native.textModelAccessVersion !== "function" || native.textModelAccessVersion() !== 1)
  ) {
    throw new TextModelAccessUnsupportedError();
  }
};
