// Code blocks that show a real, tested file instead of pasted text.
//
// In a doc page, a fenced block whose meta names a file is filled from that file at build time:
//
//   ```ts file=packages/graph-sdk/examples/quickstart.ts
//   ```
//
// The path is relative to the repository root. `region=<name>` shows only the lines between
// `// #region <name>` and `// #endregion <name>` (dedented); region markers never reach the page.
// A whole file is titled with its file name; a region is shown untitled unless `title=` is given.
// A missing file or region fails the build, so a page cannot drift from the code it shows.
// Every file shown this way is typechecked and run by the SDK's test suite.

const fs = require("node:fs");
const path = require("node:path");

const REPO_ROOT = path.resolve(__dirname, "../../..");
const FILE_META = /(?:^|\s)file=(\S+)/;
const REGION_META = /(?:^|\s)region=(\S+)/;
const MARKER = /^\s*(?:\/\/|#)\s*#(?:region|endregion)\b.*$/;

const dedent = (lines) => {
  const indents = lines.filter((line) => line.trim() !== "").map((line) => line.match(/^ */)[0].length);
  const cut = indents.length === 0 ? 0 : Math.min(...indents);
  return lines.map((line) => line.slice(cut));
};

/** The text a block shows: the whole file, or one region of it, without region markers. */
const readCodeFile = (relativePath, region) => {
  const absolute = path.resolve(REPO_ROOT, relativePath);
  if (!absolute.startsWith(REPO_ROOT + path.sep)) {
    throw new Error(`code-files: ${relativePath} is outside the repository`);
  }
  const lines = fs.readFileSync(absolute, "utf8").split("\n");
  let selected = lines;
  if (region !== undefined) {
    const start = lines.findIndex((line) => new RegExp(`#region\\s+${region}\\b`).test(line));
    const end = lines.findIndex((line, i) => i > start && new RegExp(`#endregion\\s+${region}\\b`).test(line));
    if (start === -1 || end === -1) {
      throw new Error(`code-files: region "${region}" not found in ${relativePath}`);
    }
    selected = dedent(lines.slice(start + 1, end));
  }
  return selected
    .filter((line) => !MARKER.test(line))
    .join("\n")
    .replace(/\n+$/, "");
};

/** Resolve a code block's meta: `{ value, meta }` with the file's text, or `undefined`. */
const resolveCodeBlock = (meta) => {
  if (typeof meta !== "string") return undefined;
  const file = FILE_META.exec(meta);
  if (file === null) return undefined;
  const region = REGION_META.exec(meta);
  const value = readCodeFile(file[1], region === null ? undefined : region[1]);
  let rest = meta.replace(FILE_META, " ").replace(REGION_META, " ").trim();
  // A whole file is titled with its name; a region is an excerpt, shown untitled unless the page
  // gives it a title.
  if (region === null && !/(?:^|\s)title=/.test(rest)) {
    rest = `${rest} title="${path.basename(file[1])}"`.trim();
  }
  return { value, meta: rest, file: file[1] };
};

/** The remark plugin: fill every `file=` code block from its file. */
const remarkCodeFiles = () => (tree) => {
  const walk = (node) => {
    if (node.type === "code") {
      const resolved = resolveCodeBlock(node.meta);
      if (resolved !== undefined) {
        node.value = resolved.value;
        node.meta = resolved.meta;
      }
    }
    for (const child of node.children ?? []) walk(child);
  };
  walk(tree);
};

module.exports = { remarkCodeFiles, resolveCodeBlock, readCodeFile, REPO_ROOT };
