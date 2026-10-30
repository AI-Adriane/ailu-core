import { describe, expect, it } from "vitest";
import type { NodeId, RunId } from "@ailu-ai/graph-core";

import { InMemoryArtifactStore } from "./in-memory-artifact-store.js";

const runId = "run-1" as RunId;
const nodeId = "node-1" as NodeId;

describe("InMemoryArtifactStore", () => {
  it.each([
    ["run-1", "analysis", "run-1:analysis"],
    ["a:b", "c", "a%3Ab:c"],
    ["a", "b:c", "a:b%3Ac"],
    ["a%3Ab", "c", "a%253Ab:c"],
    ["", "", ":"],
    ["é:💡", "%:文", "é%3A💡:%25%3A文"]
  ])("matches the Rust wire fixture for (%j, %j)", async (run, name, expected) => {
    const artifact = await new InMemoryArtifactStore().write({
      runId: run as RunId,
      nodeId,
      name,
      mediaType: "text/plain",
      content: "fixture"
    });
    expect(artifact.id).toBe(expected);
  });

  it("assigns distinct IDs to generated run/name pairs", async () => {
    const alphabet = ["a", ":", "%", "3", "é", "💡", "\0"];
    const components = ["", ...alphabet, ...alphabet.flatMap((a) => alphabet.map((b) => a + b))];
    const store = new InMemoryArtifactStore();
    const ids = new Set<string>();
    for (const run of components) {
      for (const name of components) {
        const artifact = await store.write({
          runId: run as RunId,
          nodeId,
          name,
          mediaType: "text/plain",
          content: "test"
        });
        expect(artifact.version).toBe(1);
        expect(ids.has(artifact.id)).toBe(false);
        ids.add(artifact.id);
      }
    }
    expect(ids.size).toBe(components.length ** 2);
  });

  it.each([
    [
      ["a:b", "c"],
      ["a", "b:c"]
    ],
    [
      ["a:", ""],
      ["a", ":"]
    ],
    [
      ["a:b", "c"],
      ["a%3Ab", "c"]
    ],
    [
      ["a", "b:c"],
      ["a", "b%3Ac"]
    ],
    [
      ["é:💡", "%:文"],
      ["é", "💡:%:文"]
    ]
  ] as const)("keeps separate histories for %j and %j", async (leftPair, rightPair) => {
    const store = new InMemoryArtifactStore();
    const leftInput = {
      runId: leftPair[0] as RunId,
      nodeId,
      name: leftPair[1],
      mediaType: "text/plain" as const,
      content: "left"
    };
    const left = await store.write(leftInput);
    const right = await store.write({
      ...leftInput,
      runId: rightPair[0] as RunId,
      name: rightPair[1],
      content: "right"
    });
    expect(left.id).not.toBe(right.id);
    expect(right.version).toBe(1);
    const leftV2 = await store.write({ ...leftInput, content: "left v2" });
    expect(leftV2.id).toBe(left.id);
    expect(leftV2.version).toBe(2);
    expect(await store.read(left.id)).toEqual(leftV2);
    expect(await store.read(right.id)).toEqual(right);
    expect(await store.readVersion(left.id, 1)).toEqual(left);
    expect(await store.readVersion(right.id, 2)).toBeUndefined();
    expect(await store.listVersions(left.id)).toEqual([left, leftV2]);
    expect(await store.listVersions(right.id)).toEqual([right]);
    for (const run of [leftPair[0], rightPair[0]]) {
      expect(
        (await store.listByRun(run as RunId)).every((artifact) => artifact.runId === run)
      ).toBe(true);
    }
  });

  it("write creates an artifact with version 1", async () => {
    const store = new InMemoryArtifactStore();

    const artifact = await store.write({
      runId,
      nodeId,
      name: "analysis",
      mediaType: "application/json",
      content: { score: 1 }
    });

    expect(artifact.version).toBe(1);
    expect(artifact.name).toBe("analysis");
  });

  it("write with same runId + name increments version to 2", async () => {
    const store = new InMemoryArtifactStore();

    const first = await store.write({
      runId,
      nodeId,
      name: "analysis",
      mediaType: "application/json",
      content: { score: 1 }
    });
    const second = await store.write({
      runId,
      nodeId,
      name: "analysis",
      mediaType: "application/json",
      content: { score: 2 }
    });

    expect(first.id).toBe(second.id);
    expect(second.version).toBe(2);
  });

  it("read returns the latest artifact version", async () => {
    const store = new InMemoryArtifactStore();
    const first = await store.write({
      runId,
      nodeId,
      name: "report",
      mediaType: "text/markdown",
      content: "# v1"
    });
    await store.write({
      runId,
      nodeId,
      name: "report",
      mediaType: "text/markdown",
      content: "# v2"
    });

    const latest = await store.read(first.id);

    expect(latest?.version).toBe(2);
    expect(latest?.content).toBe("# v2");
  });

  it("readVersion returns the exact requested version", async () => {
    const store = new InMemoryArtifactStore();
    const first = await store.write({
      runId,
      nodeId,
      name: "report",
      mediaType: "text/plain",
      content: "v1"
    });
    await store.write({
      runId,
      nodeId,
      name: "report",
      mediaType: "text/plain",
      content: "v2"
    });

    const v1 = await store.readVersion(first.id, 1);

    expect(v1?.version).toBe(1);
    expect(v1?.content).toBe("v1");
  });

  it("listByRun returns all artifacts for a run", async () => {
    const store = new InMemoryArtifactStore();
    await store.write({
      runId,
      nodeId,
      name: "report",
      mediaType: "text/plain",
      content: "v1"
    });
    await store.write({
      runId,
      nodeId,
      name: "report",
      mediaType: "text/plain",
      content: "v2"
    });
    await store.write({
      runId,
      nodeId,
      name: "raw",
      mediaType: "application/octet-stream",
      content: new Uint8Array([1, 2, 3])
    });

    const artifacts = await store.listByRun(runId);

    expect(artifacts).toHaveLength(3);
  });

  it("listVersions returns all versions for an artifact id", async () => {
    const store = new InMemoryArtifactStore();
    const first = await store.write({
      runId,
      nodeId,
      name: "report",
      mediaType: "text/plain",
      content: "v1"
    });
    await store.write({
      runId,
      nodeId,
      name: "report",
      mediaType: "text/plain",
      content: "v2"
    });

    const versions = await store.listVersions(first.id);

    expect(versions.map((artifact) => artifact.version)).toEqual([1, 2]);
  });
});
