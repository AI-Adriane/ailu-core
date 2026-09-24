---
sidebar_position: 9
title: MCP server
description: Expose Ailu over the Model Context Protocol — run agents and graphs as MCP tools, and read a knowledge base as MCP resources.
---

# MCP server

`@ailu/plugin-mcp` exposes Ailu over the **Model Context Protocol (MCP)** on a stdio
transport, so any MCP client — Claude Desktop, an IDE, another agent — can drive the engine and
read its knowledge through the open standard. Execution runs on the Rust engine.

## Tools

| Tool | What it does |
| --- | --- |
| `list_agents` | List the prebuilt agents available to run. |
| `run_agent` | Run a prebuilt agent to completion (or until it suspends on an approval gate). |
| `approve_and_resume` | Resolve a gated tool for a suspended run and resume it (a different principal — never self-approval). |
| `run_graph` | Run a graph definition. |
| `validate_graph` | Validate a graph definition without executing it. |
| `compile_graph_yaml` | Compile Ailu graph YAML into a `GraphDefinition`. |

A run that hits a sensitive tool **suspends** rather than acting; `approve_and_resume` is how a
human resolves it — the governance contract holds over MCP exactly as it does in-process.

## Resources — a knowledge base over MCP

The server also serves a persistent knowledge base as MCP **resources** (`resources/list` +
`resources/read`), so an MCP client can read an organization's knowledge through the standard:

- Each document is a resource `ailu-kb://<namespace>/<id>` with `mimeType: text/markdown`.
- The namespace is set by `AILU_MCP_KB_NAMESPACE` (default `ailu`).
- The server holds no database — it reads the knowledge base over the control-plane HTTP API.
  Absent a reachable knowledge-base API it simply lists nothing, so it is inert in a pure OSS
  setup and lights up when a knowledge base is present.

This is the symmetric half of the inbound MCP source connector: knowledge flows both ways across
the open standard.

## Wiring into an MCP client

Register the server on stdio. For Claude Desktop (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "ailu": {
      "command": "node",
      "args": ["--import", "tsx", "/path/to/plugin/mcp/server.mts"],
      "env": { "AILU_SDK_ENGINE": "rust", "AILU_MCP_KB_NAMESPACE": "ailu" }
    }
  }
}
```

The client then lists Ailu's tools and resources on connect, calls tools (`run_agent`,
`run_graph`, …), and reads knowledge-base resources — all over MCP.

## See also

- [Tools and tool nodes](/docs/building/tools-and-tool-nodes) — how tools (and their approval gates) are defined.
- [Providers & BYOM](/docs/building/providers) — which model serves the agents the MCP server runs.
