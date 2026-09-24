//! Open Knowledge Format (OKF) parse + serialize — a dependency-free reader/writer for
//! the markdown-with-YAML-frontmatter convention. Rust port of `@ailu-ai/okf`,
//! byte-compatible with the TypeScript implementation (the control plane and the
//! polyglot SDKs share one format definition). No YAML or regex dependency: OKF
//! frontmatter is a shallow map (scalars + string lists), parsed by a small subset
//! reader; markdown links are scanned by hand. Unknown frontmatter keys round-trip in
//! `frontmatter`.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A typed graph edge from the OKF `relations` frontmatter convention.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    #[serde(rename = "type")]
    pub relation_type: String,
    pub target: String,
}

/// The structured view of one parsed OKF document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedOkf {
    /// OKF's only required field; defaults to `"document"` when absent.
    #[serde(rename = "type")]
    pub doc_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Markdown cross-references (relative), the untyped graph edges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<String>>,
    /// Typed graph edges from the `relations` frontmatter convention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relations: Option<Vec<Relation>>,
    /// Extra producer frontmatter keys, preserved for round-trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontmatter: Option<BTreeMap<String, Value>>,
    /// The markdown body (frontmatter stripped).
    pub body: String,
}

/// The fields a stored document exposes for OKF serialization.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SerializableOkf {
    pub doc_type: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub resource: Option<String>,
    pub timestamp: Option<String>,
    pub tags: Option<Vec<String>>,
    pub frontmatter: Option<BTreeMap<String, Value>>,
    pub body: String,
}

const KNOWN_KEYS: [&str; 9] = [
    "type",
    "title",
    "description",
    "resource",
    "tags",
    "timestamp",
    "links",
    "relations",
    "okf_version",
];

/// Strip a single layer of matching surrounding quotes (after trimming).
fn unquote(raw: &str) -> String {
    let value = raw.trim();
    let mut chars = value.chars();
    if value.len() >= 2 {
        let first = chars.next();
        let last = value.chars().next_back();
        if (first == Some('"') && last == Some('"')) || (first == Some('\'') && last == Some('\''))
        {
            return value[1..value.len() - 1].to_owned();
        }
    }
    value.to_owned()
}

/// Match a frontmatter `key: rest` line anchored at column 0 (mirrors the TS
/// `^([A-Za-z0-9_-]+):\s*(.*)$`). Returns `(key, rest)` where `rest` has its leading
/// whitespace stripped.
fn parse_key_line(line: &str) -> Option<(String, &str)> {
    let colon = line.find(':')?;
    let key = &line[..colon];
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some((key.to_owned(), line[colon + 1..].trim_start()))
}

/// True when `line` is a block-list item (`^\s*-\s+`).
fn is_block_list_item(line: &str) -> bool {
    let trimmed = line.trim_start();
    let mut chars = trimmed.chars();
    chars.next() == Some('-') && matches!(chars.next(), Some(c) if c.is_whitespace())
}

/// Strip the `^\s*-\s+` marker from a block-list item, returning the value text.
fn strip_block_list_marker(line: &str) -> &str {
    let trimmed = line.trim_start();
    trimmed[1..].trim_start()
}

/// Parse a shallow YAML frontmatter block (scalars, inline `[a, b]`, block `- item`).
fn parse_frontmatter_block(block: &str) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    let lines: Vec<&str> = block.split('\n').collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, rest)) = parse_key_line(line) else {
            continue;
        };
        if rest.is_empty() {
            let mut items = Vec::new();
            while i < lines.len() && is_block_list_item(lines[i]) {
                items.push(Value::String(unquote(strip_block_list_marker(lines[i]))));
                i += 1;
            }
            out.insert(key, Value::Array(items));
        } else if rest.starts_with('[') && rest.ends_with(']') {
            let inner = &rest[1..rest.len() - 1];
            let items: Vec<Value> = inner
                .split(',')
                .map(unquote)
                .filter(|part| !part.is_empty())
                .map(Value::String)
                .collect();
            out.insert(key, Value::Array(items));
        } else {
            out.insert(key, Value::String(unquote(rest)));
        }
    }
    out
}

/// Collect relative markdown links from a body (external http(s) links skipped), in
/// first-seen order. Matches `[label](target)` with no `]` inside the label and no `)`
/// inside the target (mirrors the TS `LINK_RE`).
pub fn extract_links(body: &str) -> Vec<String> {
    let chars: Vec<char> = body.chars().collect();
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '[' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < chars.len() && chars[j] != ']' {
            j += 1;
        }
        if j + 1 < chars.len() && chars[j] == ']' && chars[j + 1] == '(' {
            let mut k = j + 2;
            let mut target = String::new();
            while k < chars.len() && chars[k] != ')' {
                target.push(chars[k]);
                k += 1;
            }
            if k < chars.len() {
                let trimmed = target.trim();
                if !trimmed.is_empty()
                    && !trimmed.starts_with("http://")
                    && !trimmed.starts_with("https://")
                    && seen.insert(trimmed.to_owned())
                {
                    out.push(trimmed.to_owned());
                }
                i = k + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn as_string(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

fn as_string_array(value: Option<&Value>) -> Option<Vec<String>> {
    match value {
        Some(Value::Array(items)) => Some(
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect(),
        ),
        _ => None,
    }
}

/// Parse a `relations` string list (`"<type>:<target>"`) into typed edges.
fn parse_relations(value: Option<&Value>) -> Option<Vec<Relation>> {
    let Some(Value::Array(items)) = value else {
        return None;
    };
    let relations: Vec<Relation> = items
        .iter()
        .filter_map(|item| item.as_str())
        .filter_map(|item| {
            let colon = item.find(':')?;
            if colon == 0 {
                return None;
            }
            let target = item[colon + 1..].trim().to_owned();
            if target.is_empty() {
                return None;
            }
            Some(Relation {
                relation_type: item[..colon].trim().to_owned(),
                target,
            })
        })
        .collect();
    (!relations.is_empty()).then_some(relations)
}

/// Parse one OKF document (frontmatter + body). Frontmatter is optional; missing →
/// defaults. Byte-compatible with the TS `parseOkfDocument`.
pub fn parse_okf_document(raw: &str) -> ParsedOkf {
    let normalized = raw.replace("\r\n", "\n");
    let mut frontmatter: BTreeMap<String, Value> = BTreeMap::new();
    let mut body = normalized.clone();

    if normalized.starts_with("---\n") {
        if let Some(rel) = normalized[3..].find("\n---") {
            let close = rel + 3;
            frontmatter = parse_frontmatter_block(&normalized[4..close]);
            match normalized[close + 1..].find('\n') {
                Some(nl) => body = normalized[close + 1 + nl + 1..].to_owned(),
                None => body = String::new(),
            }
        }
    }

    let mut extras: BTreeMap<String, Value> = BTreeMap::new();
    for (key, value) in &frontmatter {
        if !KNOWN_KEYS.contains(&key.as_str()) {
            extras.insert(key.clone(), value.clone());
        }
    }

    let body_trimmed = body.trim().to_owned();

    let mut links: Vec<String> = Vec::new();
    let mut seen = BTreeSet::new();
    for link in as_string_array(frontmatter.get("links"))
        .unwrap_or_default()
        .into_iter()
        .chain(extract_links(&body_trimmed))
    {
        if seen.insert(link.clone()) {
            links.push(link);
        }
    }

    ParsedOkf {
        doc_type: as_string(frontmatter.get("type")).unwrap_or_else(|| "document".to_owned()),
        title: as_string(frontmatter.get("title")),
        description: as_string(frontmatter.get("description")),
        resource: as_string(frontmatter.get("resource")),
        timestamp: as_string(frontmatter.get("timestamp")),
        tags: as_string_array(frontmatter.get("tags")),
        links: (!links.is_empty()).then_some(links),
        relations: parse_relations(frontmatter.get("relations")),
        frontmatter: (!extras.is_empty()).then_some(extras),
        body: body_trimmed,
    }
}

/// Quote a scalar only when YAML would need it.
fn yaml_scalar(value: &str) -> String {
    let needs_quote = value.is_empty()
        || value.contains(':')
        || value.contains('#')
        || value.starts_with(char::is_whitespace)
        || value.ends_with(char::is_whitespace)
        || value.starts_with('[')
        || value.starts_with('{');
    if needs_quote {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_owned()
    }
}

fn yaml_value(value: &Value) -> String {
    match value {
        Value::String(s) => yaml_scalar(s),
        Value::Array(items) => {
            let parts: Vec<String> = items
                .iter()
                .map(|item| match item {
                    Value::String(s) => yaml_scalar(s),
                    other => serde_json::to_string(other).unwrap_or_default(),
                })
                .collect();
            format!("[{}]", parts.join(", "))
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// Serialize a stored document back to an OKF markdown file (frontmatter + body).
/// Byte-compatible with the TS `serializeOkfDocument`.
pub fn serialize_okf_document(doc: &SerializableOkf) -> String {
    let mut lines: Vec<String> = vec![
        "---".to_owned(),
        format!("type: {}", yaml_scalar(&doc.doc_type)),
    ];
    if let Some(title) = doc.title.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("title: {}", yaml_scalar(title)));
    }
    if let Some(description) = doc.description.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("description: {}", yaml_scalar(description)));
    }
    if let Some(resource) = doc.resource.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("resource: {}", yaml_scalar(resource)));
    }
    if let Some(tags) = doc.tags.as_ref().filter(|t| !t.is_empty()) {
        lines.push("tags:".to_owned());
        for tag in tags {
            lines.push(format!("  - {}", yaml_scalar(tag)));
        }
    }
    if let Some(timestamp) = doc.timestamp.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("timestamp: {}", yaml_scalar(timestamp)));
    }
    if let Some(frontmatter) = doc.frontmatter.as_ref() {
        for (key, value) in frontmatter {
            lines.push(format!("{key}: {}", yaml_value(value)));
        }
    }
    lines.push("---".to_owned());
    lines.push(String::new());
    format!("{}{}\n", lines.join("\n"), doc.body)
}

/// OKF reserves `index.md` (navigation) and `log.md` (change history) — skipped on ingest.
pub fn is_reserved_okf_file(path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path);
    base == "index.md" || base == "log.md"
}

/// One entry of an OKF `index.md` navigation listing.
#[derive(Clone, Debug, Default)]
pub struct IndexEntry {
    pub path: String,
    pub title: Option<String>,
    pub description: Option<String>,
}

/// Generate an OKF `index.md` navigation listing for a set of exported documents.
pub fn build_okf_index(namespace: &str, entries: &[IndexEntry]) -> String {
    let mut lines = vec![format!("# {namespace}"), String::new()];
    for entry in entries {
        let title = entry
            .title
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&entry.path);
        let suffix = entry
            .description
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|d| format!(" - {d}"))
            .unwrap_or_default();
        lines.push(format!("* [{title}](/{}){suffix}", entry.path));
    }
    format!("{}\n", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_type_and_trims_body_without_frontmatter() {
        let doc = parse_okf_document("\n# Hello\n\nbody text\n");
        assert_eq!(doc.doc_type, "document");
        assert_eq!(doc.body, "# Hello\n\nbody text");
        assert!(doc.title.is_none());
    }

    #[test]
    fn reads_scalars_block_lists_and_inline_lists() {
        let raw = "---\ntype: note\ntitle: Checkpointing\ntags:\n  - runtime\n  - determinism\nlinks: [./a.md, ./b.md]\n---\nBody.";
        let doc = parse_okf_document(raw);
        assert_eq!(doc.doc_type, "note");
        assert_eq!(doc.title.as_deref(), Some("Checkpointing"));
        assert_eq!(
            doc.tags,
            Some(vec!["runtime".to_owned(), "determinism".to_owned()])
        );
        assert_eq!(
            doc.links,
            Some(vec!["./a.md".to_owned(), "./b.md".to_owned()])
        );
        assert_eq!(doc.body, "Body.");
    }

    #[test]
    fn parses_typed_relations_and_skips_malformed() {
        let raw = "---\ntype: note\nrelations:\n  - depends-on:/runtime/checkpointing.md\n  - references:/runtime/gates.md\n  - malformed\n---\nx";
        let doc = parse_okf_document(raw);
        assert_eq!(
            doc.relations,
            Some(vec![
                Relation {
                    relation_type: "depends-on".to_owned(),
                    target: "/runtime/checkpointing.md".to_owned()
                },
                Relation {
                    relation_type: "references".to_owned(),
                    target: "/runtime/gates.md".to_owned()
                },
            ])
        );
    }

    #[test]
    fn merges_body_links_and_skips_external() {
        let raw =
            "---\ntype: note\n---\nSee [a](./a.md) and [ext](https://x.com) and [b](../b.md).";
        let doc = parse_okf_document(raw);
        assert_eq!(
            doc.links,
            Some(vec!["./a.md".to_owned(), "../b.md".to_owned()])
        );
    }

    #[test]
    fn preserves_unknown_frontmatter_keys() {
        let doc = parse_okf_document("---\ntype: note\nauthor: alice\n---\nx");
        let extras = doc.frontmatter.expect("extras");
        assert_eq!(extras.get("author"), Some(&json!("alice")));
    }

    #[test]
    fn round_trips_through_serialize_then_parse() {
        let doc = SerializableOkf {
            doc_type: "note".to_owned(),
            title: Some("T".to_owned()),
            description: Some("D".to_owned()),
            tags: Some(vec!["a".to_owned(), "b".to_owned()]),
            timestamp: Some("2026-01-01T00:00:00Z".to_owned()),
            frontmatter: Some(
                [("author".to_owned(), json!("alice"))]
                    .into_iter()
                    .collect(),
            ),
            body: "Hello body".to_owned(),
            ..SerializableOkf::default()
        };
        let md = serialize_okf_document(&doc);
        let reparsed = parse_okf_document(&md);
        assert_eq!(reparsed.doc_type, "note");
        assert_eq!(reparsed.title.as_deref(), Some("T"));
        assert_eq!(reparsed.description.as_deref(), Some("D"));
        assert_eq!(reparsed.tags, Some(vec!["a".to_owned(), "b".to_owned()]));
        assert_eq!(reparsed.timestamp.as_deref(), Some("2026-01-01T00:00:00Z"));
        assert_eq!(reparsed.body, "Hello body");
        assert_eq!(
            reparsed.frontmatter.and_then(|f| f.get("author").cloned()),
            Some(json!("alice"))
        );
    }

    #[test]
    fn quotes_scalars_yaml_would_misread() {
        let md = serialize_okf_document(&SerializableOkf {
            doc_type: "note".to_owned(),
            title: Some("a: b # c".to_owned()),
            body: "x".to_owned(),
            ..SerializableOkf::default()
        });
        assert!(md.contains("title: \"a: b # c\""));
    }

    #[test]
    fn reserved_files_and_index() {
        assert!(is_reserved_okf_file("dir/index.md"));
        assert!(is_reserved_okf_file("log.md"));
        assert!(!is_reserved_okf_file("notes/topic.md"));

        let index = build_okf_index(
            "kb",
            &[
                IndexEntry {
                    path: "a.md".to_owned(),
                    title: Some("Alpha".to_owned()),
                    description: Some("first".to_owned()),
                },
                IndexEntry {
                    path: "b.md".to_owned(),
                    ..IndexEntry::default()
                },
            ],
        );
        assert!(index.contains("# kb"));
        assert!(index.contains("* [Alpha](/a.md) - first"));
        assert!(index.contains("* [b.md](/b.md)"));
    }

    #[test]
    fn json_wire_uses_type_and_camel_case() {
        let doc = parse_okf_document("---\ntype: note\ntitle: T\n---\nbody");
        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("\"type\":\"note\""));
        assert!(json.contains("\"title\":\"T\""));
    }
}
