---
sidebar_position: 14
title: Glossary
description: One-line definitions of Ailu's core terms, each linked to the page that covers it.
---

# Glossary

One-line definitions with a deep link to where each term is explained in full.

- **Channel** — a named, typed slot of graph state with its own default and reducer; the graph's state type is inferred from its channels. See [Channels and reducers](/docs/core-concepts/channels-and-reducers).

- **Reducer** — the rule for merging a node's output into a channel: `"replace"` (default), `"append"`, or `"merge"`. See [Channels and reducers](/docs/core-concepts/channels-and-reducers).

- **Named predicate / condition** — a routing test registered by name and run by the engine; conditions are **never** `eval`'d strings, which keeps routing inspectable and deterministic. See [The execution contract](/docs/core-concepts/execution-contract).

- **Attestation** — the Ed25519 signature recorded when an approval decision is made, binding the decision to who made it. See [Tool approval and attestation](/docs/governance/tool-approval-and-attestation).

- **Principal** — the identity acting in a decision; an agent requests approval under one principal and a human grants it under a different one — the engine forbids self-approval. See [Approval gates](/docs/governance/approval-gates).

- **Seam** — the napi boundary where TypeScript node handlers and tools cross into the Rust engine (`on_node`); the engine awaits the returned promise. See [The native bridge](/docs/architecture/napi-bridge).

- **Fan-out / `send`** — parallel branching from one node into many. See [Runtime and engine](/docs/core-concepts/runtime-and-engine).

  :::warning Reserved, not implemented
  The `NodeDefinition.fanOut` slot exists in the schema but is **not implemented** in the runtime yet — don't rely on it.
  :::

- **Checkpoint** — the full typed state plus position in the graph, written after every node completion and state mutation; this is what makes `resume` exact. See [The execution contract](/docs/core-concepts/execution-contract).

- **Tier** — a capability level (`"frontier" | "balanced" | "fast" | "creative"`) an agent node declares instead of a hardcoded model; the engine resolves it against available providers, and an explicit `model` always wins. See [Agent nodes & ReAct](/docs/building/agent-nodes-and-react).

- **`AgentResult`** — the object an agent node writes to its output channel: `{ artifacts, blockers, approvalRequests, confidence, reasoning, requiresHumanReview }`; route on `confidence` or `requiresHumanReview`. See [Agent nodes & ReAct](/docs/building/agent-nodes-and-react).

- **Human gate** — a `human-gate` node that suspends the run (`run_suspended`) for an out-of-band human decision, then resumes from the latest checkpoint. See [Resumability and approvals](/docs/core-concepts/resumability-and-approvals).

- **Approval gate** — the governance loop where a sensitive tool call pauses for a human (a distinct principal) to approve before execution continues. See [Approval gates](/docs/governance/approval-gates).

- **Recursion limit** — `recursionLimit` bounds steps per run so cyclic graphs (e.g. an agent loop) can't spin forever; exceeding it raises `RecursionLimitError`. See [The execution contract](/docs/core-concepts/execution-contract).
