# @ailu-ai/graph-sdk

The front door to the [Ailu](https://github.com/AI-Adriane/ailu-core) framework: build,
compile and run **stateful, resumable agent graphs** — agents, tools, human-approval
gates, artifacts and long-running workflows — without touching the lower-level engine.

Every run is checkpointed after every step: it can stop at a human-approval gate and
resume from where it stopped. To resume in another process, run the graph with
`runCatalogGraph` and keep the state it returns.

## Install

```bash
npm install @ailu-ai/graph-sdk
# or: pnpm add @ailu-ai/graph-sdk   /   yarn add @ailu-ai/graph-sdk
```

Graphs run on Ailu's Rust engine. It ships as a prebuilt native addon (`@ailu-ai/napi`,
installed with the SDK) for macOS (x64, arm64), Linux with glibc (x64, arm64) and Windows x64.
There is no TypeScript fallback: on another platform `compile()` throws
`RustEngineRequiredError`. Check at boot with:

```ts
import { rustEngineAvailable } from "@ailu-ai/graph-sdk";
console.log(rustEngineAvailable()); // true when the native engine loaded
```

## Quickstart

```ts
import { createGraph } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "greeter" })
  .node("hello", async (_input, state) => ({
    greeting: `Hello, ${(state.channels as Record<string, unknown>).name}!`
  }))
  .compile();

const result = await app.run({ name: "Ada" });
console.log(result.channels.greeting); // "Hello, Ada!"
```

Add a human-approval gate and the run **suspends** cleanly, then **resumes** from its
checkpoint:

```ts
const app = createGraph({ name: "publish-flow" })
  .node("write", async () => ({ draft: "Hello from Ailu." }))
  .humanGate("review")
  .node("publish", async () => ({ approved: true }))
  .edge("write", "review")
  .edge("review", "publish")
  .compile();

const suspended = await app.run();              // status: "suspended"
const done = await app.resume(suspended.runId); // status: "completed"
```

An agent node takes a model and a prompt. The API key is read from the environment
(`ANTHROPIC_API_KEY` here); with no key the run fails and says which variable to set.
To run offline on the engine's deterministic mock (tests, CI), set `AILU_LLM_MOCK=1`.

```ts
import { createGraph, model } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "assistant" })
  .agentNode("reply", {
    model: model.anthropic("claude-sonnet-4-6"),
    prompt: { system: "Answer in one sentence." }
  })
  .compile();

const out = await app.run({ question: "What is a checkpoint?" });
console.log(out.channels.agentResult);
```

Conditional routing is always a **named predicate** — never an `eval`'d string — which
is what keeps Ailu's flows safe and inspectable.

Documentation: <https://ai-adriane.github.io/ailu-core/>.

## License

Apache-2.0. See [`LICENSE`](./LICENSE).
