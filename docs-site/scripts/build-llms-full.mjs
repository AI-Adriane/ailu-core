// Build static/llms-full.txt: the whole documentation as one plain-text file for AI agents, in
// sidebar order, with every `file=` code block filled from its tested source file.
// Runs before `docusaurus build` and `docusaurus start` (npm pre-scripts).
import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const siteDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const { resolveCodeBlock } = require(join(siteDir, "src/remark/code-files.js"));
const sidebars = require(join(siteDir, "sidebars.js"));
const version = require(join(siteDir, "../packages/graph-sdk/package.json")).version;

const docIds = [];
const collect = (items) => {
  for (const item of items) {
    if (typeof item === "string") docIds.push(item);
    else if (item.items) collect(item.items);
  }
};
collect(sidebars.docs);

const read = (id) => readFileSync(join(siteDir, "docs", `${id}.md`), "utf8");

/** A page as plain Markdown: no front matter, no MDX imports, partials and code files inlined. */
const plain = (id) => {
  let text = read(id).replace(/^---\n[\s\S]*?\n---\n/, "");
  const partials = {};
  text = text.replace(/^import (\w+) from "(.+)";\n/gm, (_, name, source) => {
    if (source.startsWith("./")) partials[name] = source.slice(2).replace(/\.md$/, "");
    return "";
  });
  text = text.replace(/<SdkVersion \/>/g, version);
  text = text.replace(/<(\w+) \/>/g, (whole, name) => {
    const partial = partials[name];
    if (partial === undefined) return whole;
    const dir = id.includes("/") ? id.slice(0, id.lastIndexOf("/") + 1) : "";
    return read(`${dir}${partial}`).replace(/^\{\/\*.*\*\/\}\n/, "");
  });
  text = text.replace(/```(\w+) ([^\n]*file=[^\n]*)\n```/g, (whole, lang, meta) => {
    const resolved = resolveCodeBlock(meta);
    return `\`\`\`${lang} ${resolved.meta}\n${resolved.value}\n\`\`\``;
  });
  return text.replace(/&#123;/g, "{").replace(/&#125;/g, "}").replace(/&lt;/g, "<").replace(/&gt;/g, ">").trim();
};

const header = `# Ailu documentation (@ailu-ai/graph-sdk ${version})

> The full Ailu documentation as one file, for AI coding agents. The short index is /llms.txt.
> Every TypeScript example below is typechecked and run by the SDK's test suite.
`;
const body = docIds.map((id) => `\n\n<!-- ${id} -->\n\n${plain(id)}`).join("");
writeFileSync(join(siteDir, "static/llms-full.txt"), `${header}${body}\n`);
console.log(`llms-full.txt: ${docIds.length} pages`);
