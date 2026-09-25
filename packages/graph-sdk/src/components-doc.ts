import { componentCatalog } from "./catalog.js";

/** Prose for MDX: pipes escaped, and braces and angle brackets as entities (MDX reads `{` as
 * code and `<` as a tag). */
const prose = (text: string): string =>
  text
    .replace(/\|/g, "\\|")
    .replace(/\n/g, " ")
    .replace(/\{/g, "&#123;")
    .replace(/\}/g, "&#125;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");

/** A value inside backticks: only pipes need escaping in a table. */
const cell = (text: string): string => text.replace(/\|/g, "\\|").replace(/\n/g, " ");

/**
 * The component reference for the documentation site (`docs-site/docs/reference/_components.md`),
 * generated from {@link componentCatalog} so the docs list exactly the components and parameters
 * the SDK has. Regenerate with `pnpm --filter @ailu-ai/graph-sdk run gen:docs`.
 */
export function generateComponentsDoc(): string {
  const lines = [
    "{/* Generated from componentCatalog by `pnpm --filter @ailu-ai/graph-sdk run gen:docs`. Do not edit. */}",
    ""
  ];
  for (const entry of componentCatalog) {
    const factory = entry.integration ? `.node(id, components.${entry.kind}({...}))` : `.component(id, components.${entry.kind}({...}))`;
    lines.push(`### ${entry.kind}`, "");
    lines.push(`${prose(entry.description)} Category: ${entry.category}. Use: \`${factory}\`.`, "");
    lines.push("| Parameter | Type | Required | Description |", "| --- | --- | --- | --- |");
    for (const param of entry.params) {
      lines.push(
        `| \`${param.name}\` | \`${cell(param.type)}\` | ${param.required ? "yes" : "no"} | ${prose(param.description)} |`
      );
    }
    lines.push("");
  }
  return lines.join("\n");
}
