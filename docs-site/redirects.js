// Old documentation URLs → their new home, so links from before the 1.28 rewrite (bookmarks,
// search results, older llms.txt files, AI answers) keep working.

/** New page → the old pages it replaces. Paths are site-relative, under /docs. */
const MOVES = {
  "/docs/": [
    "introduction/why-ailu",
    "introduction/comparison",
    "start-here/pick-your-path",
    "roadmap",
    "architecture/overview",
    "architecture/napi-bridge"
  ],
  "/docs/install": ["getting-started/installation", "core-concepts/runtime-and-engine"],
  "/docs/quickstart": [
    "getting-started/quickstart",
    "getting-started/your-first-run",
    "getting-started/agent-quickstart",
    "getting-started/governance-quickstart"
  ],
  "/docs/guides/graphs": [
    "core-concepts/graphs-nodes-edges-state",
    "core-concepts/channels-and-reducers",
    "core-concepts/execution-contract",
    "building/action-nodes-and-routing"
  ],
  "/docs/guides/agents": [
    "building/agent-nodes-and-react",
    "building/llm-gateway",
    "building/providers",
    "integrations/overview",
    "integrations/llm-providers/overview",
    "integrations/middleware/overview",
    "advanced-agents/middleware-and-profiles",
    "recipes/structured-output",
    "recipes/model-packages",
    "recipes/multimodal-input"
  ],
  "/docs/guides/tools": [
    "building/tools-and-tool-nodes",
    "governance/tool-approval-and-attestation",
    "governance/approval-decision",
    "integrations/tool-registries/overview"
  ],
  "/docs/guides/human-approval": ["governance/approval-gates", "core-concepts/resumability-and-approvals"],
  "/docs/guides/streaming": ["building/streaming-and-events", "recipes/token-streaming", "recipes/stream-to-dashboard"],
  "/docs/guides/multi-agent": [
    "building/multi-agent-orchestration",
    "building/subgraphs",
    "building/dynamic-message-send",
    "recipes/parallel-fan-out",
    "recipes/hierarchical-delegation",
    "recipes/governed-council-decision",
    "recipes/react-planner-critic"
  ],
  "/docs/guides/deep-agents": [
    "advanced-agents/overview",
    "advanced-agents/deep-agents",
    "advanced-agents/governed-filesystem",
    "advanced-agents/skills",
    "recipes/governed-skills",
    "recipes/agent-memory",
    "core-concepts/memory-architecture",
    "integrations/backends/overview",
    "integrations/sandboxes/overview"
  ],
  "/docs/guides/rag": [
    "integrations/retrievers/overview",
    "integrations/vector-stores/overview",
    "integrations/text-splitters/overview",
    "knowledge/knowledge-base-and-graph",
    "knowledge/open-knowledge-format"
  ],
  "/docs/guides/long-running": ["building/durable-timers-and-signals", "integrations/checkpointers/overview"],
  "/docs/guides/governance": [
    "governance/governance-model",
    "governance/the-moat",
    "governance/compliance-framework",
    "governance/replay-as-evidence",
    "governance/pii-redaction",
    "recipes/secrets-and-no-log"
  ],
  "/docs/guides/observability": [
    "governance/observability-otel",
    "governance/observable-runs",
    "production/troubleshooting",
    "recipes/dev-inspector"
  ],
  "/docs/guides/production": ["production/deployment", "production/best-practices"],
  "/docs/guides/yaml-and-cli": [
    "dsl/graph-yaml-syntax",
    "dsl/compiler-pipeline",
    "dsl/prompt-agent-chain-syntax",
    "cli/commands",
    "recipes/yaml-and-builder"
  ],
  "/docs/guides/python": [
    "sdk-parity/python-sdk",
    "sdk-parity/one-engine-two-languages",
    "sdk-parity/polyglot-c-abi"
  ],
  "/docs/examples/overview": ["recipes/overview", "recipes/idea-to-ship-pipeline"],
  "/docs/examples/refund-agent": ["recipes/governed-refund-agent"],
  "/docs/examples/document-qa": ["recipes/rag-question-answerer"],
  "/docs/examples/resume-across-processes": ["recipes/resume-across-processes"],
  "/docs/examples/deep-agent": ["recipes/build-a-governed-deep-agent"],
  "/docs/reference/api": ["reference/builder-api", "sdk-parity/typescript-sdk"],
  "/docs/reference/models": [
    "integrations/models/overview",
    "integrations/models/anthropic",
    "integrations/models/openai",
    "integrations/models/google",
    "integrations/models/mistral",
    "integrations/models/openrouter",
    "integrations/models/huggingface",
    "integrations/models/ollama",
    "integrations/models/azure",
    "integrations/models/groq",
    "integrations/models/nvidia",
    "integrations/models/aws-bedrock"
  ],
  "/docs/reference/components": ["reference/component-catalog", "building/components-reference"],
  "/docs/reference/events": ["reference/events-and-streams"],
  "/docs/reference/environment": ["reference/configuration-and-env"],
  "/docs/reference/for-ai-agents": ["reference/built-for-ai-agents", "building/mcp-server"],
  "/docs/reference/glossary": ["glossary"]
};

/** @type {Array<{ from: string[]; to: string }>} */
const redirects = Object.entries(MOVES).map(([to, olds]) => ({
  to,
  from: olds.map((old) => `/docs/${old}`)
}));

module.exports = { redirects };
