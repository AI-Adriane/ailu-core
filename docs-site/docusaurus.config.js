// @ts-check
const prismAilu = require("./src/prism-ailu.js");
const { remarkCodeFiles } = require("./src/remark/code-files.js");
const { redirects } = require("./redirects.js");

// The SDK version these docs describe: read from the package, never written by hand.
const sdkVersion = require("../packages/graph-sdk/package.json").version;

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

  onBrokenLinks: "throw",
  customFields: { sdkVersion },

  // The Ailu typefaces, self-hosted (see src/fonts.js).
  clientModules: [require.resolve("./src/fonts.js")],
  markdown: {
    mermaid: true,
    hooks: {
      onBrokenMarkdownLinks: "throw"
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
          // ```ts file=path/to/example.ts``` blocks show a tested file from the repository.
          remarkPlugins: [remarkCodeFiles],
          editUrl: "https://github.com/AI-Adriane/ailu-core/tree/main/docs-site/"
        },
        blog: false,
        theme: {
          customCss: require.resolve("./src/css/custom.css")
        }
      })
    ]
  ],

  plugins: [["@docusaurus/plugin-client-redirects", { redirects }]],

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
          { to: "/docs/guides/graphs", label: "Guides", position: "left" },
          { to: "/docs/examples/overview", label: "Examples", position: "left" },
          { to: "/docs/reference/api", label: "Reference", position: "left" },
          { type: "html", position: "right", value: `<span class="ailu-version">v${sdkVersion}</span>` },
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
            title: "Start",
            items: [
              { label: "What is Ailu", to: "/docs/" },
              { label: "Install", to: "/docs/install" },
              { label: "Quickstart", to: "/docs/quickstart" }
            ]
          },
          {
            title: "Build",
            items: [
              { label: "Guides", to: "/docs/guides/graphs" },
              { label: "Examples", to: "/docs/examples/overview" },
              { label: "Reference", to: "/docs/reference/api" }
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
