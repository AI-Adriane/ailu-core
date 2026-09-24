---
sidebar_position: 1
title: Commands
description: The ailu CLI — validate, compile, run, publish, diff, init.
---

# The `ailu` CLI

The `ailu` CLI **validates**, **compiles**, **runs**, **publishes**, **diffs** and
**initializes** Ailu definitions (graphs, agents, prompts). It is published as
`@ailu-ai/cli` and installs the `ailu` command:

```bash
npm i -g @ailu-ai/cli
ailu <command> [arguments] [options]
```

It ships as a self-contained bundle (the engine is inlined), so it runs the moment it's
installed.

:::note File-kind detection
For `validate` and `compile`, a file whose name contains `.graph.` is treated as a **graph**
(compiled via `graph-ailu`); otherwise it's treated as a **prompt / agent / chain** file
(compiled via `lang-ailu`, which detects the subtype). `run` and `diff` operate only on
**graph** files.
:::

## `validate <file>`

Validate an Ailu file and print diagnostics.

```bash
ailu validate ./my-flow.graph.yaml
ailu validate ./my-agent.yaml
```

Compiles the file and formats diagnostics; writes nothing to disk. **Exit code:** `1` if there
is at least one `error`-severity diagnostic, otherwise `0`.

## `compile <file> --out <dir>`

Compile a file to its JSON form and write it to disk.

```bash
ailu compile ./my-flow.graph.yaml --out ./dist
```

| Option | Required | Description |
| --- | --- | --- |
| `--out <dir>` | yes | Output directory (created recursively if absent) |

On a compile error (or no result), prints diagnostics and exits `1` — **nothing is written**.
Otherwise writes `<dir>/<basename>.json` (2-space-indented) and prints `Wrote <file>`.

## `run <file> [--input <json>] [--watch]`

Run a **graph** file and stream execution events in `debug` mode to stdout (one JSON object per
line).

```bash
ailu run ./my-flow.graph.yaml
ailu run ./my-flow.graph.yaml --input '{"name":"Ada"}'
ailu run ./my-flow.graph.yaml --watch
```

| Option | Required | Description |
| --- | --- | --- |
| `--input <json>` | no | Initial run data, as JSON. Default: `{}` |
| `--watch` | no | Re-run on every file change |

:::note
`run` builds an in-memory runtime and registers **each node with a no-op handler** — it
validates and traces the graph's execution flow; it does not execute node business logic. A
compile error is written to **stderr** and the run is skipped. **Exit code:** `0`.
:::

## `publish <file> --registry <url>`

Publish a file's **raw content** to a registry via HTTP `POST` (`content-type:
application/yaml`).

```bash
ailu publish ./my-flow.graph.yaml --registry https://registry.example.com/graphs
```

| Option | Required | Description |
| --- | --- | --- |
| `--registry <url>` | yes | Target registry URL |

On a non-`ok` response, writes `Publish failed: <status>` to stderr and exits `1`; otherwise
prints `Published successfully.`

## `diff <left> <right>`

Diff two **graph** files and print added/removed nodes, edges and channels. A diagnostic tool.

```bash
ailu diff ./v1.graph.yaml ./v2.graph.yaml
ailu diff ./flow.graph.yaml@1.0.0 ./flow.graph.yaml@2.0.0
```

Each argument accepts `<file>@<version>`; the part after `@` is used only as a **label** in the
output header, not to resolve a version. If either graph is invalid, writes `Unable to diff
invalid graph files.` to stderr and exits `1`. Otherwise prints added (`+`) and removed (`-`)
sets for nodes, edges and channels.

## `init <kind> --id <id> --out <file>`

Scaffold a template file for a graph, agent, or prompt.

```bash
ailu init graph  --id my-flow   --out ./my-flow.graph.yaml
ailu init agent  --id my-agent  --out ./my-agent.yaml
ailu init prompt --id my-prompt --out ./my-prompt.yaml
```

| Argument / Option | Required | Description |
| --- | --- | --- |
| `<kind>` | yes | `graph` \| `agent` \| `prompt` |
| `--id <id>` | yes | Identifier injected into the template |
| `--out <file>` | yes | Output path (parent directory created if absent) |

Writes the file and prints `Initialized <kind> template at <file>`. **Exit code:** `0`.

## Exit codes at a glance

| Command | `0` | `1` |
| --- | --- | --- |
| `validate` | no error diagnostics | at least one `error` diagnostic |
| `compile` | wrote the JSON | compile error (nothing written) |
| `run` | always | — (compile errors traced to stderr) |
| `publish` | HTTP `ok` response | request failed |
| `diff` | both graphs valid | a graph is invalid |
| `init` | always | — |

## See also

- [The Ailu DSL](/docs/dsl/graph-yaml-syntax)
- [Architecture overview](/docs/architecture/overview)
