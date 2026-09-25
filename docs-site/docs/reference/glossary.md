---
title: "Glossary"
description: "The words these docs use, in one line each."
---

# Glossary

| Term | Meaning |
| --- | --- |
| **Agent node** | A node that runs an LLM in a loop, with tools. `agentNode`. |
| **Approval request** | A gated tool call waiting for a human, listed in the agent's `approvalRequests`. |
| **Attestation** | A signed record of an approval decision, chained to the previous one. |
| **Catalog runner** | `runCatalogGraph` / `resumeCatalogGraph`: runs a graph from its JSON definition and returns its state, so it can resume anywhere. |
| **Channel** | A named piece of run state, with a type label, a default and a reducer. |
| **Checkpoint** | The saved state of a run after a node. A run resumes from its last checkpoint. |
| **Component** | A ready-made node that runs inside the engine, such as a retriever or a prompt template. |
| **Compiled graph** | What `compile()` returns: a checked graph you can run many times. |
| **Council** | A graph where several agents answer, others rank the anonymized answers, and a chair decides. |
| **Edge** | A link from one node to the next. Plain, conditional (with a named predicate) or error. |
| **Engine** | The Rust runtime that executes graphs, shipped inside the npm package. |
| **Fan-out** | Running several branches at the same time, then joining. |
| **Graph definition** | A graph as plain JSON (`app.definition`), the same format YAML compiles to. |
| **Human gate** | A node that suspends the run until you resume it. |
| **Offline mode** | `AILU_LLM_MOCK=1`: agents without a key answer from a deterministic mock. |
| **Reducer** | How a channel combines updates: replace, append or merge. |
| **Replay** | Re-running a recorded run from its record, without calling a model. |
| **Resolved by** | The person who approved a tool call, recorded with the approval. |
| **Run** | One execution of a graph, with its own `runId` and state. |
| **Subgraph** | A graph used as one node of another graph. |
| **Suspended** | The status of a run waiting for a human, a date or a signal. |
| **Tier** | A capability level (`fast`, `balanced`, `frontier`, `creative`) mapped to a model per provider. |
| **Tool** | A function an agent can call. Gated tools need a human's approval. |
