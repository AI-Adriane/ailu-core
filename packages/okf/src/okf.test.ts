import { describe, expect, it } from "vitest";

import {
  buildOkfIndex,
  extractLinks,
  isReservedOkfFile,
  parseOkfDocument,
  serializeOkfDocument
} from "./index.js";

describe("@ailu/okf — parse", () => {
  it("defaults type to 'document' and trims the body when there is no frontmatter", () => {
    const doc = parseOkfDocument("\n# Hello\n\nbody text\n");
    expect(doc.type).toBe("document");
    expect(doc.body).toBe("# Hello\n\nbody text");
    expect(doc.title).toBeUndefined();
  });

  it("reads frontmatter scalars, block lists, and inline lists", () => {
    const raw = [
      "---",
      "type: note",
      "title: Checkpointing",
      "description: how it works",
      "tags:",
      "  - runtime",
      "  - determinism",
      "links: [./a.md, ./b.md]",
      "---",
      "Body."
    ].join("\n");
    const doc = parseOkfDocument(raw);
    expect(doc.type).toBe("note");
    expect(doc.title).toBe("Checkpointing");
    expect(doc.description).toBe("how it works");
    expect(doc.tags).toEqual(["runtime", "determinism"]);
    // Inline frontmatter links are merged with body links (deduped).
    expect(doc.links).toEqual(["./a.md", "./b.md"]);
    expect(doc.body).toBe("Body.");
  });

  it("parses typed `relations` (`<type>:<target>`) into edges", () => {
    const raw = [
      "---",
      "type: note",
      "relations:",
      "  - depends-on:/runtime/checkpointing.md",
      "  - references:/runtime/gates.md",
      "  - malformed",
      "---",
      "x"
    ].join("\n");
    const doc = parseOkfDocument(raw);
    expect(doc.relations).toEqual([
      { type: "depends-on", target: "/runtime/checkpointing.md" },
      { type: "references", target: "/runtime/gates.md" }
    ]);
  });

  it("merges body markdown links and skips external http(s) links", () => {
    const raw = "---\ntype: note\n---\nSee [a](./a.md) and [ext](https://x.com) and [b](../b.md).";
    const doc = parseOkfDocument(raw);
    expect(doc.links).toEqual(["./a.md", "../b.md"]);
  });

  it("preserves unknown frontmatter keys for round-trip", () => {
    const raw = "---\ntype: note\nauthor: alice\n---\nx";
    const doc = parseOkfDocument(raw);
    expect(doc.frontmatter).toEqual({ author: "alice" });
  });
});

describe("@ailu/okf — serialize", () => {
  it("round-trips a document's frontmatter and body", () => {
    const md = serializeOkfDocument({
      type: "note",
      title: "T",
      description: "D",
      tags: ["a", "b"],
      timestamp: "2026-01-01T00:00:00Z",
      frontmatter: { author: "alice" },
      body: "Hello body"
    });
    const reparsed = parseOkfDocument(md);
    expect(reparsed.type).toBe("note");
    expect(reparsed.title).toBe("T");
    expect(reparsed.description).toBe("D");
    expect(reparsed.tags).toEqual(["a", "b"]);
    expect(reparsed.timestamp).toBe("2026-01-01T00:00:00Z");
    expect(reparsed.frontmatter).toEqual({ author: "alice" });
    expect(reparsed.body).toBe("Hello body");
  });

  it("quotes scalars that YAML would otherwise misread", () => {
    const md = serializeOkfDocument({ type: "note", title: "a: b # c", body: "x" });
    expect(md).toContain('title: "a: b # c"');
  });
});

describe("@ailu/okf — helpers", () => {
  it("extractLinks finds relative links only", () => {
    expect(extractLinks("[a](./a.md) [x](http://e.com)")).toEqual(["./a.md"]);
  });

  it("isReservedOkfFile flags index.md and log.md", () => {
    expect(isReservedOkfFile("dir/index.md")).toBe(true);
    expect(isReservedOkfFile("log.md")).toBe(true);
    expect(isReservedOkfFile("notes/topic.md")).toBe(false);
  });

  it("buildOkfIndex lists exported entries", () => {
    const index = buildOkfIndex("kb", [
      { path: "a.md", title: "Alpha", description: "first" },
      { path: "b.md" }
    ]);
    expect(index).toContain("# kb");
    expect(index).toContain("* [Alpha](/a.md) - first");
    expect(index).toContain("* [b.md](/b.md)");
  });
});
