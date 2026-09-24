import React from "react";
import Link from "@docusaurus/Link";
import useBaseUrl from "@docusaurus/useBaseUrl";
import Layout from "@theme/Layout";
import CodeBlock from "@theme/CodeBlock";

const HERO_CODE = `import { createGraph, DefaultLLMGateway } from "@ailu/graph-sdk";

const app = createGraph({ name: "refunder" })
  .agentNode("decide", {
    llm: new DefaultLLMGateway(),
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
  { k: "eval", label: "Evaluate Ailu in 5 minutes", to: "/docs/getting-started/quickstart" },
  { k: "build", label: "Build a governed agent", to: "/docs/building/agent-nodes-and-react" },
  { k: "deep", label: "Ship a deep agent", to: "/docs/advanced-agents/overview" },
  { k: "gate", label: "Add an approval gate", to: "/docs/governance/approval-decision" },
  { k: "comply", label: "Pass a compliance review", to: "/docs/governance/governance-model" },
  { k: "model", label: "Wire a model provider", to: "/docs/integrations/models/overview" },
  { k: "agent", label: "Let an AI agent author graphs", to: "/docs/reference/built-for-ai-agents" },
  { k: "ship", label: "Deploy to production", to: "/docs/production/deployment" }
];

// Ordered reading paths, one per audience.
const TRACKS = [
  { name: "Evaluate in 5 minutes", body: "Install, run a governed agent, watch it suspend at a gate and resume.", to: "/docs/getting-started/quickstart" },
  { name: "Build a deep agent", body: "Plan with todos, spawn sub-agents, load skills — governed throughout.", to: "/docs/advanced-agents/overview" },
  { name: "Governance & compliance", body: "Approval gates, attestation, determinism — the moat, end to end.", to: "/docs/governance/governance-model" },
  { name: "For AI coding agents", body: "Machine-legible surface: /llms.txt, JSON Schema, a recovery loop.", to: "/docs/reference/built-for-ai-agents" }
];

function Hero() {
  return (
    <header className="hero2">
      <div className="container hero2Grid">
        <div className="hero2Copy">
          <span className="alphaBadge">alpha · honest about scope</span>
          <h1 className="hero2Title">Governed agents,<br />by construction.</h1>
          <p className="hero2Tagline">
            A stateful, resumable agent-graph engine. Deterministic, checkpointed after every node,
            and gated for human approval — so an agent <strong>stops for a human</strong> before it acts.
          </p>
          <div className="hero2Buttons">
            <Link className="button button--primary button--lg" to="/docs/getting-started/quickstart">
              Try in 5 minutes →
            </Link>
            <Link className="button button--secondary button--lg" to="/docs/introduction/why-ailu">
              Why Ailu?
            </Link>
          </div>
          <div className="agentStrip">
            <span className="agentBot">🤖 Building with a coding agent?</span>
            <span>
              Start at <a href={useBaseUrl("/llms.txt")}>/llms.txt</a> and the{" "}
              <Link to="/docs/reference/built-for-ai-agents">For-AI-agents</Link> guide.
            </span>
          </div>
        </div>
        <div className="hero2Demo">
          <CodeBlock language="ts">{HERO_CODE}</CodeBlock>
          <div className="runStrip">
            <span className="runStep">run()</span>
            <span className="arrow">→</span>
            <span className="pill pillSuspended">suspended</span>
            <span className="arrow">→</span>
            <span className="runStep">approve (resolvedBy = you)</span>
            <span className="arrow">→</span>
            <span className="pill pillDone">completed</span>
          </div>
        </div>
      </div>
    </header>
  );
}

function Goals() {
  return (
    <section className="container homeSection">
      <h2 className="homeH2">I want to…</h2>
      <div className="goalGrid">
        {GOALS.map((g) => (
          <Link className="goalTile" key={g.k} to={g.to}>
            <span className="goalK">{g.k}</span>
            {g.label}
          </Link>
        ))}
      </div>
    </section>
  );
}

function Tracks() {
  return (
    <section className="container homeSection">
      <h2 className="homeH2">Pick your path</h2>
      <div className="trackGrid">
        {TRACKS.map((t) => (
          <Link className="trackCard" key={t.name} to={t.to}>
            <h3>{t.name}</h3>
            <p>{t.body}</p>
            <span className="trackGo">Start →</span>
          </Link>
        ))}
      </div>
      <p className="scopeStrip">
        Honest about scope: see the{" "}
        <Link to="/docs/introduction/comparison">comparison</Link> and the{" "}
        <Link to="/docs/roadmap">roadmap</Link> (stable / experimental / reserved).
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
