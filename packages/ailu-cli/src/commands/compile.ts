import { mkdir, readFile, writeFile } from "node:fs/promises";
import { basename, join } from "node:path";

import { compileFile } from "../../../lang-ailu/src/compiler/compile-file.js";
import { compileGraphFile } from "../../../graph-ailu/src/compiler/compile-graph-file.js";
import { formatDiagnostics } from "../formatter/diagnostics.js";

const isGraphFile = (file: string): boolean => file.includes(".graph.");

export const compileCommand = async (file: string, outDir: string): Promise<number> => {
  const content = await readFile(file, "utf8");
  const compiled = isGraphFile(file) ? compileGraphFile(content, file) : compileFile(content, file);
  const errors = compiled.diagnostics.filter((d) => d.severity === "error");
  if (errors.length > 0 || compiled.result === undefined) {
    process.stdout.write(`${formatDiagnostics(compiled.diagnostics, content)}\n`);
    return 1;
  }
  await mkdir(outDir, { recursive: true });
  const outFile = join(outDir, `${basename(file).replace(/\.[^.]+$/, "")}.json`);
  await writeFile(outFile, JSON.stringify(compiled.result, null, 2), "utf8");
  process.stdout.write(`Wrote ${outFile}\n`);
  return 0;
};
