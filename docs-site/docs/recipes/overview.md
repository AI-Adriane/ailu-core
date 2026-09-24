---
sidebar_position: 0
title: Cookbook
---

# Cookbook

Explore hands-on recipes for building with Ailu — from your first agent suspension to deep multi-agent orchestration, streaming, retrieval, and governance. Each recipe is a standalone, runnable example paired with explanations of the trade-offs and design patterns.

## Governance

Build approval gates and attestation into your graphs. Suspend for human review, approve, and resume — with no self-approval, full audit trails, and durable checkpoints.

- [Governed refund agent](./governed-refund-agent) — Beginner · A billing agent reaches for a gated refund tool, the run suspends, a human approves, and it executes.
- [Secrets redaction & no-log channels](./secrets-and-no-log) — Intermediate · API keys and sensitive data are scrubbed from requests automatically; mark channels to keep values out of logs.
- [Idea-to-ship pipeline](./idea-to-ship-pipeline) — Advanced · A full venture pipeline chains agents, a structural human gate, and a native agent suspension through an ApprovalEngine.
- [RAG question answerer](./rag-question-answerer) — Intermediate · A retrieval QA flow with governance twist: uncited answers route to a human gate instead of publishing.

## Agents & Deep Agents

Build single and multi-agent systems with ReAct, planning, criticism, and progressive skill disclosure. The agent loop is built into `agentNode` — pass tools and it runs the reason → call → observe → repeat cycle.

- [ReAct agent with planner & critic](./react-planner-critic) — Intermediate · An optional planner flows through the agent loop and optional critic, comparing Ailu's built-in patterns to LangGraph.
- [Governed skills for a deep agent](./governed-skills) — Advanced · Give a deep agent playbooks it loads progressively, withheld until granted when they grant capability.
- [Parallel fan-out (mapAgents)](./parallel-fan-out) — Intermediate · Run the same agent concurrently over every item in an array channel, then join the per-item results in input order.
- [Hierarchical delegation (fanOut)](./hierarchical-delegation) — Intermediate · A manager agent dispatches to different, heterogeneous specialist nodes concurrently, then joins at a report node.
- [Governed council for a high-stakes decision](./governed-council-decision) — Advanced · N member agents answer independently, reviewers rank the anonymized field, and a chair synthesizes — with an optional human gate before the verdict.

## Retrieval & Knowledge

Ground agents in documents and knowledge bases. Build RAG pipelines with citation-aware routing and governance seams that prevent hallucination.

- [RAG question answerer](./rag-question-answerer) — Intermediate · A retrieval QA flow with governance twist: uncited answers route to a human gate instead of publishing.

## Streaming & State

Stream tokens and lifecycle events live. Build long-term agent memory, durable checkpoints, and cross-process resumption.

- [Per-token streaming](./token-streaming) — Intermediate · Stream an agent's generation token by token as it runs for a live typing UI, byte-identical to non-streaming runs.
- [Stream to a governance dashboard](./stream-to-dashboard) — Intermediate · Subscribe to a run's lifecycle events and relay them over SSE to a live governance dashboard.
- [Resume across processes](./resume-across-processes) — Advanced · Suspend a governed run in one process, persist the checkpoint, and resume it in another by implementing the Checkpointer interface.
- [Long-term agent memory](./agent-memory) — Intermediate · Give an agent governed long-term memory: it recalls relevant past context before a run and persists what it learns.

## Ops & Deploy

Observe, debug, and operate graphs in production. Durable checkpointing, inspector dashboards, and controlled secret redaction.

- [Watch a run in the inspector](./dev-inspector) — Intermediate · Run a graph and watch it execute in the browser with node-by-node timeline, event stream, and governance lens.
- [Stream to a governance dashboard](./stream-to-dashboard) — Intermediate · Subscribe to a run's lifecycle events and relay them over SSE to a live governance dashboard.
- [Secrets redaction & no-log channels](./secrets-and-no-log) — Intermediate · API keys and sensitive data are scrubbed from requests automatically; mark channels to keep values out of logs.

## DSL & Authoring

Author graphs with code or YAML. Both paths compile to the same `GraphDefinition` wire format.

- [YAML and the builder](./yaml-and-builder) — Beginner · Author the same governed graph two ways: as Ailu DSL YAML and as TypeScript builder, both compile to the same definition.

## Models

Choose, configure, and constrain models. Stream tokens, add structured output, send images and files.

- [Choosing a model](./model-packages) — Beginner · One import, one surface: choose models by provider and tier, with zero-config fallback and environment key resolution.
- [Structured output](./structured-output) — Intermediate · Constrain an agent to a JSON Schema, validated in-engine, with a bounded retry and fail-closed or lenient mode.
- [Multimodal input](./multimodal-input) — Beginner · Send an agent image, audio, or file content alongside text, fanned out per provider's wire format.
- [Per-token streaming](./token-streaming) — Intermediate · Stream an agent's generation token by token as it runs for a live typing UI, byte-identical to non-streaming runs.
