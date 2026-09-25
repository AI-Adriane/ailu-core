---
title: "YAML and the CLI"
description: "Describe a graph's shape in YAML, validate and diff it with the ailu CLI, and compile it to a graph definition."
---

# YAML and the CLI

A YAML graph describes a graph's **shape**: its channels, nodes and edges. Keep one in your
repository when the shape itself is worth reviewing and diffing, for example a process that
compliance signs off.

YAML holds structure only. It carries no agent configuration (model, prompt, tools), no
functions, and no fan-out, error edges or subgraph mappings. To give nodes behavior, build the
graph with the TypeScript builder; the builder and YAML produce the same `GraphDefinition`.

## The format

```yaml
id: triage              # required
version: 1.0.0          # required
name: Ticket triage     # required
entryNodeId: intake     # required: where runs start
recursionLimit: 25      # optional: required for graphs with cycles
channels:
  ticket: { type: string, reducer: replace, default: "" }
  log: { type: "string[]", reducer: append, default: [] }
nodes:
  - { id: intake, type: action, label: Intake }
  - { id: review, type: human-gate, label: Human review }
edges:
  - { id: e1, from: intake, to: review, type: default }
```

- Node `type`: `action`, `agent`, `tool`, `human-gate` or `subgraph`.
- Edge `type`: `default` or `conditional` (with `condition: <predicate name>`).
- Channel `reducer`: `replace`, `append` or `merge`. See [Graphs](./graphs.md#combine-updates-with-reducers).

## Compile from code

```ts file=packages/graph-sdk/examples/docs/yaml-compile.ts region=example
```

## The CLI

```bash
npm install -g @ailu-ai/cli
```

| Command | What it does |
| --- | --- |
| `ailu init graph --id triage --out triage.graph.yaml` | Writes a starter file. |
| `ailu validate triage.graph.yaml` | Checks the file. Exits with 1 and lists the problems if it is invalid. |
| `ailu compile triage.graph.yaml --out build/` | Writes the compiled `GraphDefinition` to `build/triage.graph.json`. |
| `ailu diff old.graph.yaml new.graph.yaml` | Lists the nodes and edges added or removed between two versions. |
| `ailu run triage.graph.yaml --input '{}'` | A dry run: walks the graph and prints each event. Nodes do nothing. |

`ailu validate` fits well in CI, next to your tests. To really run a graph, run your TypeScript
code: `npx tsx app.ts`.
