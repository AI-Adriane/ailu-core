// @ts-check
const prismAilu = require("./src/prism-ailu.js");

/** @type {import('@docusaurus/types').Config} */
const config = {
  title: "Ailu",
  tagline: "The governed agentic graph framework — deterministic, resumable, observable.",
  favicon: "img/favicon.svg",

  // GitHub Pages serves this repository's site at https://ai-adriane.github.io/ailu-core/.
  url: "https://ai-adriane.github.io",
  baseUrl: "/ailu-core/",
  organizationName: "AI-Adriane",
  projectName: "ailu-core",

  onBrokenLinks: "warn",

  // The Ailu typefaces, self-hosted (see src/fonts.js).
  clientModules: [require.resolve("./src/fonts.js")],
  markdown: {
    mermaid: true,
    hooks: {
      onBrokenMarkdownLinks: "warn"
    }
  },

  i18n: {
    defaultLocale: "en",
    locales: ["en"]
  },

  presets: [
    [
      "classic",
      /** @type {import('@docusaurus/preset-classic').Options} */
      ({
        docs: {
          routeBasePath: "docs",
          sidebarPath: require.resolve("./sidebars.js"),
          editUrl: "https://github.com/AI-Adriane/ailu-core/tree/main/docs-site/"
        },
        blog: false,
        theme: {
          customCss: require.resolve("./src/css/custom.css")
        }
      })
    ]
  ],

  themes: [
    "@docusaurus/theme-mermaid",
    [
      // Offline full-text search (no Algolia signup; works on GitHub Pages).
      require.resolve("@easyops-cn/docusaurus-search-local"),
      {
        hashed: true,
        indexBlog: false,
        docsRouteBasePath: "/docs",
        highlightSearchTermsOnTargetPage: true
      }
    ]
  ],

  themeConfig:
    /** @type {import('@docusaurus/preset-classic').ThemeConfig} */
    ({
      colorMode: {
        defaultMode: "light",
        respectPrefersColorScheme: true
      },
      mermaid: {
        theme: { light: "neutral", dark: "dark" },
        options: { fontFamily: "Instrument Sans, system-ui, sans-serif" }
      },
      image: "img/logo.svg",
      navbar: {
        title: "Ailu",
        logo: {
          alt: "Ailu",
          src: "img/logo.svg"
        },
        items: [
          {
            type: "docSidebar",
            sidebarId: "docs",
            position: "left",
            label: "Docs"
          },
          {
            to: "/docs/reference/builder-api",
            label: "API Reference",
            position: "left"
          },
          {
            to: "/docs/recipes/overview",
            label: "Cookbook",
            position: "left"
          },
          {
            to: "/docs/reference/built-for-ai-agents",
            label: "For AI agents",
            position: "left"
          },
          {
            href: "https://github.com/AI-Adriane/ailu-core/blob/main/CONTRIBUTING.md",
            label: "Contribute",
            position: "right"
          },
          {
            href: "https://github.com/AI-Adriane/ailu-core/releases",
            label: "Releases",
            position: "right"
          },
          {
            href: "https://github.com/AI-Adriane/ailu-core",
            label: "GitHub",
            position: "right"
          }
        ]
      },
      footer: {
        style: "light",
        links: [
          {
            title: "Learn",
            items: [
              { label: "Why Ailu", to: "/docs/introduction/why-ailu" },
              { label: "Installation", to: "/docs/getting-started/installation" },
              { label: "Your first run", to: "/docs/getting-started/your-first-run" }
            ]
          },
          {
            title: "Build",
            items: [
              { label: "Core concepts", to: "/docs/core-concepts/graphs-nodes-edges-state" },
              { label: "Governance", to: "/docs/governance/governance-model" },
              { label: "SDK parity", to: "/docs/sdk-parity/one-engine-two-languages" }
            ]
          },
          {
            title: "More",
            items: [
              { label: "GitHub", href: "https://github.com/AI-Adriane/ailu-core" },
              { label: "npm — @ailu-ai/graph-sdk", href: "https://www.npmjs.com/package/@ailu-ai/graph-sdk" },
              { label: "PyPI — ailu", href: "https://pypi.org/project/ailu/" }
            ]
          }
        ],
        copyright: `Apache-2.0 licensed. The Ailu framework.`
      },
      prism: {
        theme: prismAilu.light,
        darkTheme: prismAilu.dark,
        additionalLanguages: ["bash", "python", "rust", "yaml", "json", "toml"]
      }
    })
};

module.exports = config;
