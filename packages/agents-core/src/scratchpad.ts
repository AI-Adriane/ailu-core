import type { GraphState } from "@ailu/graph-core";

const SCRATCHPAD_KEY = "__scratchpad";

export const writeScratchpad = (
  state: GraphState,
  entry: unknown
): Record<string, unknown> => {
  const channels = state.channels as Record<string, unknown>;
  const existing = Array.isArray(channels[SCRATCHPAD_KEY]) ? (channels[SCRATCHPAD_KEY] as unknown[]) : [];
  return {
    [SCRATCHPAD_KEY]: [...existing, entry]
  };
};

export const clearScratchpad = (channels: Record<string, unknown>): Record<string, unknown> => {
  const next = { ...channels };
  delete next[SCRATCHPAD_KEY];
  return next;
};
