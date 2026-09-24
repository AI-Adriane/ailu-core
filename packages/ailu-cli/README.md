# @ailu-ai/cli

The Ailu command-line — author, validate, compile and run [Ailu
DSL](https://github.com/AI-Adriane/ailu-core) graphs from your terminal. It ships as a
self-contained bundle (the engine is inlined), so the `ailu` command works the
moment it's installed.

## Install

```bash
npm i -g @ailu-ai/cli      # or: pnpm add -g @ailu-ai/cli
ailu --help
```

> **Naming.** The npm package is `@ailu-ai/cli`; the installed command is
> `ailu`. (The TypeScript SDK is `@ailu-ai/graph-sdk`; the Python SDK is
> `pip install ailu` / `import ailu`.)

## Commands

| Command | What it does |
| --- | --- |
| `ailu validate <file>` | Validate an Ailu DSL document; non-zero exit on errors. |
| `ailu compile <file> --out <dir>` | Compile DSL YAML into a `GraphDefinition` JSON. |
| `ailu run <file> [--watch]` | Run a graph locally; `--watch` re-runs on change. |
| `ailu publish <file>` | Package a graph for publishing. |
| `ailu diff <left> <right>` | Diff two graph definitions. |
| `ailu init <kind>` | Scaffold a new graph/agent/prompt document. |

```bash
ailu init graph --out ./my-graph.graph.yaml
ailu validate ./my-graph.graph.yaml
ailu compile ./my-graph.graph.yaml --out ./dist
ailu run ./my-graph.graph.yaml --watch
```

## License

Apache-2.0.
