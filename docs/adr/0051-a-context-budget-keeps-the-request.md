# ADR 0051 — A context budget keeps the user's request

- Status: **Proposed** — awaiting Mathieu's validation (it changes the prompt a budgeted agent
  sends and adds a field to the replay journal).
- Date: 2026-10-10
- Driven by the beta QA finding N3-1 (2026-10-10, blocking): the product's `approval-demo` agent
  answered « Refund request not found in message » on four runs and never called `refund`, so no
  tool gate opened. Same family: the product's answer / critic agents lost their `question`
  channel under a budget (product memory, 2026-10-09, worked around with `visibleChannels`).
- Relates to: [0014](./0014-engine-token-efficiency.md) (the context budget),
  [0025](./0025-unified-agent-middleware-api.md) (it became `ContextBudgetMiddleware`),
  [0035](./0035-skills-progressive-disclosure.md) (skills prepended to the seed),
  [0038](./0038-replay-as-evidence.md) (the replay journal),
  [0047](./0047-the-brain-shown-to-an-agent-that-may-see-it.md) (the governed brain block).

## Context

An agent's first message (its *seed*) is built by the ReAct loop as
`Input: <json>\nState: <json object>`; the user's request is in it — the `Input` line for a
`mapAgents` spawn, a State channel (`input`, `question`) for a graph agent. The `before_run`
middleware then run in order: memory recall, the governed brain, skills, and the context budget.
The first three **prepend** a block and a blank line. `ContextBudgetMiddleware` kept the first
`chars` characters and added `…`.

So once the prepended context is larger than the budget, the request is exactly what goes. On the
beta, the tenant had two skills installed (5 211 + 4 372 characters, both selected: the advisory
top-k is 3), the brain block was ~2.7 k characters, and the org policy `token-efficiency` sets a
12 000-character budget: the seed ended inside the second skill. Prompt tokens stayed at ~5 250
whatever the request.

State is serialized with sorted keys and `__*` channels sort first, so a budget smaller than the
State alone also drops the channels that come late in the alphabet — `question` behind `context`
or `__brainRecall`.

## Decision

### D1 — The cut keeps the request (`BudgetTrim::KeepRequest`)

When the seed is over budget, the budget keeps, in this order:

1. **Always, whatever it costs:** the `Input:` line, and the State entries of the request channels
   `input` and `question` (`REQUEST_CHANNELS`). The budget may then be exceeded by the request
   itself — never cut.
2. If the whole `Input` / `State` part fits: all of it, and the rest of the budget goes to the
   prepended context, **kept from its end**. Skills are prepended last, so they are read first and
   cut first; then the brain, then memory. The cut is marked by a leading `…`.
3. If it does not fit: the prepended context goes entirely, and the other State entries are kept
   in seed order — ordinary channels before reserved `__*` ones (engine and control-plane plumbing,
   e.g. `__brainRecall`, which repeats the brain block). The first entry that does not fit is cut
   to the room left (when that still shows its key) and the rest go. The seed ends with `…`.

Kept entries are verbatim: the seed is parsed only if re-rendering its State gives back the exact
same text. A seed this does not recognise (not built by the ReAct loop) keeps the 2.6 head cut.
The cut is a pure function of the seed and the budget: the same seed always gives the same prompt.
A seed within budget is left untouched, so a run that was never cut sends exactly what it sent.

### D2 — The journal records the cut; an old journal replays the old cut

`ReplayGateway` serves a recorded response only to a request equal to the recorded one, so a replay
must rebuild each prompt byte for byte. The replay journal gets an optional field
`contextBudgetTrim` (`"headCut"` | `"keepRequest"`):

- a recording run always writes `"keepRequest"`;
- a replay cuts the way its journal says; a journal without the field — recorded by 2.6 or
  earlier — replays with the 2.6 head cut (`BudgetTrim::HeadCut`, kept for that purpose only);
- a live run cuts with `keepRequest`.

This follows the journal's earlier additive fields (`toolResults`, `nodeResults`): absence means
"recorded before", and the replay behaves as the engine did then. An unknown value fails the replay
loudly (`invalid replay_journal JSON`) rather than replaying with the wrong cut.

## Consequences

- **Prompts change** only for a budgeted agent whose seed is over budget — which, until now, was
  the case where the request could be lost. The prompt may now exceed the budget by the size of the
  request.
- **Replay:** every journal recorded before this change still replays byte-identically
  (D2). A run started on 2.6 and resumed on 2.7 is replayed segment by segment, each with its own
  journal and its own mark.
- **Public API:** no change for the TypeScript or Python SDKs, nor for the spec on the wire. Rust:
  `ailu_agents_core::{BudgetTrim, trim_seed, REQUEST_CHANNELS}`, `ContextBudgetMiddleware::with_trim`;
  `ContextBudgetMiddleware::new` now keeps the request.
- **Product:** bump `@ailu-ai/graph-sdk` to the release that carries this; the per-agent
  `visibleChannels` workaround for `question` can stay or go.

## Open questions

1. **A declared request channel.** The request channels are a convention (`input`, `question`). A
   `keep: string[]` parameter on `contextBudget` (Rust bridge, the SDK's `EfficiencyMiddlewareSpec`
   and the contracts schema) would let a graph name its own. It is an SDK API addition, left for a
   separate change.
2. **Skills selection.** The advisory top-k runs on `MockEmbedder` in the bridge, so every installed
   skill is selected for every agent, related or not. That is why the beta seed was full; it is a
   separate fix (real embedder, or advisory off by default).
3. **Duplicate brain.** The brain is both prepended and present in State as `__brainRecall`. Hiding
   reserved channels from State would save ~2.7 k characters per call, but changes every prompt
   (and needs the same journal mark), so it is not done here.
