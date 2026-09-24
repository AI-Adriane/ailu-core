import React from "react";
import Link from "@docusaurus/Link";
import useBaseUrl from "@docusaurus/useBaseUrl";
import Layout from "@theme/Layout";
import CodeBlock from "@theme/CodeBlock";

const HERO_CODE = `import { createGraph, model } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "refunder" })
  .agentNode("decide", {
    model: model.anthropic("claude-sonnet-4-6"),
    prompt: { system: "Decide whether to refund the order." }
  })
  .humanGate("review")          // pause for a human before it acts
  .compile();

const run = await app.run({ request: "refund order #1024" });
// run.status === "suspended"  → stopped at the gate
await app.resume(run.runId);   // after a human approves
// status === "completed"`;

// Task-based entry: goals, not features. Each is one click to the right page.
const GOALS = [
  { k: "eval", label: "Evaluate Ailu in 5 minutes", to: "/docs/quickstart" },
  { k: "build", label: "Build a governed agent", to: "/docs/guides/agents" },
  { k: "deep", label: "Ship a deep agent", to: "/docs/guides/deep-agents" },
  { k: "gate", label: "Add an approval gate", to: "/docs/guides/tools" },
  { k: "comply", label: "Pass a compliance review", to: "/docs/guides/governance" },
  { k: "model", label: "Wire a model provider", to: "/docs/reference/models" },
  { k: "agent", label: "Let an AI agent author graphs", to: "/docs/reference/for-ai-agents" },
  { k: "ship", label: "Deploy to production", to: "/docs/guides/production" }
];

// Ordered reading paths, one per audience.
const TRACKS = [
  { name: "Evaluate in 5 minutes", body: "Install, run a governed agent, watch it suspend at a gate and resume.", to: "/docs/quickstart" },
  { name: "Build a deep agent", body: "Plan with todos, spawn sub-agents, load skills — governed throughout.", to: "/docs/guides/deep-agents" },
  { name: "Governance & compliance", body: "Approval gates, attestation, determinism — end to end.", to: "/docs/guides/governance" },
  { name: "For AI coding agents", body: "Machine-legible surface: /llms.txt, JSON Schema, a recovery loop.", to: "/docs/reference/for-ai-agents" }
];

/** A runtime state, told by a word and a dot — never by colour alone. */
function StateBadge({ tone, children }) {
  return (
    <span className={`ailu-badge ailu-badge--${tone}`}>
      <span className="ailu-dot" aria-hidden="true" />
      {children}
    </span>
  );
}

function Hero() {
  return (
    <header className="ailu-hero">
      <div className="container ailu-hero__grid">
        <div>
          <StateBadge tone="pending">alpha · honest about scope</StateBadge>
          <h1 className="ailu-hero__title">
            Governed agents,
            <br />
            by construction.
          </h1>
          <p className="ailu-hero__tagline">
            A stateful, resumable agent-graph engine. Deterministic, checkpointed after every node, and
            gated for human approval — so an agent stops for a human before it acts.
          </p>
          <div className="ailu-hero__actions">
            <Link className="button button--primary button--lg" to="/docs/quickstart">
              Try in 5 minutes →
            </Link>
            <Link className="button button--secondary button--lg" to="/docs/">
              Why Ailu?
            </Link>
          </div>
          <p className="ailu-well">
            <strong>Building with a coding agent?</strong> Start at{" "}
            <a href={useBaseUrl("/llms.txt")}>/llms.txt</a> and the{" "}
            <Link to="/docs/reference/for-ai-agents">For AI agents</Link> guide.
          </p>
        </div>
        <div className="ailu-hero__demo">
          <CodeBlock language="ts" title="refunder.ts">
            {HERO_CODE}
          </CodeBlock>
          <div className="ailu-runstrip" aria-label="Run lifecycle">
            <span className="ailu-runstrip__step">run()</span>
            <span aria-hidden="true">→</span>
            <StateBadge tone="pending">suspended</StateBadge>
            <span aria-hidden="true">→</span>
            <span className="ailu-runstrip__step">approve (resolvedBy = you)</span>
            <span aria-hidden="true">→</span>
            <StateBadge tone="approved">completed</StateBadge>
          </div>
        </div>
      </div>
    </header>
  );
}

function Goals() {
  return (
    <section className="container ailu-section">
      <h2 className="ailu-eyebrow">I want to…</h2>
      <div className="ailu-tiles">
        {GOALS.map((goal) => (
          <Link className="ailu-card ailu-card--tile" key={goal.k} to={goal.to}>
            <span className="ailu-card__key">{goal.k}</span>
            <span className="ailu-card__title">{goal.label}</span>
          </Link>
        ))}
      </div>
    </section>
  );
}

function Tracks() {
  return (
    <section className="container ailu-section">
      <h2 className="ailu-eyebrow">Pick your path</h2>
      <div className="ailu-cards">
        {TRACKS.map((track) => (
          <Link className="ailu-card" key={track.name} to={track.to}>
            <h3 className="ailu-card__title">{track.name}</h3>
            <p className="ailu-card__body">{track.body}</p>
            <span className="ailu-card__go">Start →</span>
          </Link>
        ))}
      </div>
      <p className="ailu-scope">
        Honest about scope: see <Link to="/docs/#whats-stable">what is stable today</Link>.
      </p>
    </section>
  );
}

export default function Home() {
  return (
    <Layout
      title="Governed agents, by construction"
      description="Ailu — a stateful, resumable agent-graph engine. Deterministic, checkpointed, governed with human-approval gates. One Rust engine, TypeScript and Python SDKs."
    >
      <Hero />
      <main>
        <Goals />
        <Tracks />
      </main>
    </Layout>
  );
}
