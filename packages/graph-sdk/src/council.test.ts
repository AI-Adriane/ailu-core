import { describe, expect, it } from "vitest";

import { aggregateRanks, anonymizeAndShuffle, parseRanking } from "./council.js";

describe("anonymizeAndShuffle (ADR 0013 / 0061)", () => {
  const answers = [
    { memberId: "gpt", content: "answer-gpt" },
    { memberId: "claude", content: "answer-claude" },
    { memberId: "mistral", content: "answer-mistral" }
  ];

  it("relabels A,B,C and keeps content + memberId for de-anonymization", () => {
    const field = anonymizeAndShuffle(answers, "seed1");
    expect(field.map((f) => f.label)).toEqual(["A", "B", "C"]);
    // every original answer survives (content + memberId retained), just reordered/relabeled.
    expect(new Set(field.map((f) => f.memberId))).toEqual(new Set(["gpt", "claude", "mistral"]));
    for (const f of field) {
      expect(f.content).toBe(answers.find((a) => a.memberId === f.memberId)!.content);
    }
  });

  it("is deterministic per seed (replay-faithful) and reorders across seeds", () => {
    expect(anonymizeAndShuffle(answers, "seed1")).toEqual(anonymizeAndShuffle(answers, "seed1"));
    const order = (seed: string) => anonymizeAndShuffle(answers, seed).map((f) => f.memberId);
    // The shuffle depends on the seed; at least one seed yields a non-input order.
    expect([order("s1"), order("s2"), order("s3")].some((o) => o.join() !== "gpt,claude,mistral")).toBe(
      true
    );
  });
});

describe("aggregateRanks (Borda)", () => {
  it("ranks the field by summed Borda score, best-first", () => {
    // Two reviewers both prefer B over A over C → B wins, then A, then C.
    const out = aggregateRanks(
      [
        ["B", "A", "C"],
        ["B", "A", "C"]
      ],
      ["A", "B", "C"]
    );
    expect(out).toEqual(["B", "A", "C"]);
  });

  it("breaks ties by label asc and scores unranked labels 0", () => {
    // One reviewer ranks only A; B and C stay at 0 → A first, then B,C by label.
    expect(aggregateRanks([["A"]], ["A", "B", "C"])).toEqual(["A", "B", "C"]);
  });

  it("ignores unknown/duplicate labels in a ranking", () => {
    expect(aggregateRanks([["B", "B", "Z", "A"]], ["A", "B"])).toEqual(["B", "A"]);
  });
});

describe("parseRanking", () => {
  it("extracts named labels in order from prose, deduped", () => {
    expect(parseRanking("I rank B first, then A, then C.", ["A", "B", "C"])).toEqual(["B", "A", "C"]);
  });

  it("counts only whole-word labels, not the first letter of every word", () => {
    const labels = ["A", "B", "C"];
    expect(parseRanking("A is clearly better than B", labels)).toEqual(["A", "B"]);
    expect(parseRanking("Answer B is the best", labels)).toEqual(["B"]);
    expect(parseRanking("C gives a clearer answer than B", labels)).toEqual(["C", "B"]);
  });

  it("drops unknown labels and is empty when none match", () => {
    expect(parseRanking("none of these", ["A", "B"])).toEqual([]);
  });
});

import { council } from "./council.js";
import { isCatalogGraph, validateGraph } from "./index.js";

describe("council(...) graph (ADR 0061 E2)", () => {
  const seat = () => ({ prompt: { system: "s" }, provider: "mistral" as const });

  it("is a valid CATALOG graph — every node carries a component/agent carrier (no JS handlers)", () => {
    const def = council({ members: [seat(), seat(), seat()], chair: seat() });
    expect(validateGraph(def)).toEqual([]);
    expect(isCatalogGraph(def)).toBe(true);
  });

  it("wires dispatch → members (fan-out) → anonymize → reviewers (fan-out) → aggregate → chair", () => {
    const def = council({ members: [seat(), seat(), seat()], chair: seat() });
    const ids = def.nodes.map((n) => String(n.id));
    expect(ids).toEqual(
      expect.arrayContaining([
        "dispatch",
        "member_0",
        "member_1",
        "member_2",
        "anonymize",
        "review_0",
        "review_1",
        "review_2",
        "aggregate",
        "chair"
      ])
    );
    const byId = (id: string) => def.nodes.find((n) => String(n.id) === id);
    // anonymize/aggregate are catalog components (Rust), not JS action handlers.
    expect((byId("anonymize")?.metadata as { component?: { kind?: string } }).component?.kind).toBe(
      "councilAnonymize"
    );
    expect((byId("aggregate")?.metadata as { component?: { kind?: string } }).component?.kind).toBe(
      "councilAggregate"
    );
    // dispatch fans out to all members (join at anonymize); anonymize fans out to reviewers.
    expect((byId("dispatch") as { fanOut?: { parallelTo: string[]; joinAt: string } }).fanOut).toEqual({
      parallelTo: ["member_0", "member_1", "member_2"],
      joinAt: "anonymize"
    });
    expect((byId("anonymize") as { fanOut?: { joinAt: string } }).fanOut?.joinAt).toBe("aggregate");
  });

  it("inserts an optional human gate before the chair", () => {
    const def = council({ members: [seat(), seat()], chair: seat(), humanGate: true });
    expect(def.nodes.map((n) => String(n.id))).toContain("gate");
    expect(validateGraph(def)).toEqual([]);
  });
});
