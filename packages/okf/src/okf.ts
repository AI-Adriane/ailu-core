/**
 * Open Knowledge Format (OKF) parse + serialize — a dependency-free reader/writer for the
 * markdown-with-YAML-frontmatter convention (https://github.com/GoogleCloudPlatform/knowledge-catalog).
 * OKF frontmatter is a shallow YAML map (scalars + string lists), so a small subset parser
 * is enough and avoids pulling a YAML dependency. Unknown frontmatter keys are preserved
 * verbatim in `frontmatter` so a bundle round-trips losslessly on export.
 *
 * This is the engine-owned home of the format (`@ailu/okf`); the control plane and
 * the polyglot engine crate (`ailu-okf`) consume the same definition.
 */

/** The structured view of one parsed OKF document. */
export type ParsedOkf = {
  /** OKF's only required field; defaults to `"document"` when absent. */
  type: string;
  title?: string;
  description?: string;
  resource?: string;
  /** ISO 8601 string from frontmatter, kept verbatim. */
  timestamp?: string;
  tags?: string[];
  /** Markdown cross-references (bundle-relative / relative), the untyped graph edges. */
  links?: string[];
  /**
   * TYPED graph edges from an OKF frontmatter `relations` convention — a string list of
   * `"<type>:<target>"` (e.g. `depends-on:/runtime/checkpointing.md`). Kept as a string
   * list (not nested objects) so the dependency-free frontmatter parser handles it.
   */
  relations?: Array<{ type: string; target: string }>;
  /** Extra producer frontmatter keys, preserved for round-trip. */
  frontmatter?: Record<string, unknown>;
  /** The markdown body (frontmatter stripped). */
  body: string;
};

const KNOWN_KEYS = new Set([
  "type",
  "title",
  "description",
  "resource",
  "tags",
  "timestamp",
  "links",
  "relations",
  "okf_version"
]);

/** Parse a `relations` string list (`"<type>:<target>"`) into typed edges. */
const parseRelations = (value: unknown): Array<{ type: string; target: string }> | undefined => {
  if (!Array.isArray(value)) {
    return undefined;
  }
  const relations = value
    .filter((item): item is string => typeof item === "string")
    .map((item) => {
      const colon = item.indexOf(":");
      if (colon <= 0) {
        return undefined;
      }
      return { type: item.slice(0, colon).trim(), target: item.slice(colon + 1).trim() };
    })
    .filter(
      (relation): relation is { type: string; target: string } =>
        relation !== undefined && relation.target.length > 0
    );
  return relations.length > 0 ? relations : undefined;
};

/** Strip a single layer of matching surrounding quotes. */
const unquote = (raw: string): string => {
  const value = raw.trim();
  if (
    value.length >= 2 &&
    ((value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'")))
  ) {
    return value.slice(1, -1);
  }
  return value;
};

/** Parse a shallow YAML frontmatter block (scalars, inline `[a, b]` and block `- item` lists). */
const parseFrontmatterBlock = (block: string): Record<string, unknown> => {
  const out: Record<string, unknown> = {};
  const lines = block.split("\n");
  let i = 0;
  while (i < lines.length) {
    const line = lines[i] ?? "";
    i += 1;
    const trimmed = line.trim();
    if (trimmed === "" || trimmed.startsWith("#")) {
      continue;
    }
    const match = /^([A-Za-z0-9_-]+):\s*(.*)$/.exec(line);
    if (match === null) {
      continue;
    }
    const key = match[1] ?? "";
    const rest = match[2] ?? "";
    if (key === "") {
      continue;
    }
    if (rest === "") {
      const items: string[] = [];
      while (i < lines.length && /^\s*-\s+/.test(lines[i] ?? "")) {
        items.push(unquote((lines[i] ?? "").replace(/^\s*-\s+/, "")));
        i += 1;
      }
      out[key] = items;
    } else if (rest.startsWith("[") && rest.endsWith("]")) {
      out[key] = rest
        .slice(1, -1)
        .split(",")
        .map((part) => unquote(part))
        .filter((part) => part.length > 0);
    } else {
      out[key] = unquote(rest);
    }
  }
  return out;
};

/**
 * Collect bundle-relative / relative markdown links from a body (external http links
 * skipped). Hand-rolled single pass (no regex) — `[label](target)` where the label has
 * no `]` and the target no `)`. Linear in the body length, so there is no
 * regex-backtracking / ReDoS exposure on untrusted markdown.
 */
export const extractLinks = (body: string): string[] => {
  const links = new Set<string>();
  const n = body.length;
  let i = 0;
  while (i < n) {
    if (body[i] !== "[") {
      i += 1;
      continue;
    }
    let j = i + 1;
    while (j < n && body[j] !== "]") {
      j += 1;
    }
    if (j + 1 < n && body[j] === "]" && body[j + 1] === "(") {
      let k = j + 2;
      while (k < n && body[k] !== ")") {
        k += 1;
      }
      if (k < n) {
        const target = body.slice(j + 2, k).trim();
        if (target.length > 0 && !target.startsWith("http://") && !target.startsWith("https://")) {
          links.add(target);
        }
        i = k + 1;
        continue;
      }
    }
    i += 1;
  }
  return [...links];
};

const asString = (value: unknown): string | undefined =>
  typeof value === "string" && value.length > 0 ? value : undefined;
const asStringArray = (value: unknown): string[] | undefined =>
  Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : undefined;

/** Parse one OKF document (frontmatter + body). Frontmatter is optional; missing → defaults. */
export const parseOkfDocument = (raw: string): ParsedOkf => {
  const normalized = raw.replace(/\r\n/g, "\n");
  let frontmatter: Record<string, unknown> = {};
  let body = normalized;

  if (normalized.startsWith("---\n")) {
    const close = normalized.indexOf("\n---", 3);
    if (close !== -1) {
      frontmatter = parseFrontmatterBlock(normalized.slice(4, close));
      const newlineAfterClose = normalized.indexOf("\n", close + 1);
      body = newlineAfterClose === -1 ? "" : normalized.slice(newlineAfterClose + 1);
    }
  }

  const extras: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(frontmatter)) {
    if (!KNOWN_KEYS.has(key)) {
      extras[key] = value;
    }
  }

  const bodyTrimmed = body.trim();
  const links = [...new Set([...(asStringArray(frontmatter.links) ?? []), ...extractLinks(bodyTrimmed)])];

  return {
    type: asString(frontmatter.type) ?? "document",
    title: asString(frontmatter.title),
    description: asString(frontmatter.description),
    resource: asString(frontmatter.resource),
    timestamp: asString(frontmatter.timestamp),
    tags: asStringArray(frontmatter.tags),
    links: links.length > 0 ? links : undefined,
    relations: parseRelations(frontmatter.relations),
    frontmatter: Object.keys(extras).length > 0 ? extras : undefined,
    body: bodyTrimmed
  };
};

/** Quote a scalar only when YAML would need it (contains `:` / `#` / leading-trailing space). */
const yamlScalar = (value: string): string => {
  if (value === "" || /[:#]|^\s|\s$|^[[{]/.test(value)) {
    return `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
  }
  return value;
};

const yamlValue = (value: unknown): string => {
  if (typeof value === "string") {
    return yamlScalar(value);
  }
  if (Array.isArray(value)) {
    return `[${value
      .map((item) => (typeof item === "string" ? yamlScalar(item) : JSON.stringify(item)))
      .join(", ")}]`;
  }
  return JSON.stringify(value);
};

/** The fields a stored document exposes for OKF serialization. */
export type SerializableOkf = {
  type: string;
  title?: string | null;
  description?: string | null;
  resource?: string | null;
  timestamp?: string | null;
  tags?: string[] | null;
  frontmatter?: Record<string, unknown> | null;
  body: string;
};

/** Serialize a stored document back to an OKF markdown file (frontmatter + body). */
export const serializeOkfDocument = (doc: SerializableOkf): string => {
  const lines: string[] = ["---", `type: ${yamlScalar(doc.type)}`];
  if (doc.title != null && doc.title.length > 0) lines.push(`title: ${yamlScalar(doc.title)}`);
  if (doc.description != null && doc.description.length > 0)
    lines.push(`description: ${yamlScalar(doc.description)}`);
  if (doc.resource != null && doc.resource.length > 0)
    lines.push(`resource: ${yamlScalar(doc.resource)}`);
  if (doc.tags != null && doc.tags.length > 0) {
    lines.push("tags:");
    for (const tag of doc.tags) lines.push(`  - ${yamlScalar(tag)}`);
  }
  if (doc.timestamp != null && doc.timestamp.length > 0)
    lines.push(`timestamp: ${yamlScalar(doc.timestamp)}`);
  if (doc.frontmatter != null) {
    for (const [key, value] of Object.entries(doc.frontmatter)) lines.push(`${key}: ${yamlValue(value)}`);
  }
  lines.push("---", "");
  return `${lines.join("\n")}${doc.body}\n`;
};

/** OKF reserves `index.md` (navigation) and `log.md` (change history) — skipped on ingest. */
export const isReservedOkfFile = (path: string): boolean => {
  const base = path.split("/").pop() ?? path;
  return base === "index.md" || base === "log.md";
};

/** Generate an OKF `index.md` navigation listing for a set of exported documents. */
export const buildOkfIndex = (
  namespace: string,
  entries: Array<{ path: string; title?: string | null; description?: string | null }>
): string => {
  const lines = [`# ${namespace}`, ""];
  for (const entry of entries) {
    const title = entry.title != null && entry.title.length > 0 ? entry.title : entry.path;
    const suffix =
      entry.description != null && entry.description.length > 0 ? ` - ${entry.description}` : "";
    lines.push(`* [${title}](/${entry.path})${suffix}`);
  }
  return `${lines.join("\n")}\n`;
};
