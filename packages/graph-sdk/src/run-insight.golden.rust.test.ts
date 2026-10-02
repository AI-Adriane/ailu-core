import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import {
  explainRun,
  rustEngineAvailable,
  verifyReplayDecisions,
  type GraphState,
  type ReplayDecision,
  type RunEvent
} from "./index.js";

/**
 * ADR 0045 D3.4 — golden cases for what a host reads about a run: `explainRun(state, events?)` and
 * `verifyReplayDecisions(attested, replayed)`, recorded from the TypeScript SDK (2.2.0, its results
 * as JSON) before the engine took them over. The file is frozen: the SDK, now through the engine,
 * must keep giving the recorded answers — and so must `@ailu-ai/napi` here, and the Rust and Python
 * tests that read the same file. A deliberate change edits the file by hand.
 */
const FIXTURE = fileURLToPath(
  new URL("../../../crates/runtime-bridge/tests/fixtures/run_insight_golden.json", import.meta.url)
);

type ExplainCase = {
  name: string;
  kind: "explain";
  input: { state: unknown; events?: unknown[] };
  expected: unknown;
};
type VerifyCase = {
  name: string;
  kind: "verify";
  input: { attested: unknown[]; replayed: unknown[] };
  expected: unknown;
};
type GoldenCase = ExplainCase | VerifyCase;

const napi = (): {
  engineExplainRun: (stateJson: string, eventsJson?: string | null) => string;
  engineVerifyReplayDecisions: (attestedJson: string, replayedJson: string) => string;
} => createRequire(import.meta.url)("@ailu-ai/napi");

/** The SDK's answer for a case, as JSON. */
const sdkResult = (golden: Omit<GoldenCase, "expected">): unknown =>
  JSON.parse(
    JSON.stringify(
      golden.kind === "explain"
        ? explainRun(
            (golden as ExplainCase).input.state as GraphState,
            (golden as ExplainCase).input.events as RunEvent[] | undefined
          )
        : verifyReplayDecisions(
            (golden as VerifyCase).input.attested as ReplayDecision[],
            (golden as VerifyCase).input.replayed as ReplayDecision[]
          )
    )
  );

/** The engine's answer for a case. */
const engineResult = (golden: GoldenCase): unknown =>
  golden.kind === "explain"
    ? JSON.parse(
        napi().engineExplainRun(
          JSON.stringify(golden.input.state),
          golden.input.events === undefined ? undefined : JSON.stringify(golden.input.events)
        )
      )
    : JSON.parse(
        napi().engineVerifyReplayDecisions(
          JSON.stringify(golden.input.attested),
          JSON.stringify(golden.input.replayed)
        )
      );

const readGolden = (): GoldenCase[] => JSON.parse(readFileSync(FIXTURE, "utf8")) as GoldenCase[];

const native = rustEngineAvailable() ? describe : describe.skip;

native("@ailu-ai/graph-sdk — run insight golden cases (ADR 0045 D3.4)", () => {
  it("the SDK explains runs and checks replays as recorded", () => {
    expect(readGolden().length).toBeGreaterThanOrEqual(20);
    for (const golden of readGolden()) {
      expect({ name: golden.name, got: sdkResult(golden) }).toEqual({
        name: golden.name,
        got: golden.expected
      });
    }
  });

  it("the engine explains runs and checks replays as recorded", () => {
    for (const golden of readGolden()) {
      expect({ name: golden.name, got: engineResult(golden) }).toEqual({
        name: golden.name,
        got: golden.expected
      });
    }
  });
});
