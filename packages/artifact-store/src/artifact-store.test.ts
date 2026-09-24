import { describe, expect, it } from "vitest";
import type { NodeId, RunId } from "@ailu/graph-core";

import { InMemoryArtifactStore } from "./in-memory-artifact-store.js";

const runId = "run-1" as RunId;
const nodeId = "node-1" as NodeId;

describe("InMemoryArtifactStore", () => {
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
