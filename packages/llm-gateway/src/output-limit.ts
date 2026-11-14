// Mirrors the Rust gateway's output_limit (ADR 0060).
const MODEL_OUTPUT_LIMITS = new Map<string, number>([
  ["gpt-4", 8192],
  ["gpt-4-0613", 8192],
  ["gpt-4-turbo", 4096],
  ["gpt-3.5-turbo", 4096],
  ["claude-3-haiku-20240307", 4096],
  ["gemini-2.0-flash", 8192],
  ["gemini-2.0-flash-001", 8192]
]);

export const outputLimit = (model: string, requested?: number): number => {
  const id = model.replace(/^(openai|anthropic|google)\//, "");
  return Math.min(requested ?? 16000, MODEL_OUTPUT_LIMITS.get(id) ?? Infinity);
};
