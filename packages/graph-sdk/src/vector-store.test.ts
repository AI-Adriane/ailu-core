import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { cosineSimilarity, createVectorStore } from "./vector-store.js";

describe("@ailu-ai/graph-sdk — cosineSimilarity", () => {
  it("scores identical vectors at 1 and orthogonal vectors at 0", () => {
    expect(cosineSimilarity([1, 0], [1, 0])).toBe(1);
    expect(cosineSimilarity([1, 0], [0, 1])).toBe(0);
  });

  it("returns 0 for a zero-magnitude vector rather than NaN", () => {
    expect(cosineSimilarity([0, 0], [1, 1])).toBe(0);
  });
});

describe("@ailu-ai/graph-sdk — createVectorStore (in-memory)", () => {
  it("upsert + query ranks by cosine similarity, descending", () => {
    const store = createVectorStore();
    store.upsert([
      { id: "near", content: "near doc", embedding: [1, 0, 0] },
      { id: "mid", content: "mid doc", embedding: [1, 1, 0] },
      { id: "far", content: "far doc", embedding: [0, 0, 1] }
    ]);
    expect(store.size()).toBe(3);

    const hits = store.query([1, 0, 0], 3);
    expect(hits.map((h) => h.id)).toEqual(["near", "mid", "far"]);
    expect(hits[0]!.score).toBeCloseTo(1, 10);
    expect(hits[0]!.content).toBe("near doc");
    // Descending order holds across the whole list.
    expect(hits[0]!.score).toBeGreaterThan(hits[1]!.score);
    expect(hits[1]!.score).toBeGreaterThan(hits[2]!.score);
  });

  it("honours k and last-write-wins upsert by id, carrying metadata through", () => {
    const store = createVectorStore();
    store.upsert([{ id: "a", content: "first", embedding: [1, 0], metadata: { v: 1 } }]);
    store.upsert([{ id: "a", content: "second", embedding: [1, 0], metadata: { v: 2 } }]);
    expect(store.size()).toBe(1);

    const hits = store.query([1, 0], 5);
    expect(hits).toHaveLength(1);
    expect(hits[0]!.content).toBe("second");
    expect(hits[0]!.metadata).toEqual({ v: 2 });

    const top1 = store.query([1, 0], 1);
    expect(top1).toHaveLength(1);
  });
});

describe("@ailu-ai/graph-sdk — createVectorStore (file persistence)", () => {
  const dirs: string[] = [];
  const tempPath = (): string => {
    const dir = mkdtempSync(join(tmpdir(), "ailu-vstore-"));
    dirs.push(dir);
    return join(dir, "store.json");
  };

  afterEach(() => {
    for (const dir of dirs.splice(0)) {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("round-trips: write to a temp path, reload into a fresh store, query", () => {
    const path = tempPath();

    const writer = createVectorStore({ persistPath: path });
    writer.upsert([
      { id: "x", content: "alpha", embedding: [1, 0, 0], metadata: { tag: "a" } },
      { id: "y", content: "beta", embedding: [0, 1, 0] }
    ]);
    expect(existsSync(path)).toBe(true);
    // The file is valid, round-trippable JSON.
    expect(JSON.parse(readFileSync(path, "utf8")) as unknown).toBeInstanceOf(Array);

    // A brand-new store reading the same file sees the persisted items.
    const reloaded = createVectorStore({ persistPath: path });
    expect(reloaded.size()).toBe(2);
    const hits = reloaded.query([1, 0, 0], 1);
    expect(hits).toHaveLength(1);
    expect(hits[0]!.id).toBe("x");
    expect(hits[0]!.content).toBe("alpha");
    expect(hits[0]!.metadata).toEqual({ tag: "a" });
  });

  it("creates the parent directory and starts empty for a missing file", () => {
    const dir = mkdtempSync(join(tmpdir(), "ailu-vstore-"));
    dirs.push(dir);
    const path = join(dir, "nested", "deep", "store.json");

    const store = createVectorStore({ persistPath: path });
    expect(store.size()).toBe(0);
    store.upsert([{ id: "z", content: "gamma", embedding: [1, 1] }]);
    expect(existsSync(path)).toBe(true);
    expect(createVectorStore({ persistPath: path }).size()).toBe(1);
  });
});
