// Regenerate the committed files that are derived from the SDK: llms.txt (repo root and docs
// site) and the component reference of the docs site.
// Run: pnpm --filter @ailu-ai/graph-sdk run gen:docs
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { generateComponentsDoc } from "../src/components-doc.js";
import { generateLlmsTxt } from "../src/llms-txt-generator.js";

const write = (target: string, text: string): void =>
  writeFileSync(fileURLToPath(new URL(target, import.meta.url)), text);

const llms = generateLlmsTxt();
write("../../../llms.txt", llms);
write("../../../docs-site/static/llms.txt", llms);
write("../../../docs-site/docs/reference/_components.md", generateComponentsDoc());
