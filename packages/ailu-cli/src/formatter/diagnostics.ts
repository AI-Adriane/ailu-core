import type { Diagnostic } from "../../../lang-ailu/src/validator/types.js";

const red = (s: string): string => `\u001b[31m${s}\u001b[0m`;
const yellow = (s: string): string => `\u001b[33m${s}\u001b[0m`;

export const formatDiagnostics = (diagnostics: Diagnostic[], content: string): string => {
  const lines = content.split(/\r?\n/);
  return diagnostics
    .map((diagnostic) => {
      const color = diagnostic.severity === "error" ? red : yellow;
      const lineText = lines[diagnostic.loc.line - 1] ?? "";
      const caretPos = Math.max(0, diagnostic.loc.col - 1);
      const underline = `${" ".repeat(caretPos)}^`;
      const header = `${diagnostic.loc.file}:${diagnostic.loc.line}:${diagnostic.loc.col} ${diagnostic.code} ${diagnostic.message}`;
      return `${color(header)}\n${lineText}\n${color(underline)}`;
    })
    .join("\n");
};
