---
title: Govern it
description: Rung 3 of the ladder — gate a sensitive tool, approve it out of band, and get a signed, replayable attestation. The payoff.
---

# Govern it

[Quickstart](./quickstart) made a run **suspend**. [Add a real agent](./agent-quickstart) made it
**think**. Now the payoff Ailu exists for: when the agent reaches for a **sensitive tool**, the
whole run pauses for a human — and the decision becomes **cryptographic, replayable evidence**.

## 1. Mark a tool as gated

```ts
import { createGraph, DefaultLLMGateway, InMemoryToolRegistry } from "@ailu-ai/graph-sdk";

const tools = new InMemoryToolRegistry();
tools.register(
  {
    name: "refund",
    description: "Issue a customer refund.",
    requiresApproval: true,                 // ← this tool needs a human
    jsonSchema: { type: "object" }
  },
  async () => ({ ok: true, refunded: true })
);

const app = createGraph({ name: "support" })
  .messagesChannel()
  .agentNode("assistant", {
    llm: new DefaultLLMGateway(),
    prompt: { system: "You are a support agent. Use tools when needed." },
    tools,
    suspendForApproval: true                // ← suspend the run the instant approval is needed
  })
  .compile();
```

## 2. Run, suspend, approve

```ts
const suspended = await app.run({
  messages: [{ role: "user", content: "Refund order ORD-8830 — duplicate charge." }]
});
console.log(suspended.status);              // "suspended" — the agent asked for `refund`; NOTHING ran

// A DIFFERENT human approves out of band (an agent can never approve its own tool call),
// then you resume from the latest checkpoint, granting the named tool:
const done = await app.approveAndResume(suspended.runId, ["refund"]);
console.log(done.status);                   // "completed" — the refund tool then executed
```

## Expected result

```
suspended
completed
```

The run stops **before** the side effect. No refund is issued until a human grants it — and that
grant is recorded as a tamper-evident, **Ed25519-signed** decision, chained to the run.

## The payoff — proof, not logs

That approval isn't a log line you have to trust. It's:

- **Attested** — signed + hash-chained, so it can't be altered or reordered after the fact.
- **Replayable** — the run can be re-derived from its journal and checked to reach the *same*
  attested decision. "Don't trust us — re-run it yourself."
- **No self-approval** — the engine forbids an agent approving its own output, structurally.

This is the moat. Go deeper:

- [The moat](/docs/governance/the-moat) — why governed + replayable beats log-after-the-fact.
- [Tool approval & attestation](/docs/governance/tool-approval-and-attestation) — the full seam.
- [Replay as evidence](/docs/governance/replay-as-evidence) — the cryptographic proof model.

## You climbed the ladder 🪜

**Suspend** (Quickstart) → **think** ([agent](./agent-quickstart)) → **govern + prove** (here). You
now have the smallest complete picture of what Ailu is for.

Next: [Why Ailu](/docs/introduction/why-ailu) for the full thesis, or jump into the
[recipes](/docs/recipes/overview) to build the real thing.
