// @ts-check

// A MANUAL, JOURNEY-FIRST sidebar (ADR-less Phase-2 DX redesign): the top level maps to
// audience intent — Start here · Learn · Build · Govern · Operate · Integrations · Cookbook ·
// Reference · For AI agents — and Diátaxis (tutorial / how-to / reference / explanation) governs
// the grouping inside. Files stay where they are on disk: this file controls navigation grouping
// only, so doc URLs and cross-links are unchanged (a re-grouping, not a content move).
/** @type {import('@docusaurus/plugin-content-docs').SidebarsConfig} */
const sidebars = {
  docs: [
    {
      type: "category",
      label: "Start here",
      collapsed: false,
      items: [
        "introduction/why-ailu",
        "getting-started/quickstart",
        "getting-started/agent-quickstart",
        "getting-started/governance-quickstart",
        "getting-started/installation",
        "getting-started/your-first-run",
        "introduction/comparison",
        "start-here/pick-your-path"
      ]
    },
    {
      type: "category",
      label: "Learn",
      items: [
        {
          type: "category",
          label: "Foundations",
          items: [
            "core-concepts/graphs-nodes-edges-state",
            "core-concepts/channels-and-reducers",
            "core-concepts/execution-contract"
          ]
        },
        {
          type: "category",
          label: "Resumability & state",
          items: [
            "core-concepts/resumability-and-approvals",
            "core-concepts/memory-architecture"
          ]
        },
        "core-concepts/runtime-and-engine"
      ]
    },
    {
      type: "category",
      label: "Build",
      items: [
        {
          type: "category",
          label: "Graphs",
          items: [
            "building/action-nodes-and-routing",
            "building/streaming-and-events",
            "building/subgraphs",
            "building/dynamic-message-send",
            "building/durable-timers-and-signals"
          ]
        },
        {
          type: "category",
          label: "Agents",
          items: [
            "building/agent-nodes-and-react",
            "building/tools-and-tool-nodes",
            "building/multi-agent-orchestration"
          ]
        },
        {
          type: "category",
          label: "Deep agents",
          items: [
            "advanced-agents/overview",
            "advanced-agents/middleware-and-profiles",
            "advanced-agents/governed-filesystem",
            "advanced-agents/deep-agents",
            "advanced-agents/skills"
          ]
        },
        {
          type: "category",
          label: "Ailu Lang (DSL)",
          items: [
            "dsl/graph-yaml-syntax",
            "dsl/prompt-agent-chain-syntax",
            "dsl/compiler-pipeline"
          ]
        },
        {
          type: "category",
          label: "Knowledge & RAG",
          items: ["knowledge/knowledge-base-and-graph", "knowledge/open-knowledge-format"]
        },
        { type: "category", label: "Components", items: ["building/components-reference"] },
        { type: "category", label: "MCP", items: ["building/mcp-server"] }
      ]
    },
    {
      type: "category",
      label: "Govern",
      items: [
        "governance/the-moat",
        "governance/approval-decision",
        "governance/governance-model",
        "governance/approval-gates",
        "governance/tool-approval-and-attestation",
        "governance/replay-as-evidence",
        "governance/pii-redaction",
        "governance/compliance-framework"
      ]
    },
    {
      type: "category",
      label: "Operate",
      items: [
        {
          type: "category",
          label: "Observe",
          items: [
            "governance/observable-runs",
            "governance/observability-otel",
            "recipes/dev-inspector"
          ]
        },
        {
          type: "category",
          label: "Deploy & run",
          items: [
            "production/deployment",
            "production/best-practices",
            "production/troubleshooting",
            "cli/commands",
            "reference/configuration-and-env"
          ]
        }
      ]
    },
    {
      type: "category",
      label: "Integrations",
      items: [
        "integrations/overview",
        "building/llm-gateway",
        "building/providers",
        {
          type: "category",
          label: "Models",
          items: [
            "integrations/models/overview",
            "integrations/models/anthropic",
            "integrations/models/google",
            "integrations/models/openai",
            "integrations/models/azure",
            "integrations/models/mistral",
            "integrations/models/openrouter",
            "integrations/models/groq",
            "integrations/models/huggingface",
            "integrations/models/ollama",
            "integrations/models/nvidia",
            "integrations/models/aws-bedrock"
          ]
        },
        { type: "category", label: "Middleware", items: ["integrations/middleware/overview"] },
        { type: "category", label: "Backends", items: ["integrations/backends/overview"] },
        { type: "category", label: "Checkpointers", items: ["integrations/checkpointers/overview"] },
        { type: "category", label: "Retrievers", items: ["integrations/retrievers/overview"] },
        { type: "category", label: "Text splitters", items: ["integrations/text-splitters/overview"] },
        { type: "category", label: "Vector stores", items: ["integrations/vector-stores/overview"] },
        { type: "category", label: "Sandboxes", items: ["integrations/sandboxes/overview"] },
        { type: "category", label: "Tool registries", items: ["integrations/tool-registries/overview"] }
      ]
    },
    {
      type: "category",
      label: "Cookbook",
      items: [
        "recipes/overview",
        "recipes/governed-refund-agent",
        "recipes/build-a-governed-deep-agent",
        "recipes/react-planner-critic",
        "recipes/idea-to-ship-pipeline",
        "recipes/rag-question-answerer",
        "recipes/resume-across-processes",
        "recipes/stream-to-dashboard",
        "recipes/yaml-and-builder",
        "recipes/structured-output",
        "recipes/multimodal-input",
        "recipes/secrets-and-no-log",
        "recipes/agent-memory",
        "recipes/model-packages",
        "recipes/token-streaming",
        "recipes/governed-skills",
        "recipes/parallel-fan-out",
        "recipes/hierarchical-delegation",
        "recipes/governed-council-decision"
      ]
    },
    {
      type: "category",
      label: "Reference",
      items: [
        "reference/builder-api",
        "reference/component-catalog",
        "reference/events-and-streams",
        "reference/errors",
        "architecture/overview",
        "architecture/napi-bridge",
        {
          type: "category",
          label: "SDK parity",
          items: [
            "sdk-parity/one-engine-two-languages",
            "sdk-parity/typescript-sdk",
            "sdk-parity/python-sdk",
            "sdk-parity/polyglot-c-abi"
          ]
        },
        "glossary",
        "roadmap"
      ]
    },
    {
      type: "category",
      label: "For AI agents",
      items: ["reference/built-for-ai-agents"]
    }
  ]
};

module.exports = sidebars;
