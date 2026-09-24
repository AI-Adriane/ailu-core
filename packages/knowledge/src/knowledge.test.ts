import { describe, expect, it } from "vitest";

import {
  buildEdges,
  buildGraph,
  cosineSimilarity,
  InMemoryKnowledgeStore,
  neighbors,
  resolveTargetId,
  type KbDocument
} from "./index.js";

const doc = (id: string, ns: string, type: string, title?: string): KbDocument => ({
  id,
  namespace: ns,
  content: `content of ${id}`,
  type,
  ...(title !== undefined ? { title } : {}),
  createdAt: "2026-01-01T00:00:00Z"
});

describe("@ailu/knowledge — pure ops", () => {
  it("resolveTargetId strips a leading slash and namespaces", () => {
    expect(resolveTargetId("kb", "/a/b.md")).toBe("kb:a/b.md");
    expect(resolveTargetId("kb", "rel.md")).toBe("kb:rel.md");
  });

  it("buildEdges turns links into references + typed relations, deduped", () => {
    const edges = buildEdges(
      "kb",
      "kb:src.md",
      ["/a.md", "a.md"],
      [{ type: "depends-on", target: "/b.md" }]
    );
    expect(edges).toEqual([
      { from: "kb:src.md", to: "kb:a.md", type: "references" },
      { from: "kb:src.md", to: "kb:b.md", type: "depends-on" }
    ]);
  });

  it("buildGraph projects documents to nodes", () => {
    const graph = buildGraph(
      [doc("a", "kb", "note", "Alpha"), doc("b", "kb", "doc")],
      [{ from: "a", to: "b", type: "references" }]
    );
    expect(graph.nodes).toHaveLength(2);
    expect(graph.nodes[0]?.title).toBe("Alpha");
    expect(graph.edges).toHaveLength(1);
  });

  it("neighbors does a depth-limited BFS", () => {
    const graph = {
      nodes: ["a", "b", "c", "d"].map((id) => ({ id, type: "n" })),
      edges: [
        { from: "a", to: "b", type: "r" },
        { from: "b", to: "c", type: "r" },
        { from: "c", to: "d", type: "r" }
      ]
    };
    expect(neighbors(graph, "a", 1).nodes.map((n) => n.id).sort()).toEqual(["a", "b"]);
    expect(neighbors(graph, "a", 2).nodes.map((n) => n.id).sort()).toEqual(["a", "b", "c"]);
  });

  it("cosineSimilarity scores aligned vectors higher", () => {
    expect(cosineSimilarity([1, 0], [1, 0])).toBeCloseTo(1);
    expect(cosineSimilarity([1, 0], [0, 1])).toBeCloseTo(0);
    expect(cosineSimilarity([1, 2], [1])).toBe(0);
  });
});

describe("@ailu/knowledge — InMemoryKnowledgeStore", () => {
  it("ranks search by cosine similarity", async () => {
    const store = new InMemoryKnowledgeStore();
    await store.putDocument(doc("refunds", "kb", "note", "Refunds"), [1, 0]);
    await store.putDocument(doc("weather", "kb", "note", "Weather"), [0, 1]);
    const hits = await store.search("kb", [1, 0.1], 2);
    expect(hits[0]?.id).toBe("refunds");
    expect(hits[0]!.score).toBeGreaterThan(hits[1]!.score);
  });

  it("builds the graph and depth-limited neighbors from stored docs + relations", async () => {
    const store = new InMemoryKnowledgeStore();
    await store.putDocument(doc("kb:a.md", "kb", "note", "A"), [1]);
    await store.putDocument(doc("kb:b.md", "kb", "note", "B"), [1]);
    await store.setRelations("kb", "kb:a.md", buildEdges("kb", "kb:a.md", ["/b.md"], []));

    const graph = await store.graph("kb");
    expect(graph.nodes).toHaveLength(2);
    expect(graph.edges).toEqual([{ from: "kb:a.md", to: "kb:b.md", type: "references" }]);

    const nbrs = await store.neighbors("kb", "kb:a.md", 1);
    expect(nbrs.nodes).toHaveLength(2);
  });

  it("setRelations replaces a document's prior outgoing edges", async () => {
    const store = new InMemoryKnowledgeStore();
    await store.setRelations("kb", "x", buildEdges("kb", "x", ["/a.md"], []));
    await store.setRelations("kb", "x", buildEdges("kb", "x", ["/b.md"], []));
    const edges = await store.listRelations("kb");
    expect(edges).toEqual([{ from: "x", to: "kb:b.md", type: "references" }]);
  });
});
