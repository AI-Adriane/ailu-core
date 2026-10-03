# ADR 0047 — The brain is shown to an agent that may see it

- Status: **Accepted** — 2026-10-03, by the owner (Mathieu: « ba g oui »).
- Date: 2026-10-03
- Deciders: Mathieu (owner)
- Driven by the product repo's ADR 0105 (web search for Mesh agents), D2 as applied on
  2026-10-02: a web agent is shown only the run's input, yet the engine added the organisation's
  brain to its first message. The product closed it by running any run with a web agent without
  the brain, "until the engine shows it only to the agents it is meant for" — this ADR.
- Relates to: [0045](./0045-host-nodes-in-rust-and-sdk-parity.md) (Rust first, both SDKs in the
  same release).

## Context

Read from the source on 2026-10-03 (release 2.3.0).

- **An agent declares what it sees.** `visibleChannels` on an agent spec (TypeScript, the catalog
  carrier, Python `visible_channels`) narrows the state written into its seed message
  (`ReactAgent::run`, `crates/agents-core/src/react.rs`).
- **The brain ignores it.** `BrainMiddleware` (`crates/agents-core/src/brain.rs`) is installed on
  every agent (`crates/runtime-bridge/src/lib.rs`, `push_governed`) and reads `__brainRecall` from
  the run's full channels (`RunCtx.channels`), then prepends it to the seed. An agent narrowed to
  one channel still receives the organisation's governed knowledge.
- **The brain is only in Rust.** Neither the TypeScript fallback runtime nor Python injects it:
  both SDKs reach it through the Rust core, so one change covers both.

## Decision

### D1 — An agent that declares its channels sees the brain only if it names it

The bridge installs `BrainMiddleware` on an agent when its spec declares no `visibleChannels`
(today's behaviour, unchanged), or when they include `__brainRecall`. An agent narrowed to other
channels is not given the brain. Decided once, when the agent is built — nothing is read per call.

### D2 — Nothing else changes

The recall itself, its format, its cap, the journaled seed (a replay reuses the same snapshot) and
`BrainMiddleware`'s public type stay as they are. No new field: `visibleChannels` already says
what an agent sees; the brain now obeys it like the rest of the state.

### D3 — One change, both SDKs, one release

The change is in the Rust bridge; TypeScript and Python get it by the same release (ADR 0045's
rule), with a test in each that an agent narrowed to one channel receives no brain block.

## Consequences

- **Compatibility.** An agent that declares `visibleChannels` and relied on the brain loses it
  unless it adds `__brainRecall`. In the product, only web agents declare them today (ADR 0105
  D2, Work's web step) — exactly the agents that must not see it.
- **The product's workaround goes.** A run with a web agent can keep the brain for its other agents
  instead of running without it.
- **Replay.** A run recorded before the change in which a narrowed agent received the brain would
  replay with a different seed. The product has started none since its workaround (2026-10-02):
  such runs carried no `__brainRecall` at all.

## Not decided here

- The same rule for skills (`SkillMiddleware`): the product never gives a web agent skills
  (ADR 0105 D2), and a skill is not a channel.
