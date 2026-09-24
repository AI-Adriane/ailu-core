// Regenerate the committed llms.txt (repo root and docs site) from the SDK's own catalogs.
// Run: pnpm --filter @ailu-ai/graph-sdk run gen:llms-txt
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { generateLlmsTxt } from "../src/llms-txt-generator.js";

const text = generateLlmsTxt();
for (const target of ["../../../llms.txt", "../../../docs-site/static/llms.txt"]) {
  writeFileSync(fileURLToPath(new URL(target, import.meta.url)), text);
}
