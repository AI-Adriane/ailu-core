---
sidebar_position: 1
title: Open Knowledge Format (OKF)
description: A dependency-free markdown-with-frontmatter format for knowledge documents.
---

# Open Knowledge Format (OKF)

OKF is the file format behind Ailu's knowledge base: **markdown with a shallow YAML
frontmatter**. `@ailu-ai/okf` is a dependency-free parser/serializer for it — and
`ailu-okf` is a byte-compatible Rust port, so every SDK shares one definition.

```markdown
---
type: note
title: Checkpointing
description: how it works
tags:
  - runtime
  - determinism
relations:
  - depends-on:/runtime/determinism.md
---
Checkpointing persists state after every node. See [gates](/runtime/gates.md).
```

## Parsing

`parseOkfDocument(raw)` returns a `ParsedOkf`:

```ts
import { parseOkfDocument } from "@ailu-ai/okf";

const doc = parseOkfDocument(raw);
doc.type;        // "note"  (OKF's only required field; defaults to "document")
doc.tags;        // ["runtime", "determinism"]
doc.links;       // ["/runtime/gates.md"]  — markdown cross-refs (the untyped graph edges)
doc.relations;   // [{ type: "depends-on", target: "/runtime/determinism.md" }]  (typed edges)
doc.frontmatter; // any unknown frontmatter keys, preserved for lossless round-trip
doc.body;        // the markdown body (frontmatter stripped, trimmed)
```

- **`links`** are bundle-relative / relative markdown links scanned from the body (external
  `http(s)` links are skipped) merged with any frontmatter `links` — the *untyped* edges.
- **`relations`** is a frontmatter convention: a string list of `"<type>:<target>"`, parsed
  into *typed* edges. This is what makes the knowledge graph a typed property graph.

## Serializing

`serializeOkfDocument(doc)` writes a stored document back to an OKF file (frontmatter +
body), quoting scalars only when YAML needs it. It round-trips: parse → serialize → parse
yields the same fields, including unknown frontmatter keys.

## Helpers

| Function | Purpose |
| --- | --- |
| `extractLinks(body)` | relative markdown links from a body (linear scan, no regex) |
| `isReservedOkfFile(path)` | OKF reserves `index.md` (navigation) and `log.md` (history) |
| `buildOkfIndex(namespace, entries)` | generate an OKF `index.md` navigation listing |
