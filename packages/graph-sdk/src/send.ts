import type { GraphState } from "@ailu-ai/graph-core";

/**
 * Dynamic-message (`send`) helpers. Pre-queue inputs for a node via
 * {@link import("./compiled-graph.js").RunOptions.inbox}; each node execution consumes
 * the next queued input, exposed under the reserved `__injected` channel. A node handler
 * reads it with {@link readInjected}. The map-reduce / dynamic-dispatch seam.
 */

/** Reserved channel exposing a `send`-injected input to a node handler (per execution). */
export const INJECTED_KEY = "__injected";

/**
 * Read the `send`-injected input from a node's state, if the node consumed a queued
 * input this execution; `undefined` otherwise. The value is visible to the handler only
 * and is never persisted into the run's channels.
 */
export const readInjected = (state: Pick<GraphState, "channels">): unknown =>
  (state.channels as Record<string, unknown>)[INJECTED_KEY];
