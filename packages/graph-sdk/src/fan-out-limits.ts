import { AiluSdkError } from "./errors.js";

/** Host/runtime options only; never part of a saved graph or its channels. */
export type FanOutLimits = {
  /** Default 128; integer in 1..1024. */
  maxItems?: number;
  /** Default 8; integer in 1..32, per map. */
  maxConcurrency?: number;
  /** Default 512; integer in 1..4096, per traversal. */
  maxTotalItems?: number;
};

export class InvalidFanOutLimitsError extends AiluSdkError {
  public constructor(message: string) {
    super(message, {
      code: "AILU_INVALID_FAN_OUT_LIMITS",
      hint: "Set host fanOutLimits to positive integers: maxItems <= 1024, maxConcurrency <= 32, maxTotalItems <= 4096."
    });
    this.name = "InvalidFanOutLimitsError";
  }
}

/** Validate and snapshot host options at construction. Rust independently enforces the same ceilings. */
export const validateFanOutLimits = (
  input: FanOutLimits | undefined
): Readonly<Required<FanOutLimits>> | undefined => {
  if (input === undefined) return undefined;
  if (input === null || typeof input !== "object" || Array.isArray(input)) {
    throw new InvalidFanOutLimitsError("fanOutLimits must be an object");
  }
  const bounds = {
    maxItems: [128, 1024],
    maxConcurrency: [8, 32],
    maxTotalItems: [512, 4096]
  } as const;
  for (const key of Object.keys(input)) {
    if (!Object.hasOwn(bounds, key)) {
      throw new InvalidFanOutLimitsError(`unknown fan-out limit '${key}'`);
    }
  }
  const resolved = { maxItems: 128, maxConcurrency: 8, maxTotalItems: 512 };
  for (const key of Object.keys(bounds) as (keyof FanOutLimits)[]) {
    const value = Object.hasOwn(input, key) ? input[key] : bounds[key][0];
    if (
      typeof value !== "number" ||
      !Number.isSafeInteger(value) ||
      value < 1 ||
      value > bounds[key][1]
    ) {
      throw new InvalidFanOutLimitsError(
        `fan-out limit '${key}' must be an integer in 1..${bounds[key][1]}`
      );
    }
    resolved[key] = value;
  }
  return Object.freeze(resolved);
};
