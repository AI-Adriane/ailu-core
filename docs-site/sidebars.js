// @ts-check

// One page per question, in the order a newcomer needs them: start, then a guide per task, then
// complete examples, then reference. Each page shows one way to do the task, with code that is
// typechecked and run by the SDK's tests (see src/remark/code-files.js).
/** @type {import('@docusaurus/plugin-content-docs').SidebarsConfig} */
const sidebars = {
  docs: [
    {
      type: "category",
      label: "Start",
      collapsed: false,
      items: ["intro", "install", "quickstart"]
    },
    {
      type: "category",
      label: "Guides",
      collapsed: false,
      items: [
        "guides/graphs",
        "guides/agents",
        "guides/tools",
        "guides/human-approval",
        "guides/streaming",
        "guides/multi-agent",
        "guides/deep-agents",
        "guides/rag",
        "guides/long-running",
        "guides/governance",
        "guides/observability",
        "guides/production",
        "guides/yaml-and-cli",
        "guides/python"
      ]
    },
    {
      type: "category",
      label: "Examples",
      items: [
        "examples/overview",
        "examples/refund-agent",
        "examples/document-qa",
        "examples/streaming-chat",
        "examples/resume-across-processes",
        "examples/parallel-agents",
        "examples/deep-agent"
      ]
    },
    {
      type: "category",
      label: "Reference",
      items: [
        "reference/api",
        "reference/models",
        "reference/components",
        "reference/events",
        "reference/errors",
        "reference/environment",
        "reference/for-ai-agents",
        "reference/migration",
        "reference/glossary"
      ]
    }
  ]
};

module.exports = sidebars;
