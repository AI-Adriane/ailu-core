---
sidebar_position: 1
title: Tool registries overview
description: The ToolRegistry interface (register / resolve / list) an agent's tool calls dispatch through — the in-engine InMemoryToolRegistry default, and how a product built on Ailu (e.g. an MCP-backed registry) implements the same seam.
---

# Tool registries overview

An agent's `tools` (whether passed to `agentNode`/`toolNode` in the fluent SDK, or bound at run
start on the catalog path) resolve through one small interface: `ToolRegistry`. It is the seam
between "an agent decided to call a tool" and "code actually runs" — register a definition +
handler pair, and the ReAct loop's `resolve`/`list` calls are all it ever needs.

```ts
export interface ToolRegistry {
  register<TInput, TOutput>(
    definition: ToolDefinition<TInput, TOutput>,
    handler: ToolHandler<TInput, TOutput>
  ): void;
  resolve(id: ToolId): { definition: ToolDefinition<unknown, unknown>; handler: ToolHandler<unknown, unknown> } | undefined;
  list(): ToolDefinition<unknown, unknown>[];
}
```

`ToolDefinition` carries the tool's `id`/`name`/`description`, a `jsonSchema` (advertised to the
LLM so it can emit calls), Zod `inputSchema`/`outputSchema` for validation, and an optional
`requiresApproval` flag — a `requiresApproval` tool always passes through the intrinsic approval
gate (see [Approval gates](/docs/governance/approval-gates)) regardless of which registry served it.

## The in-engine default: `InMemoryToolRegistry`

```ts
import { InMemoryToolRegistry, createGraph } from "@ailu-ai/graph-sdk";

const tools = new InMemoryToolRegistry();
tools.register(
  {
    id: "search" as never,
    name: "search",
    description: "Search the knowledge base.",
    inputSchema: { parse: (v) => v as { query: string } },
    outputSchema: { parse: (v) => v as { results: string[] } },
    permissions: [],
    jsonSchema: { type: "object", properties: { query: { type: "string" } } }
  },
  async (input) => ({ results: [`(stub) results for ${input.query}`] })
);

const app = createGraph({ name: "researcher" })
  .agentNode("worker", { llm, prompt: { system: "Answer using search." }, tools })
  .compile();
```

`InMemoryToolRegistry` is a flat in-process map (`register`/`resolve`/`list` over a `Map<ToolId,
Entry>`) — the same shape every other registry implements. It has no external dependency and no
persistence; tools registered on it exist only for the process lifetime.

## Building your own registry over an external tool source

Because `ToolRegistry` is just an interface, a product built on Ailu can implement it over any
external tool source and hand the result to `agentNode`/`toolNode` exactly like
`InMemoryToolRegistry` — the ReAct loop never knows the difference. The shape that comes up most:
a registry backed by a remote **MCP** (Model Context Protocol) server, so an agent can call a
third-party tool it discovers at run time rather than one hardcoded at authoring time.

A real example (private product repo, not part of this OSS engine — shown here as the reference
shape): `McpToolRegistry` connects to an MCP server, lists its tools, and registers each one with
`requiresApproval: true` unconditionally — an MCP server is code the platform doesn't control and
can't statically vet, so every tool it contributes is gated by construction:

```ts
class McpToolRegistry implements ToolRegistry {
  static async connect(url: string): Promise<McpToolRegistry> { /* connect + list, once */ }
  register<TInput, TOutput>(definition: ToolDefinition<TInput, TOutput>, handler: ToolHandler<TInput, TOutput>): void { /* … */ }
  resolve(id: ToolId) { /* … */ }
  list() { /* … */ }
  // every tool discovered from the MCP server is registered with requiresApproval: true
}
```

The pattern generalizes to any tool source you'd bind to an agent at run time (an internal tool
API, a plugin marketplace, a per-tenant catalog): implement `register`/`resolve`/`list`, decide
your own `requiresApproval` policy per tool, and pass the instance where `InMemoryToolRegistry`
would otherwise go.

## Where it sits among the seams

| Capability | Placement | Status |
| --- | --- | --- |
| In-process tool registration | `InMemoryToolRegistry` (`@ailu-ai/agents-core`) | Shipped |
| External tool source (e.g. MCP) | Product/control-plane-side `ToolRegistry` implementation | Pattern established; MCP-backed example ships in the private product repo |

The engine ships the **interface and the gate**, not a specific external tool-source client — the
same "engine ships the seam, you (or the product layer) bring the source" split as
[Backends](/docs/integrations/backends/overview) and [Sandboxes](/docs/integrations/sandboxes/overview).

## See also

- [Approval gates](/docs/governance/approval-gates) — how a `requiresApproval` tool is gated regardless of registry.
- [Middleware overview](/docs/integrations/middleware/overview) — the other agent-configurable layer.
