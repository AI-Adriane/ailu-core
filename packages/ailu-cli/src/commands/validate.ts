import { readFile } from "node:fs/promises";

import { compileFile } from "../../../lang-ailu/src/compiler/compile-file.js";
import { compileGraphFile } from "../../../graph-ailu/src/compiler/compile-graph-file.js";
import { formatDiagnostics } from "../formatter/diagnostics.js";

const isGraphFile = (file: string): boolean => file.includes(".graph.");

export const validateCommand = async (file: string): Promise<number> => {
  const content = await readFile(file, "utf8");
  const compiled = isGraphFile(file) ? compileGraphFile(content, file) : compileFile(content, file);
  const output = formatDiagnostics(compiled.diagnostics, content);
  if (output.length > 0) {
    process.stdout.write(`${output}\n`);
  }
  return compiled.diagnostics.some((d) => d.severity === "error") ? 1 : 0;
};
