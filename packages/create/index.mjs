#!/usr/bin/env node
// @ailu-ai/create — scaffold a governed Ailu app in one command (ADR DX batch 5).
//   npm create @ailu-ai@latest my-app   →   a runnable governed graph + the dev inspector.
// Dependency-free: writes a tiny starter that installs @ailu-ai/graph-sdk (prebuilt Rust
// engine — no toolchain) and opens the run inspector on a human-gated, resumable graph.

import { mkdirSync, writeFileSync, existsSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const SDK_VERSION = "^1.2.0";

/** Scaffold a governed Ailu starter into `<cwd>/<appName>`. Returns the created path.
 * Pure (no process exit / logging) so it is testable; the CLI wrapper handles I/O. */
export function scaffold(rawName, cwd = process.cwd()) {
  const appName = (rawName && !String(rawName).startsWith("-") ? String(rawName) : "ailu-app").replace(
    /[^a-zA-Z0-9._-]/g,
    "-"
  );
  const dir = join(cwd, appName);
  if (existsSync(dir)) {
    throw new Error(`"${appName}" already exists. Pick another name: npm create @ailu-ai@latest <name>`);
  }
  mkdirSync(dir, { recursive: true });
  for (const [name, content] of Object.entries(templates(appName))) {
    writeFileSync(join(dir, name), content);
  }
  return { appName, dir };
}

function templates(appName) {
  return {
  "package.json": `${JSON.stringify(
    {
      name: appName,
      private: true,
      type: "module",
      scripts: {
        start: "node --import tsx app.ts",
        inspect: "node --import tsx inspect.ts"
      },
      dependencies: { "@ailu-ai/graph-sdk": SDK_VERSION },
      devDependencies: { tsx: "^4.20.6", typescript: "^5.8.3" }
    },
    null,
    2
  )}\n`,

  "app.ts": `import { createGraph } from "@ailu-ai/graph-sdk";

// Ailu's core is governance: a run pauses at a human-approval gate and resumes from its
// checkpoint — deterministically, even across process restarts.
const app = createGraph({ name: "publish-flow" })
  .node("write", async () => ({ draft: "Hello from Ailu." }))
  .humanGate("review") // ← the run SUSPENDS here for human approval
  .node("publish", async () => ({ published: true }))
  .edge("write", "review")
  .edge("review", "publish")
  .compile();

const suspended = await app.run();
console.log("status:", suspended.status);            // "suspended"
const done = await app.resume(suspended.runId);      // a human approved
console.log("status:", done.status, "published:", done.channels.published); // "completed" true
`,

  "inspect.ts": `import { createGraph, serveInspector } from "@ailu-ai/graph-sdk";

// Watch the graph execute in your browser — node-by-node, with the governance lens.
const app = createGraph({ name: "publish-flow" })
  .node("write", async () => ({ draft: "Hello from Ailu." }))
  .humanGate("review")
  .node("publish", async () => ({ published: true }))
  .edge("write", "review")
  .edge("review", "publish")
  .compile();

const inspector = await serveInspector(app, {});
console.log("Inspecting at " + inspector.url + " — open it, then click Resume.");
`,

  "README.md": `# ${appName}

A governed Ailu agent-graph app. The Rust engine ships **prebuilt** — no toolchain to install.

\`\`\`bash
npm install
npm start        # run the governed graph (suspend → resume)
npm run inspect  # watch it execute in the browser (the dev inspector)
\`\`\`

Add an agent with one line — \`model.openai("gpt-4o")\` (set \`OPENAI_API_KEY\`); or \`model.anthropic()\`, \`model.fast\`.
Docs: https://github.com/AI-Adriane/ailu-core
`,

    ".gitignore": "node_modules\n"
  };
}

// CLI: only when run directly (not when imported by a test).
const isEntrypoint =
  process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href;
if (isEntrypoint) {
  try {
    const { appName } = scaffold(process.argv[2]);
    console.log(`\n✔ Created governed Ailu app in ./${appName}\n`);
    console.log("Next:");
    console.log(`  cd ${appName}`);
    console.log("  npm install");
    console.log("  npm start        # run the governed graph (suspend → resume)");
    console.log("  npm run inspect  # watch it in the browser\n");
  } catch (err) {
    console.error(`✖ ${err instanceof Error ? err.message : String(err)}`);
    process.exit(1);
  }
}
