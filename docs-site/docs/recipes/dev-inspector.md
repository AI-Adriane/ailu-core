---
sidebar_position: 14
title: Watch a run in the inspector (ailu dev)
description: Run a graph and watch it execute in the browser — node-by-node timeline, the event stream, and a governance lens that shows exactly where it suspended, with one-click resume.
tags: ["ops", "governance"]
difficulty: intermediate
---

# Watch a run in the inspector

`serveInspector` runs a graph and serves a live, dependency-free **web inspector** — the
"watch your graph think" view. A node-by-node timeline, the lifecycle-event stream, each node's
output, and a **governance lens** that marks exactly where the run suspended (a human gate / an
approval) and *why* — with `explain()` and one-click resume.

```ts
import { createGraph, serveInspector } from "@ailu/graph-sdk";

const app = createGraph({ name: "publish-flow" })
  .node("write", async () => ({ draft: "Hello from Ailu." }))
  .humanGate("review")
  .node("publish", async () => ({ published: true }))
  .edge("write", "review")
  .edge("review", "publish")
  .compile();

const inspector = await serveInspector(app, {});
console.log(`inspecting at ${inspector.url}`); // open it in a browser
await inspector.done;                           // resolves when the run settles (here: suspended)
// … click "Resume" in the page, or POST /resume, to continue past the gate …
// await inspector.close();
```

Open the URL: the left pane is the **node timeline** (each node turns green on completion and shows
its output; a suspended node is highlighted as a gate); the right pane is the **event log**. When
the run suspends, an `explain()` panel shows the reason and the **exact next action** to resume.

## What it reuses (and what's next)

It's built on what the engine already gives you — the lifecycle-event stream (`onEvent`), the
per-node checkpoints, and `explain(runId)` — served over a tiny `node:http` + SSE bridge with an
**inline** page (no framework, no build step, no CDN; binds to `127.0.0.1` only).

This v1 streams a single run **live**. True **time-travel** — rewind to an arbitrary checkpoint and
replay — is the next step: the runtime already checkpoints after every node, so the remaining work
is a fork/replay control through the native bridge.

> A first-class `ailu dev <graph.ts>` CLI command wraps this; today, call `serveInspector(app, data)`
> from a small script.
