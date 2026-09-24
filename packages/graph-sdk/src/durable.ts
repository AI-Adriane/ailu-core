import type { GraphState } from "@ailu-ai/graph-core";

/**
 * Durable-timer / external-signal helpers for node handlers (ADR 0009). A node handler
 * returns one of these to make the run SUSPEND after applying its channel update:
 * - {@link sleepUntil} — a durable timer: the run waits until an external scheduler
 *   resumes it at `wakeAt` (the engine never sleeps — `wakeAt` is opaque data).
 * - {@link waitForSignal} — wait for a named external signal delivered via
 *   {@link import("./compiled-graph.js").CompiledGraph.signal}; optionally with a
 *   timeout `wakeAt` (signal-or-timeout).
 *
 * Both are reserved-key markers the Rust engine recognises across the napi seam; they
 * run on the Rust engine (the production runtime), not the TypeScript dev fallback.
 */

/** Reserved handler-return key requesting a durable-timer suspension. */
export const SLEEP_UNTIL_KEY = "__sleepUntil";
/** Reserved handler-return key requesting a signal-wait suspension. */
export const WAIT_FOR_SIGNAL_KEY = "__waitForSignal";
/** Channel key carrying the suspend reason + scheduler hints on a suspended run. */
export const SUSPEND_META_KEY = "__suspend";
/** Channel key carrying delivered signal payloads, keyed by signal name. */
export const SIGNALS_KEY = "__signals";

/** Why a run is suspended, with any timer / signal scheduler hints. */
export type SuspendMeta = {
  /** `"human-gate" | "interrupt" | "timer" | "signal"`. */
  reason: string;
  /** For a durable timer (or signal-or-timeout): when to resume. Opaque to the engine. */
  wakeAt?: string;
  /** For a signal wait: the signal name a `signal(...)` must deliver. */
  awaitingSignal?: string;
};

/**
 * Node-handler return: suspend as a **durable timer** until `wakeAt`, applying `update`
 * to the channels first. `wakeAt` is an opaque deadline (e.g. ISO-8601) — the engine
 * stores it and never reads a clock; the control-plane scheduler resumes the run then.
 * On resume the run advances past this node.
 */
export const sleepUntil = (
  wakeAt: string,
  update: Record<string, unknown> = {}
): Record<string, unknown> => ({ ...update, [SLEEP_UNTIL_KEY]: wakeAt });

/**
 * Node-handler return: suspend awaiting the external signal `name`, applying `update`
 * first. Deliver it with {@link import("./compiled-graph.js").CompiledGraph.signal};
 * the payload lands in `__signals[name]`. Pass `wakeAt` for a signal-OR-timeout (the
 * run also wakes at `wakeAt` if the signal never arrives).
 */
export const waitForSignal = (
  name: string,
  options: { wakeAt?: string; update?: Record<string, unknown> } = {}
): Record<string, unknown> => ({
  ...(options.update ?? {}),
  [WAIT_FOR_SIGNAL_KEY]: name,
  ...(options.wakeAt === undefined ? {} : { [SLEEP_UNTIL_KEY]: options.wakeAt })
});

/**
 * Read the suspend metadata off a (suspended) run state — the control-plane scheduler
 * uses `wakeAt` to know when to resume a timer and `awaitingSignal` to route a signal.
 * Returns `undefined` when the run is not suspended on a timer / signal.
 */
export const readSuspendMeta = (state: Pick<GraphState, "channels">): SuspendMeta | undefined => {
  const raw = (state.channels as Record<string, unknown>)[SUSPEND_META_KEY];
  if (raw !== null && typeof raw === "object" && typeof (raw as SuspendMeta).reason === "string") {
    return raw as SuspendMeta;
  }
  return undefined;
};

/** Read a delivered signal's payload from a run state's `__signals` channel. */
export const readSignal = (state: Pick<GraphState, "channels">, name: string): unknown => {
  const signals = (state.channels as Record<string, unknown>)[SIGNALS_KEY];
  return signals !== null && typeof signals === "object"
    ? (signals as Record<string, unknown>)[name]
    : undefined;
};
