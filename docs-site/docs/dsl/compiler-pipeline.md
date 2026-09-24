---
sidebar_position: 2
title: The compiler pipeline
description: parser → ast → validator → transformer → compiler, shared by both Ailu DSLs.
---

# The compiler pipeline

Ailu has **two DSL compilers** that follow the **same pipeline**:

- `graph-ailu` — the **graph** DSL → `GraphDefinition`.
- `lang-ailu` — the **prompt / agent / chain** DSL.

## `graph-ailu` (graph YAML → `GraphDefinition`)

```text
YAML
  → yaml.load          (raw parse)
  → buildGraphAST      (parser/  → AST)
  → validateGraphAST   (validator/ → Diagnostic[])    ── on any error: stop, result = undefined
  → transformGraph     (transformer/ → GraphDefinition)
```

`compileGraphFile(content, file)` returns `{ result?: GraphDefinition; diagnostics: Diagnostic[] }`.
If any diagnostic has severity `"error"`, `result` is `undefined`.

## `lang-ailu` (prompt / agent / chain)

```text
YAML
  → parseYaml          (parser/)
  → detectKind         ("prompt" | "agent" | "chain") — via _kind, or the presence of `template` / `steps`
  → build{Prompt|Agent|Chain}AST       (parser/)
  → validate{…}AST                     (validator/ → Diagnostic[])
  → transform{…}                       (transformer/)
```

`compileFile(content, file)` returns
`{ result?: PromptTemplate | AgentConfig | ChainDefinition; diagnostics: Diagnostic[] }`.

## Where it runs

The pipeline is also available natively on the Rust side via `compileGraphYamlJson` (napi) and
`compile_graph_yaml` (Python). The SDK and the CLI use that native path when the addon is
present, and the equivalent TypeScript pipeline otherwise — same stages, same diagnostics.

## See also

- [The Ailu DSL](./graph-yaml-syntax)
- [Architecture → the native bridge](/docs/architecture/napi-bridge)
