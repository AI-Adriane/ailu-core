---
slug: /
title: "What is Ailu"
description: "Ailu runs AI agents as graphs that stop for a human before they act, keep a checkpoint after every step, and can be replayed as evidence."
---

import SdkVersion from "@site/src/components/SdkVersion";

# What is Ailu

Ailu is a TypeScript SDK for building AI agent workflows as **graphs**. A graph is a set of steps
(nodes) connected by edges. Some steps are your code, some are LLM agents, and some are
**human approval gates**: the run stops there until a person says yes.

Three things set Ailu apart:

- **Governed.** An agent can be made to stop before it calls a sensitive tool (a refund, a
  payment, a deletion). A named human approves, and only then does the tool run. Decisions can be
  signed and checked later.
- **Resumable.** The engine saves a checkpoint after every step. A run that stops for a human, a
  timer or an external event picks up exactly where it stopped.
- **Replayable.** A run can record every model call. Later, anyone can replay it from the record,
  without calling a model, and check that it reaches the same result.

The engine is written in Rust and ships as a prebuilt native addon, so you only install an npm
package. These docs describe `@ailu-ai/graph-sdk` <SdkVersion />.

## When to use Ailu

Use Ailu when an agent's actions matter: it moves money, changes customer data, sends messages,
or must leave an audit trail. Ailu is also a good fit for long workflows that wait for people or
external systems.

For a one-off chat completion, you don't need a graph: call a model directly with
[`model.invoke()`](./guides/agents.md#call-a-model-directly).

## What's stable

| Area | Status |
| --- | --- |
| TypeScript SDK: graphs, agents, tools, human gates, streaming, sub-agents | Stable |
| Governance: tool approval, attestation, replay, secret redaction | Stable |
| Resume in another process (`runCatalogGraph` / `resumeCatalogGraph`) | Stable |
| Python SDK | Partial: validate and compile graphs, run components and prebuilt agents. No graph runs yet. |
| YAML graphs and the `ailu` CLI | Graph shape only: validate, inspect, compile |
| C ABI and other language bindings | Experimental |

## Next steps

1. [Install](./install.md) the SDK.
2. Run the [quickstart](./quickstart.md): an agent, a human gate, and a resume, in one file.
3. Pick a [guide](./guides/graphs.md) for the task at hand.

Building with an AI coding agent? Point it at [`/llms.txt`](pathname:///llms.txt) and
[For AI agents](./reference/for-ai-agents.md).
