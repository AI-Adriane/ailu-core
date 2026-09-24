import type { GraphState } from "@ailu-ai/graph-core";
import type { RunEvent } from "@ailu-ai/graph-runtime";
import { describe, expect, it } from "vitest";

import { generateLlmsTxt } from "./llms-txt-generator.js";
import { componentSchemas, type ComponentSchema, paramTypeToJsonSchema } from "./schema-generator.js";
import { explainRun } from "./run-explainer.js";

// Loose fixture builder — branded RunId/NodeId are erased through the cast.
const state = (over: Record<string, unknown>): GraphState =>
  ({
    runId: "r1",
    graphId: "g1",
    currentNodeId: "n1",
    status: "running",
    version: 0,
    ...over
  }) as unknown as GraphState;

describe("AI-readable triad (ADR DX batch 3)", () => {
  describe("llms.txt", () => {
    it("is ground-truth: install, the model surface, a real component kind, error codes", () => {
      const txt = generateLlmsTxt();
      expect(txt).toMatch(/^# Ailu/);
      expect(txt).toContain("npm install @ailu-ai/graph-sdk");
      expect(txt).toContain("model.openai");
      expect(txt).toContain("`promptBuilder`"); // a real catalog kind, not hallucinated
      expect(txt).toContain("AILU_RUST_ENGINE_REQUIRED");
      expect(txt).toContain("conditionalEdge");
    });
  });

  describe("JSON Schema per node", () => {
    it("maps param type strings", () => {
      expect(paramTypeToJsonSchema("string")).toEqual({ type: "string" });
      expect(paramTypeToJsonSchema("number")).toEqual({ type: "number" });
      expect(paramTypeToJsonSchema("boolean")).toEqual({ type: "boolean" });
      expect(paramTypeToJsonSchema("string[]")).toEqual({ type: "array", items: { type: "string" } });
      expect(paramTypeToJsonSchema('"string" | "number" | "boolean"')).toEqual({
        type: "string",
        enum: ["string", "number", "boolean"]
      });
      expect(paramTypeToJsonSchema("RouterRule[]")).toEqual({ type: "array" }); // complex item → unconstrained
      expect(paramTypeToJsonSchema("RetrieverDoc")).toEqual({}); // unknown object → unconstrained
    });

    it("emits a per-component object schema with required + descriptions", () => {
      const schemas = componentSchemas();
      expect(schemas.promptBuilder).toBeDefined();
      const pb = schemas.promptBuilder as ComponentSchema;
      expect(pb.paramsSchema.type).toBe("object");
      expect(pb.paramsSchema.additionalProperties).toBe(false);
      expect(pb.paramsSchema.required).toContain("template");
      expect(pb.paramsSchema.properties?.template?.type).toBe("string");
      expect(pb.paramsSchema.properties?.template?.description).toBeTruthy();
    });
  });

  describe("explainRun", () => {
    it("explains a human-gate suspension with the next action", () => {
      const e = explainRun(
        state({
          currentNodeId: "review",
          status: "suspended",
          channels: { draft: "x", __suspend: { reason: "human-gate" } }
        })
      );
      expect(e.status).toBe("suspended");
      expect(e.suspended?.reason).toBe("human-gate");
      expect(e.suspended?.node).toBe("review");
      expect(e.suspended?.nextAction).toContain("resume");
      expect(e.summary).toContain("Suspended at \"review\"");
      expect(e.channels).toEqual(["draft"]); // __suspend filtered out, no values leaked
    });

    it("explains a signal wait", () => {
      const e = explainRun(
        state({
          status: "suspended",
          channels: { __suspend: { reason: "signal", awaitingSignal: "payment" } }
        })
      );
      expect(e.suspended?.awaitingSignal).toBe("payment");
      expect(e.suspended?.nextAction).toContain("payment");
    });

    it("surfaces a failure from the event log", () => {
      const events = [
        { type: "node_failed", runId: "r1", nodeId: "build", error: "boom", attempt: 1, timestamp: "0" }
      ] as unknown as RunEvent[];
      const e = explainRun(state({ status: "failed", currentNodeId: "build", channels: {} }), events);
      expect(e.failure).toEqual({ node: "build", error: "boom" });
      expect(e.summary).toContain("boom");
    });

    it("summarizes a completed run", () => {
      const e = explainRun(state({ status: "completed", channels: { result: 42 } }));
      expect(e.summary).toContain("Completed");
      expect(e.channels).toEqual(["result"]);
    });
  });
});
