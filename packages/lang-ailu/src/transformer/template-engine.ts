import type { Loc } from "../ast/types.js";
import type { Diagnostic } from "../validator/types.js";

const TOKEN_REGEX = /\{\{\s*([^}]+)\s*\}\}/g;

const truncate = (value: string, max: number): string => (value.length <= max ? value : value.slice(0, max));

export const renderTemplate = (
  template: string,
  variables: Record<string, unknown>,
  loc: Loc
): { content: string; diagnostics: Diagnostic[] } => {
  const diagnostics: Diagnostic[] = [];
  const content = template.replace(TOKEN_REGEX, (_full, rawExpr: string) => {
    const expr = rawExpr.trim();
    const [namePart, filterPart] = expr.split("|").map((part) => part.trim());
    if (namePart === undefined || namePart.length === 0) {
      return "";
    }
    const rawValue = variables[namePart];
    if (rawValue === undefined) {
      diagnostics.push({
        code: "UNRESOLVED_VARIABLE",
        message: `Variable '${namePart}' is not resolved.`,
        loc,
        severity: "warning"
      });
      return "";
    }
    let value = String(rawValue);
    if (filterPart?.startsWith("truncate:")) {
      const amountRaw = filterPart.slice("truncate:".length).trim();
      const amount = Number.parseInt(amountRaw, 10);
      if (Number.isFinite(amount) && amount >= 0) {
        value = truncate(value, amount);
      }
    }
    return value;
  });
  return { content, diagnostics };
};

export const detectUnresolvedTemplateVariables = (
  template: string,
  declaredVariables: string[],
  loc: Loc
): Diagnostic[] => {
  const declared = new Set(declaredVariables);
  const diagnostics: Diagnostic[] = [];
  for (const match of template.matchAll(TOKEN_REGEX)) {
    const rawExpr = match[1];
    if (rawExpr === undefined) {
      continue;
    }
    const name = rawExpr.split("|")[0]?.trim();
    if (name !== undefined && name.length > 0 && !declared.has(name)) {
      diagnostics.push({
        code: "UNDECLARED_TEMPLATE_VARIABLE",
        message: `Variable '${name}' is used but not declared in prompt variables.`,
        loc,
        severity: "warning"
      });
    }
  }
  return diagnostics;
};
