//! The component library: pure (no-LLM) compute building blocks, addressable by
//! a string `kind` plus a `serde_json` `params` object.
//!
//! A graph node can declare `{ component: { kind, params } }`; the engine then
//! asks [`ComponentRegistry::build_handler`] for the runtime [`NodeHandler`] that
//! reads `state.channels` together with its `params` and returns a channel-update
//! map. Building validates `kind` and `params` up front so a misconfigured graph
//! fails fast (at build time) rather than inside the running node.
//!
//! Every component here is deterministic and free of I/O and LLM calls.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ailu_graph_core::GraphState;
use ailu_graph_runtime::{sync_handler, NodeHandler, NodeOutput};
use ailu_rag_pipeline::{cosine_similarity, Document};
use serde_json::{json, Value};

use crate::error::ComponentError;

/// Builds runtime node handlers from a component `kind` + `params`.
///
/// Stateless: it is a namespace for [`build_handler`](ComponentRegistry::build_handler)
/// and the list of known kinds. A graph compiler calls `build_handler` once per
/// component node and registers the returned handler under the node id.
#[derive(Clone, Copy, Debug, Default)]
pub struct ComponentRegistry;

impl ComponentRegistry {
    /// Create a registry.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// The kinds this registry knows how to build, in a stable order.
    #[must_use]
    pub fn kinds() -> &'static [&'static str] {
        &[
            "promptBuilder",
            "jsonValidator",
            "outputParser",
            "router",
            "retriever",
            "semanticRetriever",
            "reranker",
            "textCleaner",
            "documentSplitter",
            "htmlToText",
            "csvParser",
            "documentJoiner",
            "deduplicator",
            "truncator",
            "regexExtractor",
            "answerBuilder",
            "fieldMapper",
            "fieldExtractor",
            // --- wave two: Haystack-gap coverage (all pure, deterministic) ---
            "bm25Retriever",
            "keywordRetriever",
            "sentenceWindowSplitter",
            "languageDetector",
            "metadataFilter",
            "listJoiner",
            "mergeRanker",
            "evaluator",
            "chatMessageBuilder",
            "conditionalRouter",
            "documentWriter",
            // --- council (ADR 0061 E2) ---
            "councilAnonymize",
            "councilAggregate",
        ]
    }

    /// Build the runtime [`NodeHandler`] for a component.
    ///
    /// Validates `params` for the given `kind` and returns a handler that, when
    /// driven with a [`GraphState`], reads the relevant channels and returns a
    /// channel-update [`NodeOutput`]. Returns a [`ComponentError`] if `kind` is
    /// unknown or `params` are invalid.
    pub fn build_handler(&self, kind: &str, params: &Value) -> Result<NodeHandler, ComponentError> {
        match kind {
            "promptBuilder" => build_prompt_builder(params),
            "jsonValidator" => build_json_validator(params),
            "outputParser" => build_output_parser(params),
            "router" => build_router(params),
            "retriever" => build_retriever(params),
            "semanticRetriever" => build_semantic_retriever(params),
            "reranker" => build_reranker(params),
            "textCleaner" => build_text_cleaner(params),
            "documentSplitter" => build_document_splitter(params),
            "htmlToText" => build_html_to_text(params),
            "csvParser" => build_csv_parser(params),
            "documentJoiner" => build_document_joiner(params),
            "deduplicator" => build_deduplicator(params),
            "truncator" => build_truncator(params),
            "regexExtractor" => build_regex_extractor(params),
            "answerBuilder" => build_answer_builder(params),
            "fieldMapper" => build_field_mapper(params),
            "fieldExtractor" => build_field_extractor(params),
            "bm25Retriever" => build_bm25_retriever(params),
            "keywordRetriever" => build_keyword_retriever(params),
            "sentenceWindowSplitter" => build_sentence_window_splitter(params),
            "languageDetector" => build_language_detector(params),
            "metadataFilter" => build_metadata_filter(params),
            "listJoiner" => build_list_joiner(params),
            "mergeRanker" => build_merge_ranker(params),
            "evaluator" => build_evaluator(params),
            "chatMessageBuilder" => build_chat_message_builder(params),
            "conditionalRouter" => build_conditional_router(params),
            "documentWriter" => build_document_writer(params),
            "councilAnonymize" => build_council_anonymize(params),
            "councilAggregate" => build_council_aggregate(params),
            other => Err(ComponentError::UnknownKind(other.to_string())),
        }
    }
}

// --- param helpers -----------------------------------------------------------

/// Read a required string param.
fn require_string(kind: &str, params: &Value, key: &str) -> Result<String, ComponentError> {
    match params.get(key) {
        None | Some(Value::Null) => Err(ComponentError::MissingParam {
            kind: kind.to_string(),
            param: key.to_string(),
        }),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(ComponentError::InvalidParam {
            kind: kind.to_string(),
            param: key.to_string(),
            reason: "expected a string".to_string(),
        }),
    }
}

/// Read an optional string param (absent or `null` -> `None`).
fn optional_string(
    kind: &str,
    params: &Value,
    key: &str,
) -> Result<Option<String>, ComponentError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(ComponentError::InvalidParam {
            kind: kind.to_string(),
            param: key.to_string(),
            reason: "expected a string".to_string(),
        }),
    }
}

/// Read an optional positive-integer param.
fn optional_usize(kind: &str, params: &Value, key: &str) -> Result<Option<usize>, ComponentError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            v.as_u64()
                .map(|n| Some(n as usize))
                .ok_or_else(|| ComponentError::InvalidParam {
                    kind: kind.to_string(),
                    param: key.to_string(),
                    reason: "expected a non-negative integer".to_string(),
                })
        }
    }
}

/// Read a required positive-integer param.
fn require_usize(kind: &str, params: &Value, key: &str) -> Result<usize, ComponentError> {
    match params.get(key) {
        None | Some(Value::Null) => Err(ComponentError::MissingParam {
            kind: kind.to_string(),
            param: key.to_string(),
        }),
        Some(v) => v
            .as_u64()
            .map(|n| n as usize)
            .ok_or_else(|| ComponentError::InvalidParam {
                kind: kind.to_string(),
                param: key.to_string(),
                reason: "expected a non-negative integer".to_string(),
            }),
    }
}

/// Read an optional boolean param (absent or `null` -> the given default).
fn optional_bool(
    kind: &str,
    params: &Value,
    key: &str,
    default: bool,
) -> Result<bool, ComponentError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(ComponentError::InvalidParam {
            kind: kind.to_string(),
            param: key.to_string(),
            reason: "expected a boolean".to_string(),
        }),
    }
}

/// Coerce a channel value to a string for templating/text components: strings
/// pass through unquoted; everything else is rendered as compact JSON. Mirrors
/// how the TS components stringify non-string channel values.
fn value_to_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Object(_) => {
            // An agent node writes an `AgentResult` object into its output channel. Its final answer
            // lives in `reasoning` as the last `final:<answer>` line (see agents-core react.rs). A
            // downstream component (answerBuilder, outputParser, …) reading that channel wants the
            // ANSWER, not the stringified wrapper — so unwrap it here (the universal text coercion).
            // Prefer `structuredOutput` when the agent ran with a schema; else the `final:` tail; else
            // fall through to the raw JSON for non-AgentResult objects.
            if let Some(text) = agent_result_text(value) {
                text
            } else {
                value.to_string()
            }
        }
        other => other.to_string(),
    }
}

/// Extract an `AgentResult`'s final answer text, or `None` when `value` is not an AgentResult-shaped
/// object. Uses `structuredOutput` (validated JSON) when present; otherwise the text after the last
/// `final:` marker in `reasoning`; otherwise the whole `reasoning`.
fn agent_result_text(value: &Value) -> Option<String> {
    let obj = value.as_object()?;
    let reasoning = obj.get("reasoning")?.as_str()?;
    if let Some(structured) = obj.get("structuredOutput") {
        if !structured.is_null() {
            return Some(match structured {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            });
        }
    }
    let answer = match reasoning.rfind("final:") {
        Some(idx) => reasoning[idx + "final:".len()..].trim(),
        None => reasoning.trim(),
    };
    Some(answer.to_string())
}

/// Build a single-key channel update.
fn single(channel: &str, value: Value) -> BTreeMap<String, Value> {
    let mut update = BTreeMap::new();
    update.insert(channel.to_string(), value);
    update
}

/// Millis-since-epoch as a decimal string — same convention as
/// `approval_engine::engine::now_string` (kept local here since `components` does not depend on
/// `approval-engine`). Used to timestamp discarded-candidate records (ADR 0044 D2, ailu#578).
fn now_millis_string() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
        .to_string()
}

// --- promptBuilder -----------------------------------------------------------

/// `promptBuilder { template, into, capsule? }` — render `{{var}}` placeholders
/// from the channels into the `into` channel, and (ADR 0044 D3, ailu#578)
/// optionally assemble a `RetrievalCapsule` capturing the exact set that
/// influenced the model: this is the ONE node that knows what text was
/// actually templated, so it is the right place to hash it.
///
/// `capsule.chunksFrom`/`queryFrom` are required, `discardedFrom` is optional
/// (defaults to none) — these three params are not literally named in the
/// ADR's D3 sketch, which only sketched the OUTPUT shape; filled in here
/// because a capsule-assembly node needs to be told explicitly which
/// upstream channel holds the query and which channels hold this pipeline's
/// discarded candidates (they can be scattered across several stages — D2's
/// `{into}Discarded` channels are stage-local, not a single sibling of
/// `chunksFrom`).
struct CapsuleConfig {
    into: String,
    chunks_from: String,
    query_from: String,
    discarded_from: Vec<String>,
}

fn build_prompt_builder(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "promptBuilder";
    let template = require_string(kind, params, "template")?;
    let into = require_string(kind, params, "into")?;
    let capsule = match params.get("capsule") {
        None | Some(Value::Null) => None,
        Some(c) => Some(CapsuleConfig {
            into: require_string(kind, c, "into")?,
            chunks_from: require_string(kind, c, "chunksFrom")?,
            query_from: require_string(kind, c, "queryFrom")?,
            discarded_from: optional_string_array(kind, c, "discardedFrom")?.unwrap_or_default(),
        }),
    };

    Ok(sync_handler(move |state: GraphState| {
        let rendered = render_template(&template, &state.channels);
        let mut update = single(&into, Value::String(rendered.clone()));
        if let Some(cfg) = &capsule {
            update.insert(
                cfg.into.clone(),
                build_retrieval_capsule(cfg, &state.channels, &rendered),
            );
        }
        NodeOutput::update(update)
    }))
}

/// Read an optional array-of-strings param (absent or `null` -> `None`).
fn optional_string_array(
    kind: &str,
    params: &Value,
    key: &str,
) -> Result<Option<Vec<String>>, ComponentError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                match item.as_str() {
                    Some(s) => out.push(s.to_string()),
                    None => {
                        return Err(ComponentError::InvalidParam {
                            kind: kind.to_string(),
                            param: key.to_string(),
                            reason: format!("entry {i} is not a string"),
                        })
                    }
                }
            }
            Ok(Some(out))
        }
        Some(_) => Err(ComponentError::InvalidParam {
            kind: kind.to_string(),
            param: key.to_string(),
            reason: "expected an array of strings".to_string(),
        }),
    }
}

/// Assemble the `RetrievalCapsule` (ADR 0044 D3): `order`/`chunks` from the surviving
/// `chunksFrom` array (id + full provenance chain per chunk, D1), `discarded` concatenated
/// from every `discardedFrom` channel (D2's per-stage discard records), and `renderedHash`/
/// `queryHash` — sha256 over the EXACT rendered prompt text and the query text, respectively,
/// not over the raw channel values (the point is "did this exact text reach the model").
fn build_retrieval_capsule(
    cfg: &CapsuleConfig,
    channels: &BTreeMap<String, Value>,
    rendered: &str,
) -> Value {
    let chunks_arr = match channels.get(&cfg.chunks_from) {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    let order: Vec<Value> = chunks_arr
        .iter()
        .map(|item| item.get("id").cloned().unwrap_or(Value::Null))
        .collect();
    let chunks: Vec<Value> = chunks_arr
        .iter()
        .map(|item| {
            json!({
                "id": item.get("id").cloned().unwrap_or(Value::Null),
                "provenance": item.get("provenance").cloned().unwrap_or(Value::Array(Vec::new()))
            })
        })
        .collect();

    let mut discarded: Vec<Value> = Vec::new();
    for name in &cfg.discarded_from {
        if let Some(Value::Array(items)) = channels.get(name) {
            discarded.extend(items.iter().cloned());
        }
    }

    let query_text = channels
        .get(&cfg.query_from)
        .map(value_to_text)
        .unwrap_or_default();

    json!({
        "order": order,
        "chunks": chunks,
        "discarded": discarded,
        "renderedHash": sha256_hex(rendered),
        "queryHash": sha256_hex(&query_text)
    })
}

fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Replace every `{{ name }}` placeholder with the corresponding channel value
/// (coerced to text). Unknown placeholders render as the empty string. Whitespace
/// inside the braces is tolerated.
fn render_template(template: &str, channels: &BTreeMap<String, Value>) -> String {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            if let Some(close) = template[i + 2..].find("}}") {
                let raw = &template[i + 2..i + 2 + close];
                let name = raw.trim();
                let replacement = channels.get(name).map(value_to_text).unwrap_or_default();
                out.push_str(&replacement);
                i = i + 2 + close + 2;
                continue;
            }
        }
        // Not a placeholder start (or no closing braces): copy this character.
        let ch_len = template[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&template[i..i + ch_len]);
        i += ch_len;
    }
    out
}

// --- jsonValidator -----------------------------------------------------------

/// `jsonValidator { from, requiredKeys?, expectType?, okInto, errorsInto }` —
/// validate the `from` channel value, writing a bool into `okInto` and an errors
/// array into `errorsInto`.
fn build_json_validator(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "jsonValidator";
    let from = require_string(kind, params, "from")?;
    let ok_into = require_string(kind, params, "okInto")?;
    let errors_into = require_string(kind, params, "errorsInto")?;
    let expect_type = optional_string(kind, params, "expectType")?;

    // requiredKeys?: array of strings.
    let required_keys: Vec<String> = match params.get("requiredKeys") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => {
            let mut keys = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    Some(s) => keys.push(s.to_string()),
                    None => {
                        return Err(ComponentError::InvalidParam {
                            kind: kind.to_string(),
                            param: "requiredKeys".to_string(),
                            reason: "expected an array of strings".to_string(),
                        })
                    }
                }
            }
            keys
        }
        Some(_) => {
            return Err(ComponentError::InvalidParam {
                kind: kind.to_string(),
                param: "requiredKeys".to_string(),
                reason: "expected an array of strings".to_string(),
            })
        }
    };

    Ok(sync_handler(move |state: GraphState| {
        let value = state.channels.get(&from).cloned().unwrap_or(Value::Null);
        let mut errors: Vec<Value> = Vec::new();

        if let Some(expected) = &expect_type {
            let actual = json_type_name(&value);
            if actual != expected.as_str() {
                errors.push(Value::String(format!(
                    "expected type `{expected}` but got `{actual}`"
                )));
            }
        }

        if !required_keys.is_empty() {
            match value.as_object() {
                Some(map) => {
                    for key in &required_keys {
                        if !map.contains_key(key) {
                            errors.push(Value::String(format!("missing required key `{key}`")));
                        }
                    }
                }
                None => {
                    errors.push(Value::String(
                        "expected an object to check required keys".to_string(),
                    ));
                }
            }
        }

        let ok = errors.is_empty();
        let mut update = BTreeMap::new();
        update.insert(ok_into.clone(), Value::Bool(ok));
        update.insert(errors_into.clone(), Value::Array(errors));
        NodeOutput::update(update)
    }))
}

/// The JSON type name of a value, matching the `expectType` vocabulary:
/// `"string" | "number" | "boolean" | "object" | "array" | "null"`.
fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

// --- outputParser ------------------------------------------------------------

/// `outputParser { from, into }` — extract the first JSON object/array from a
/// text channel (tolerant of surrounding prose / code fences) and write the
/// parsed value to `into`. Writes `null` when no JSON value is found.
fn build_output_parser(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "outputParser";
    let from = require_string(kind, params, "from")?;
    let into = require_string(kind, params, "into")?;

    Ok(sync_handler(move |state: GraphState| {
        let raw = state
            .channels
            .get(&from)
            .map(value_to_text)
            .unwrap_or_default();
        let parsed = extract_first_json(&raw).unwrap_or(Value::Null);
        NodeOutput::update(single(&into, parsed))
    }))
}

/// Find the first balanced JSON object or array in `text` and parse it.
///
/// Scans for the first `{` or `[`, then walks forward tracking brace/bracket
/// depth (and skipping over string literals, including escaped quotes) until the
/// structure closes, then parses that slice. Returns `None` if nothing parses.
fn extract_first_json(text: &str) -> Option<Value> {
    let bytes = text.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{' || b == b'[')?;
    let open = bytes[start];
    let close = if open == b'{' { b'}' } else { b']' };

    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    let mut idx = start;
    while idx < bytes.len() {
        let b = bytes[idx];
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
        } else if b == b'"' {
            in_string = true;
        } else if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                let candidate = &text[start..=idx];
                return serde_json::from_str::<Value>(candidate).ok();
            }
        }
        idx += 1;
    }
    None
}

// --- router ------------------------------------------------------------------

/// One routing rule: match the `from` value, then emit `route`.
struct RouterRule {
    /// Exact (string) match against the `from` value.
    equals: Option<String>,
    /// Substring match against the textual form of the `from` value.
    contains: Option<String>,
    /// The route string to emit when this rule matches.
    route: String,
}

/// `router { from, rules:[{equals?|contains?, route}], defaultRoute, into }` —
/// pick a route string from the `from` channel value and write it to `into`
/// (pairs with a conditional edge keyed on that channel).
fn build_router(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "router";
    let from = require_string(kind, params, "from")?;
    let into = require_string(kind, params, "into")?;
    let default_route = require_string(kind, params, "defaultRoute")?;

    let rules_value = params
        .get("rules")
        .ok_or_else(|| ComponentError::MissingParam {
            kind: kind.to_string(),
            param: "rules".to_string(),
        })?;
    let rules_arr = rules_value
        .as_array()
        .ok_or_else(|| ComponentError::InvalidParam {
            kind: kind.to_string(),
            param: "rules".to_string(),
            reason: "expected an array of rules".to_string(),
        })?;

    let mut rules: Vec<RouterRule> = Vec::with_capacity(rules_arr.len());
    for (i, rule) in rules_arr.iter().enumerate() {
        let route = rule
            .get("route")
            .and_then(Value::as_str)
            .ok_or_else(|| ComponentError::InvalidParam {
                kind: kind.to_string(),
                param: "rules".to_string(),
                reason: format!("rule {i} is missing a string `route`"),
            })?
            .to_string();
        let equals = match rule.get("equals") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => {
                return Err(ComponentError::InvalidParam {
                    kind: kind.to_string(),
                    param: "rules".to_string(),
                    reason: format!("rule {i} `equals` must be a string"),
                })
            }
        };
        let contains = match rule.get("contains") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => {
                return Err(ComponentError::InvalidParam {
                    kind: kind.to_string(),
                    param: "rules".to_string(),
                    reason: format!("rule {i} `contains` must be a string"),
                })
            }
        };
        rules.push(RouterRule {
            equals,
            contains,
            route,
        });
    }
    let rules = Arc::new(rules);

    Ok(sync_handler(move |state: GraphState| {
        let value = state.channels.get(&from).cloned().unwrap_or(Value::Null);
        let text = value_to_text(&value);
        let chosen = rules
            .iter()
            .find(|rule| rule_matches(rule, &value, &text))
            .map(|rule| rule.route.clone())
            .unwrap_or_else(|| default_route.clone());
        NodeOutput::update(single(&into, Value::String(chosen)))
    }))
}

/// Whether a rule matches: `equals` checks the value's text form for exact
/// equality; `contains` checks for a substring. A rule with both must satisfy
/// both; a rule with neither never matches (fall through to the default).
fn rule_matches(rule: &RouterRule, _value: &Value, text: &str) -> bool {
    let mut had_predicate = false;
    if let Some(eq) = &rule.equals {
        had_predicate = true;
        if text != eq.as_str() {
            return false;
        }
    }
    if let Some(sub) = &rule.contains {
        had_predicate = true;
        if !text.contains(sub.as_str()) {
            return false;
        }
    }
    had_predicate
}

// --- retriever ---------------------------------------------------------------

/// `retriever { query, into, k, docs:[{id,content}] }` — embed the query and the
/// docs with the deterministic mock embeddings, score by cosine similarity, and
/// write the top-`k` `{ id, content, score }` results to `into`.
///
/// The embedding is the same deterministic 4-bucket count vector as
/// `ailu_rag_pipeline::MockEmbedder` (and the cosine scoring is its
/// [`cosine_similarity`]), reproduced inline so the handler stays synchronous
/// and free of async/I/O. `query` is a channel name; the query text is read from
/// that channel at run time (falling back to the literal param value if the
/// channel is absent).
fn build_retriever(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "retriever";
    let query = require_string(kind, params, "query")?;
    let into = require_string(kind, params, "into")?;
    let k = optional_usize(kind, params, "k")?.unwrap_or(4);

    let docs_value = params
        .get("docs")
        .ok_or_else(|| ComponentError::MissingParam {
            kind: kind.to_string(),
            param: "docs".to_string(),
        })?;
    let docs_arr = docs_value
        .as_array()
        .ok_or_else(|| ComponentError::InvalidParam {
            kind: kind.to_string(),
            param: "docs".to_string(),
            reason: "expected an array of { id, content }".to_string(),
        })?;

    let mut docs: Vec<Document> = Vec::with_capacity(docs_arr.len());
    for (i, doc) in docs_arr.iter().enumerate() {
        let id =
            doc.get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| ComponentError::InvalidParam {
                    kind: kind.to_string(),
                    param: "docs".to_string(),
                    reason: format!("doc {i} is missing a string `id`"),
                })?;
        let content = doc.get("content").and_then(Value::as_str).ok_or_else(|| {
            ComponentError::InvalidParam {
                kind: kind.to_string(),
                param: "docs".to_string(),
                reason: format!("doc {i} is missing a string `content`"),
            }
        })?;
        docs.push(Document::new(id, content));
    }
    let docs = Arc::new(docs);

    Ok(sync_handler(move |state: GraphState| {
        // The query channel holds the text; fall back to the literal param.
        let query_text = state
            .channels
            .get(&query)
            .map(value_to_text)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| query.clone());
        let query_vec = mock_embed(&query_text);

        let mut scored: Vec<(f64, &Document)> = docs
            .iter()
            .map(|doc| {
                (
                    cosine_similarity(&query_vec, &mock_embed(&doc.content)),
                    doc,
                )
            })
            .collect();
        // Descending by score; stable so input order breaks ties deterministically.
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);

        let results: Vec<Value> = scored
            .into_iter()
            .map(|(score, doc)| json!({ "id": doc.id, "content": doc.content, "score": score }))
            .collect();
        NodeOutput::update(single(&into, Value::Array(results)))
    }))
}

// --- semanticRetriever -------------------------------------------------------

/// Read a JSON array channel value into a dense `f64` vector (non-numbers dropped).
fn json_to_f64_vec(value: &Value) -> Vec<f64> {
    value
        .as_array()
        .map(|arr| arr.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// `semanticRetriever { queryEmbeddingFrom, chunksFrom, into, k? }` — rank a corpus of
/// PRE-EMBEDDED chunks by cosine similarity to a PRE-EMBEDDED query, both supplied on
/// channels, and write the top-`k` `{ id, content, score }` to `into`.
///
/// Unlike `retriever` (lexical mock embeddings over inline `docs`), this consumes REAL
/// embeddings — produced at ingestion by the gateway (e.g. Mistral) and persisted in the
/// knowledge base. Embedding is a provider/gateway concern; this component owns only the
/// cosine ranking (the engine's [`cosine_similarity`]). The host (control plane) seeds
/// `chunksFrom` with a namespace's persisted KB and `queryEmbeddingFrom` with the embedded
/// query, so the same component serves an in-memory OSS corpus or a Postgres-backed one.
fn build_semantic_retriever(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "semanticRetriever";
    let query_embedding_from = require_string(kind, params, "queryEmbeddingFrom")?;
    let chunks_from = require_string(kind, params, "chunksFrom")?;
    let into = require_string(kind, params, "into")?;
    let k = optional_usize(kind, params, "k")?.unwrap_or(4);

    Ok(sync_handler(move |state: GraphState| {
        let query_vec = state
            .channels
            .get(&query_embedding_from)
            .map(json_to_f64_vec)
            .unwrap_or_default();
        let chunks = state
            .channels
            .get(&chunks_from)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut scored: Vec<(f64, Value)> = chunks
            .iter()
            .filter_map(|chunk| {
                let embedding = json_to_f64_vec(chunk.get("embedding")?);
                let score = cosine_similarity(&query_vec, &embedding);
                let id = chunk.get("id").and_then(Value::as_str).unwrap_or("");
                let content = chunk.get("content").and_then(Value::as_str).unwrap_or("");
                Some((
                    score,
                    json!({ "id": id, "content": content, "score": score }),
                ))
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        let results: Vec<Value> = scored.into_iter().map(|(_, value)| value).collect();
        NodeOutput::update(single(&into, Value::Array(results)))
    }))
}

/// The deterministic 4-bucket count vector used by
/// `ailu_rag_pipeline::MockEmbedder`: bucket `c % 4` is incremented for every
/// character `c` (by Unicode code point). Reproduced here to keep the retriever
/// handler synchronous (the rag-pipeline `Embedder` trait is async).
fn mock_embed(text: &str) -> Vec<f64> {
    let mut counts = vec![0.0_f64; 4];
    for ch in text.chars() {
        let idx = (ch as u32 as usize) % 4;
        counts[idx] += 1.0;
    }
    counts
}

// --- reranker ----------------------------------------------------------------

/// `reranker { from, into, query? }` — reorder a retrieval-result array (the
/// shape the `retriever` component emits: `{ id, content, score }`) and write the
/// reordered array to `into`.
///
/// This is the fallback used when no cross-encoder is configured (the runtime bridge
/// routes the node through the `AILU_RERANK_ENDPOINT` cross-encoder when one is set).
/// Deterministic and no-LLM: items are sorted by their existing `score` (stable, so input
/// order is kept on ties; a missing/unusable score counts as `0`). `query` is accepted but
/// not used here — a real re-score needs the cross-encoder, and re-scoring with a
/// placeholder embedding would throw away the upstream (BM25 / RRF) ranking.
fn build_reranker(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "reranker";
    let from = require_string(kind, params, "from")?;
    let into = require_string(kind, params, "into")?;
    // Validated for shape (a non-string `query` is still a config error) but not used.
    let _query = optional_string(kind, params, "query")?;

    Ok(sync_handler(move |state: GraphState| {
        let items: Vec<Value> = match state.channels.get(&from) {
            Some(Value::Array(arr)) => arr.clone(),
            _ => Vec::new(),
        };

        let mut scored: Vec<(f64, Value)> = items
            .into_iter()
            .map(|item| {
                let score = item.get("score").and_then(Value::as_f64).unwrap_or(0.0);
                (score, item)
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let algorithm = "score-passthrough";
        let reordered: Vec<Value> = scored
            .into_iter()
            .map(|(score, mut item)| {
                // Surface the score back onto the item, and append a provenance step (ADR 0044,
                // ailu#578) instead of silently overwriting `score` with no record — same fix as
                // bm25Retriever/mergeRanker (D1) and the cross-encoder reranker (runtime-bridge's
                // build_reranker_node), applied here for the no-endpoint fallback path.
                if let Value::Object(map) = &mut item {
                    let score_value = serde_json::Number::from_f64(score)
                        .map(Value::Number)
                        .unwrap_or(Value::Null);
                    map.insert("score".to_string(), score_value.clone());
                    let mut provenance = match map.get("provenance") {
                        Some(Value::Array(steps)) => steps.clone(),
                        _ => Vec::new(),
                    };
                    provenance.push(json!({
                        "stage": "rerank",
                        "algorithm": algorithm,
                        "algorithmVersion": null,
                        "score": score_value
                    }));
                    map.insert("provenance".to_string(), Value::Array(provenance));
                }
                item
            })
            .collect();
        NodeOutput::update(single(&into, Value::Array(reordered)))
    }))
}

// --- textCleaner -------------------------------------------------------------

/// `textCleaner { from, into, lowercase?, stripHtml?, collapseWhitespace?, trim? }`
/// — normalise a text channel: optionally strip HTML tags, lowercase, collapse
/// runs of whitespace to a single space, and trim leading/trailing whitespace.
///
/// All transforms default to `false`. They are applied in a fixed order so the
/// result is deterministic regardless of param order: stripHtml → lowercase →
/// collapseWhitespace → trim. A missing `from` channel is treated as empty text.
fn build_text_cleaner(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "textCleaner";
    let from = require_string(kind, params, "from")?;
    let into = require_string(kind, params, "into")?;
    let lowercase = optional_bool(kind, params, "lowercase", false)?;
    let strip_html = optional_bool(kind, params, "stripHtml", false)?;
    let collapse_whitespace = optional_bool(kind, params, "collapseWhitespace", false)?;
    let trim = optional_bool(kind, params, "trim", false)?;

    Ok(sync_handler(move |state: GraphState| {
        let mut text = state
            .channels
            .get(&from)
            .map(value_to_text)
            .unwrap_or_default();
        if strip_html {
            text = strip_html_tags(&text);
        }
        if lowercase {
            text = text.to_lowercase();
        }
        if collapse_whitespace {
            text = collapse_ws(&text);
        }
        if trim {
            text = text.trim().to_string();
        }
        NodeOutput::update(single(&into, Value::String(text)))
    }))
}

/// Collapse every run of ASCII/Unicode whitespace into a single space.
fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Remove anything between `<` and the next `>` (a simple, deterministic tag
/// stripper — it does not parse nested or malformed markup beyond this rule).
fn strip_html_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

// --- documentSplitter --------------------------------------------------------

/// `documentSplitter { from, into, by:"chars"|"sentences", size, overlap? }` —
/// split a text channel into an array of chunk strings.
///
/// `by: "chars"` slices the text (by Unicode scalar value) into windows of
/// `size` characters advancing by `size - overlap` each step. `by: "sentences"`
/// first segments on sentence terminators (`.`/`!`/`?`), then packs whole
/// sentences greedily into chunks of at most `size` sentences, with `overlap`
/// sentences repeated at the start of the next chunk. `overlap` must be smaller
/// than `size`. Empty input yields an empty array.
fn build_document_splitter(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "documentSplitter";
    let from = require_string(kind, params, "from")?;
    let into = require_string(kind, params, "into")?;
    let by = require_string(kind, params, "by")?;
    let size = require_usize(kind, params, "size")?;
    let overlap = optional_usize(kind, params, "overlap")?.unwrap_or(0);

    if size == 0 {
        return Err(ComponentError::InvalidParam {
            kind: kind.to_string(),
            param: "size".to_string(),
            reason: "must be greater than zero".to_string(),
        });
    }
    if overlap >= size {
        return Err(ComponentError::InvalidParam {
            kind: kind.to_string(),
            param: "overlap".to_string(),
            reason: "must be smaller than `size`".to_string(),
        });
    }
    let by = match by.as_str() {
        "chars" | "sentences" => by,
        other => {
            return Err(ComponentError::InvalidParam {
                kind: kind.to_string(),
                param: "by".to_string(),
                reason: format!("expected \"chars\" or \"sentences\", got \"{other}\""),
            })
        }
    };

    Ok(sync_handler(move |state: GraphState| {
        let text = state
            .channels
            .get(&from)
            .map(value_to_text)
            .unwrap_or_default();
        let chunks: Vec<Value> = if by == "chars" {
            split_by_chars(&text, size, overlap)
        } else {
            split_by_sentences(&text, size, overlap)
        }
        .into_iter()
        .map(Value::String)
        .collect();
        NodeOutput::update(single(&into, Value::Array(chunks)))
    }))
}

/// Slice `text` into windows of `size` chars, stepping by `size - overlap`.
fn split_by_chars(text: &str, size: usize, overlap: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let step = size - overlap;
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + size).min(chars.len());
        chunks.push(chars[start..end].iter().collect());
        if end == chars.len() {
            break;
        }
        start += step;
    }
    chunks
}

/// Segment `text` into sentences, then pack `size` sentences per chunk with
/// `overlap` sentences repeated at the start of each subsequent chunk.
fn split_by_sentences(text: &str, size: usize, overlap: usize) -> Vec<String> {
    let sentences = segment_sentences(text);
    if sentences.is_empty() {
        return Vec::new();
    }
    let step = size - overlap;
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < sentences.len() {
        let end = (start + size).min(sentences.len());
        chunks.push(sentences[start..end].join(" "));
        if end == sentences.len() {
            break;
        }
        start += step;
    }
    chunks
}

/// Split text into trimmed, non-empty sentences on `.`/`!`/`?` terminators,
/// keeping the terminator attached to its sentence.
fn segment_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?') {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                sentences.push(trimmed.to_string());
            }
            current.clear();
        }
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        sentences.push(trimmed.to_string());
    }
    sentences
}

// --- htmlToText --------------------------------------------------------------

/// `htmlToText { from, into }` — strip HTML tags from a text channel and decode
/// the common named entities `&amp;`, `&lt;`, `&gt;`, and `&quot;`.
///
/// Tags are removed with the same simple `<…>` rule as `textCleaner`'s
/// `stripHtml`; entity decoding runs after tag removal. A missing `from` channel
/// is treated as empty text.
fn build_html_to_text(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "htmlToText";
    let from = require_string(kind, params, "from")?;
    let into = require_string(kind, params, "into")?;

    Ok(sync_handler(move |state: GraphState| {
        let raw = state
            .channels
            .get(&from)
            .map(value_to_text)
            .unwrap_or_default();
        let stripped = strip_html_tags(&raw);
        let decoded = decode_entities(&stripped);
        NodeOutput::update(single(&into, Value::String(decoded)))
    }))
}

/// Decode the common named HTML entities. `&amp;` is decoded last so an input
/// like `&amp;lt;` round-trips to `&lt;` rather than being double-decoded.
fn decode_entities(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

// --- csvParser ---------------------------------------------------------------

/// `csvParser { from, into, delimiter?, header? }` — parse a CSV text channel
/// into an array of rows.
///
/// `delimiter` defaults to `","` (must be a single character). With
/// `header: true` (the default) the first row supplies field names and each data
/// row becomes an object keyed by those names; with `header: false` each row
/// becomes an array of cell strings. This is a simple line/character splitter: it
/// does not handle quoted cells containing the delimiter or embedded newlines —
/// rows are split on `\n` and cells on the delimiter. Empty input yields an empty
/// array.
fn build_csv_parser(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "csvParser";
    let from = require_string(kind, params, "from")?;
    let into = require_string(kind, params, "into")?;
    let header = optional_bool(kind, params, "header", true)?;
    let delimiter = match optional_string(kind, params, "delimiter")? {
        None => ',',
        Some(s) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => c,
                _ => {
                    return Err(ComponentError::InvalidParam {
                        kind: kind.to_string(),
                        param: "delimiter".to_string(),
                        reason: "expected a single character".to_string(),
                    })
                }
            }
        }
    };

    Ok(sync_handler(move |state: GraphState| {
        let text = state
            .channels
            .get(&from)
            .map(value_to_text)
            .unwrap_or_default();
        let mut rows = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                line.split(delimiter)
                    .map(|cell| cell.trim().to_string())
                    .collect::<Vec<String>>()
            });

        let parsed: Vec<Value> = if header {
            let headers = rows.next().unwrap_or_default();
            rows.map(|cells| {
                let mut obj = serde_json::Map::new();
                for (i, name) in headers.iter().enumerate() {
                    let cell = cells.get(i).cloned().unwrap_or_default();
                    obj.insert(name.clone(), Value::String(cell));
                }
                Value::Object(obj)
            })
            .collect()
        } else {
            rows.map(|cells| Value::Array(cells.into_iter().map(Value::String).collect()))
                .collect()
        };
        NodeOutput::update(single(&into, Value::Array(parsed)))
    }))
}

// --- documentJoiner ----------------------------------------------------------

/// `documentJoiner { fromChannels:[..], into, dedupeBy? }` — concatenate the
/// array values found across several channels into one merged array (in channel
/// order), optionally de-duplicating.
///
/// Each named channel that holds an array contributes its items in order;
/// non-array / missing channels contribute nothing. With `dedupeBy` set, items
/// are de-duplicated: when the items are objects, by the string form of that
/// field; otherwise the param is ignored and whole-value dedupe is not applied.
fn build_document_joiner(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "documentJoiner";
    let into = require_string(kind, params, "into")?;
    let dedupe_by = optional_string(kind, params, "dedupeBy")?;

    let from_value = params
        .get("fromChannels")
        .ok_or_else(|| ComponentError::MissingParam {
            kind: kind.to_string(),
            param: "fromChannels".to_string(),
        })?;
    let from_arr = from_value
        .as_array()
        .ok_or_else(|| ComponentError::InvalidParam {
            kind: kind.to_string(),
            param: "fromChannels".to_string(),
            reason: "expected an array of channel names".to_string(),
        })?;
    let mut channels: Vec<String> = Vec::with_capacity(from_arr.len());
    for (i, item) in from_arr.iter().enumerate() {
        match item.as_str() {
            Some(s) => channels.push(s.to_string()),
            None => {
                return Err(ComponentError::InvalidParam {
                    kind: kind.to_string(),
                    param: "fromChannels".to_string(),
                    reason: format!("entry {i} is not a string"),
                })
            }
        }
    }
    let channels = Arc::new(channels);

    Ok(sync_handler(move |state: GraphState| {
        let mut merged: Vec<Value> = Vec::new();
        for name in channels.iter() {
            if let Some(Value::Array(items)) = state.channels.get(name) {
                merged.extend(items.iter().cloned());
            }
        }
        let merged = match &dedupe_by {
            Some(field) => dedupe_array(merged, Some(field)),
            None => merged,
        };
        NodeOutput::update(single(&into, Value::Array(merged)))
    }))
}

// --- deduplicator ------------------------------------------------------------

/// `deduplicator { from, into, key? }` — de-duplicate an array channel, keeping
/// the first occurrence and preserving order.
///
/// Without `key`, items are compared by their whole value (canonical JSON form).
/// With `key`, object items are compared by the string form of that field; items
/// that are not objects (or lack the field) fall back to whole-value comparison.
/// A missing / non-array channel yields an empty array.
fn build_deduplicator(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "deduplicator";
    let from = require_string(kind, params, "from")?;
    let into = require_string(kind, params, "into")?;
    let key = optional_string(kind, params, "key")?;

    Ok(sync_handler(move |state: GraphState| {
        let items: Vec<Value> = match state.channels.get(&from) {
            Some(Value::Array(arr)) => arr.clone(),
            _ => Vec::new(),
        };
        let deduped = dedupe_array(items, key.as_deref());
        NodeOutput::update(single(&into, Value::Array(deduped)))
    }))
}

/// De-duplicate `items`, keeping the first occurrence and preserving order.
///
/// The dedupe identity of each item is: if `key` is set and the item is an
/// object with that field, the string form of that field; otherwise the item's
/// canonical JSON string. Strings dedupe by their unquoted text so they match a
/// `key` field of the same text.
fn dedupe_array(items: Vec<Value>, key: Option<&str>) -> Vec<Value> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let identity = match (key, &item) {
            (Some(field), Value::Object(map)) => map
                .get(field)
                .map(value_to_text)
                .unwrap_or_else(|| item.to_string()),
            _ => value_to_text(&item),
        };
        if seen.insert(identity) {
            out.push(item);
        }
    }
    out
}

// --- truncator ---------------------------------------------------------------

/// `truncator { from, into, maxChars, ellipsis? }` — truncate a text channel to
/// at most `maxChars` characters.
///
/// When the text is longer than `maxChars`, it is cut to `maxChars` characters
/// (counting the `ellipsis` against the budget) and the `ellipsis` (default
/// `"…"`) is appended. If `ellipsis` is itself at least `maxChars` characters,
/// the text is cut to `maxChars` characters with no suffix. Text within the limit
/// is returned unchanged.
fn build_truncator(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "truncator";
    let from = require_string(kind, params, "from")?;
    let into = require_string(kind, params, "into")?;
    let max_chars = require_usize(kind, params, "maxChars")?;
    let ellipsis = optional_string(kind, params, "ellipsis")?.unwrap_or_else(|| "…".to_string());

    Ok(sync_handler(move |state: GraphState| {
        let text = state
            .channels
            .get(&from)
            .map(value_to_text)
            .unwrap_or_default();
        let chars: Vec<char> = text.chars().collect();
        let truncated: String = if chars.len() <= max_chars {
            text
        } else {
            let ellipsis_len = ellipsis.chars().count();
            if ellipsis_len >= max_chars {
                chars[..max_chars].iter().collect()
            } else {
                let keep = max_chars - ellipsis_len;
                let mut s: String = chars[..keep].iter().collect();
                s.push_str(&ellipsis);
                s
            }
        };
        NodeOutput::update(single(&into, Value::String(truncated)))
    }))
}

// --- regexExtractor ----------------------------------------------------------

/// `regexExtractor { from, into, pattern, group?, all? }` — extract matches of a
/// pattern from a text channel.
///
/// **Pattern dialect (deterministic, zero-dependency):** to avoid pulling in the
/// heavy `regex` crate, this component implements a small, documented matcher
/// rather than full regular expressions. A `pattern` is matched as a *literal
/// substring* with two optional anchors: a leading `^` requires the match at the
/// start of the text and a trailing `$` requires it at the end. There are no
/// character classes, quantifiers, or capture groups — the `group` param is
/// accepted for forward-compatibility but only `0` (the whole match) is
/// supported, and any other value yields no match. With `all: true`, every
/// non-overlapping occurrence is returned as an array; otherwise the first match
/// (or `null` when there is none) is written.
fn build_regex_extractor(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "regexExtractor";
    let from = require_string(kind, params, "from")?;
    let into = require_string(kind, params, "into")?;
    let pattern = require_string(kind, params, "pattern")?;
    let group = optional_usize(kind, params, "group")?.unwrap_or(0);
    let all = optional_bool(kind, params, "all", false)?;

    if pattern.is_empty() {
        return Err(ComponentError::InvalidParam {
            kind: kind.to_string(),
            param: "pattern".to_string(),
            reason: "must not be empty".to_string(),
        });
    }

    Ok(sync_handler(move |state: GraphState| {
        let text = state
            .channels
            .get(&from)
            .map(value_to_text)
            .unwrap_or_default();
        // Only the whole-match group (0) is supported by the literal matcher.
        let matches: Vec<String> = if group == 0 {
            literal_matches(&text, &pattern, all)
        } else {
            Vec::new()
        };
        let value = if all {
            Value::Array(matches.into_iter().map(Value::String).collect())
        } else {
            matches
                .into_iter()
                .next()
                .map(Value::String)
                .unwrap_or(Value::Null)
        };
        NodeOutput::update(single(&into, value))
    }))
}

/// Find literal `pattern` occurrences in `text`, honouring leading `^` (start
/// anchor) and trailing `$` (end anchor). Returns the matched literal(s). When
/// `all` is false at most one match is returned; matches are non-overlapping.
fn literal_matches(text: &str, pattern: &str, all: bool) -> Vec<String> {
    let anchored_start = pattern.starts_with('^');
    let anchored_end = pattern.ends_with('$');
    let start_trim = if anchored_start { 1 } else { 0 };
    let end_trim = if anchored_end { 1 } else { 0 };
    if start_trim + end_trim >= pattern.len() {
        // Pattern is only anchors with no literal body.
        return Vec::new();
    }
    let literal = &pattern[start_trim..pattern.len() - end_trim];

    if anchored_start && anchored_end {
        return if text == literal {
            vec![literal.to_string()]
        } else {
            Vec::new()
        };
    }
    if anchored_start {
        return if text.starts_with(literal) {
            vec![literal.to_string()]
        } else {
            Vec::new()
        };
    }
    if anchored_end {
        return if text.ends_with(literal) {
            vec![literal.to_string()]
        } else {
            Vec::new()
        };
    }

    // Unanchored: scan for non-overlapping occurrences.
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(pos) = rest.find(literal) {
        out.push(literal.to_string());
        if !all {
            break;
        }
        rest = &rest[pos + literal.len()..];
    }
    out
}

// --- answerBuilder -----------------------------------------------------------

/// `answerBuilder { from, contextFrom?, into, template? }` — assemble a final
/// answer string from an answer channel, optionally appending the retrieved
/// context as numbered citations.
///
/// The `from` channel supplies the core answer text. When `contextFrom` names a
/// channel holding a retrieval-result array (the `{ id, content }` shape the
/// `retriever`/`reranker` components emit), each item is rendered as a numbered
/// citation appended after the answer. The optional `template` may contain
/// `{{answer}}` and `{{citations}}` placeholders to control the layout; when
/// absent, the default layout is the answer followed by a blank line and a
/// `Sources:` block (or just the answer when there is no context).
fn build_answer_builder(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "answerBuilder";
    let from = require_string(kind, params, "from")?;
    let into = require_string(kind, params, "into")?;
    let context_from = optional_string(kind, params, "contextFrom")?;
    let template = optional_string(kind, params, "template")?;

    Ok(sync_handler(move |state: GraphState| {
        let answer = state
            .channels
            .get(&from)
            .map(value_to_text)
            .unwrap_or_default();

        let citations = context_from
            .as_ref()
            .and_then(|c| match state.channels.get(c) {
                Some(Value::Array(items)) => Some(items.clone()),
                _ => None,
            })
            .map(|items| render_citations(&items))
            .unwrap_or_default();

        let result = match &template {
            Some(tpl) => {
                let mut vars = BTreeMap::new();
                vars.insert("answer".to_string(), Value::String(answer));
                vars.insert("citations".to_string(), Value::String(citations));
                render_template(tpl, &vars)
            }
            None => {
                if citations.is_empty() {
                    answer
                } else {
                    format!("{answer}\n\nSources:\n{citations}")
                }
            }
        };
        NodeOutput::update(single(&into, Value::String(result)))
    }))
}

/// Render a retrieval-result array into a numbered citation block: one
/// `"[n] <id>: <content>"` line per item (id omitted when absent).
fn render_citations(items: &[Value]) -> String {
    let mut lines = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let n = i + 1;
        let id = item.get("id").and_then(Value::as_str);
        let content = item
            .get("content")
            .map(value_to_text)
            .unwrap_or_else(|| value_to_text(item));
        match id {
            Some(id) => lines.push(format!("[{n}] {id}: {content}")),
            None => lines.push(format!("[{n}] {content}")),
        }
    }
    lines.join("\n")
}

// --- fieldMapper -------------------------------------------------------------

/// `fieldMapper { from, into, mapping:{<outKey>:<inKeyPath>} }` — remap an
/// object channel's fields into a new object.
///
/// For each `outKey -> inKeyPath` entry, the value at the dotted `inKeyPath`
/// (e.g. `"user.name"`) in the `from` object is copied to `outKey` in the result.
/// A path that does not resolve writes `null`. A missing / non-object `from`
/// channel resolves every path to `null`.
fn build_field_mapper(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "fieldMapper";
    let from = require_string(kind, params, "from")?;
    let into = require_string(kind, params, "into")?;

    let mapping_value = params
        .get("mapping")
        .ok_or_else(|| ComponentError::MissingParam {
            kind: kind.to_string(),
            param: "mapping".to_string(),
        })?;
    let mapping_obj = mapping_value
        .as_object()
        .ok_or_else(|| ComponentError::InvalidParam {
            kind: kind.to_string(),
            param: "mapping".to_string(),
            reason: "expected an object of { outKey: inKeyPath }".to_string(),
        })?;
    let mut mapping: Vec<(String, String)> = Vec::with_capacity(mapping_obj.len());
    for (out_key, path) in mapping_obj {
        let path = path
            .as_str()
            .ok_or_else(|| ComponentError::InvalidParam {
                kind: kind.to_string(),
                param: "mapping".to_string(),
                reason: format!("value for `{out_key}` must be a string path"),
            })?
            .to_string();
        mapping.push((out_key.clone(), path));
    }
    let mapping = Arc::new(mapping);

    Ok(sync_handler(move |state: GraphState| {
        let source = state.channels.get(&from).cloned().unwrap_or(Value::Null);
        let mut obj = serde_json::Map::new();
        for (out_key, path) in mapping.iter() {
            let value = resolve_path(&source, path).cloned().unwrap_or(Value::Null);
            obj.insert(out_key.clone(), value);
        }
        NodeOutput::update(single(&into, Value::Object(obj)))
    }))
}

/// Resolve a dotted path (`"a.b.c"`) into a JSON value, descending through
/// objects. Returns `None` if any segment is missing or a non-object is hit.
fn resolve_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

// --- fieldExtractor ----------------------------------------------------------

/// `fieldExtractor { from, into, path?, finalOnly? }` — read the `from` channel
/// value, optionally descend a dotted `path` into it, and write the resulting
/// scalar to `into`.
///
/// Generic object/dotted-path extraction: with `path` set (e.g. `"reasoning"` or
/// `"user.name"`), the value at that path in the `from` value is taken; an
/// unresolved path yields `null`. Without `path`, the whole `from` value is used.
/// With `finalOnly: true`, when the resulting value is a string containing a
/// `"final:"` marker, only the text AFTER the **last** `"final:"` (trimmed) is
/// returned — the convenience that reduces an `AgentResult.reasoning` trace to
/// just its final answer text. A non-string value (or a string without the
/// marker) is returned unchanged by `finalOnly`. A missing `from` channel reads
/// as `null`.
fn build_field_extractor(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "fieldExtractor";
    let from = require_string(kind, params, "from")?;
    let into = require_string(kind, params, "into")?;
    let path = optional_string(kind, params, "path")?;
    let final_only = optional_bool(kind, params, "finalOnly", false)?;

    Ok(sync_handler(move |state: GraphState| {
        let source = state.channels.get(&from).cloned().unwrap_or(Value::Null);
        let mut value = match &path {
            Some(p) => resolve_path(&source, p).cloned().unwrap_or(Value::Null),
            None => source,
        };
        if final_only {
            if let Value::String(s) = &value {
                value = Value::String(extract_final_answer(s));
            }
        }
        NodeOutput::update(single(&into, value))
    }))
}

/// Return only the text after the **last** `"final:"` marker (trimmed); if the
/// marker is absent, return the text unchanged (trimmed only of nothing — the
/// original string). Used by `fieldExtractor`'s `finalOnly` flag to reduce an
/// agent reasoning trace (whose final line is `final:<answer>`) to the answer.
fn extract_final_answer(text: &str) -> String {
    match text.rfind("final:") {
        Some(pos) => text[pos + "final:".len()..].trim().to_string(),
        None => text.to_string(),
    }
}

// --- shared lexical helpers (bm25 / keyword / evaluator) ---------------------

/// Tokenize text into lowercase alphanumeric word tokens, dropping punctuation
/// and empty runs. Deterministic and dependency-free; used by the lexical
/// retrievers and the token-overlap evaluator.
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Read a required array-of-`{ id, content }` doc param into [`Document`]s.
/// Shared by the lexical retrievers; mirrors the `retriever` doc parsing.
fn require_docs(kind: &str, params: &Value) -> Result<Vec<Document>, ComponentError> {
    let docs_value = params
        .get("docs")
        .ok_or_else(|| ComponentError::MissingParam {
            kind: kind.to_string(),
            param: "docs".to_string(),
        })?;
    let docs_arr = docs_value
        .as_array()
        .ok_or_else(|| ComponentError::InvalidParam {
            kind: kind.to_string(),
            param: "docs".to_string(),
            reason: "expected an array of { id, content }".to_string(),
        })?;
    let mut docs: Vec<Document> = Vec::with_capacity(docs_arr.len());
    for (i, doc) in docs_arr.iter().enumerate() {
        let id =
            doc.get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| ComponentError::InvalidParam {
                    kind: kind.to_string(),
                    param: "docs".to_string(),
                    reason: format!("doc {i} is missing a string `id`"),
                })?;
        let content = doc.get("content").and_then(Value::as_str).ok_or_else(|| {
            ComponentError::InvalidParam {
                kind: kind.to_string(),
                param: "docs".to_string(),
                reason: format!("doc {i} is missing a string `content`"),
            }
        })?;
        docs.push(Document::new(id, content));
    }
    Ok(docs)
}

/// Read the query text for a retriever: prefer the `query` channel's text,
/// falling back to the literal `query` param value when the channel is empty.
fn query_text(channels: &BTreeMap<String, Value>, query_param: &str) -> String {
    channels
        .get(query_param)
        .map(value_to_text)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| query_param.to_string())
}

/// Build a `serde_json` number from an f64, falling back to `null` on non-finite.
fn json_number(n: f64) -> Value {
    serde_json::Number::from_f64(n)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

// --- bm25Retriever -----------------------------------------------------------

/// `bm25Retriever { query, into, k?, docs:[{id,content}], k1?, b? }` — lexical
/// BM25 ranking of the corpus against the query, writing the top-`k`
/// `{ id, content, score }` results to `into`.
///
/// Pure and deterministic: tokenizes on non-alphanumeric boundaries, computes
/// IDF/term-frequency BM25 over the static corpus, and breaks score ties by
/// input order (stable sort). `k1` (default `1.2`) and `b` (default `0.75`) are
/// the usual BM25 saturation / length-normalization knobs. `query` is a channel
/// name; the query text is read at run time (falling back to the literal param).
fn build_bm25_retriever(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "bm25Retriever";
    let query = require_string(kind, params, "query")?;
    let into = require_string(kind, params, "into")?;
    let k = optional_usize(kind, params, "k")?.unwrap_or(4);
    let k1 = optional_f64(kind, params, "k1")?.unwrap_or(1.2);
    let b = optional_f64(kind, params, "b")?.unwrap_or(0.75);
    let docs = Arc::new(require_docs(kind, params)?);

    // Precompute per-doc token lists and the corpus statistics BM25 needs.
    let doc_tokens: Vec<Vec<String>> = docs.iter().map(|d| tokenize(&d.content)).collect();
    let doc_count = doc_tokens.len().max(1) as f64;
    let avg_len: f64 = if doc_tokens.is_empty() {
        0.0
    } else {
        doc_tokens.iter().map(|t| t.len()).sum::<usize>() as f64 / doc_tokens.len() as f64
    };
    // Document frequency per term.
    let mut df: BTreeMap<String, usize> = BTreeMap::new();
    for tokens in &doc_tokens {
        let mut unique: std::collections::HashSet<&String> = std::collections::HashSet::new();
        for tok in tokens {
            if unique.insert(tok) {
                *df.entry(tok.clone()).or_insert(0) += 1;
            }
        }
    }
    let doc_tokens = Arc::new(doc_tokens);
    let df = Arc::new(df);

    Ok(sync_handler(move |state: GraphState| {
        let q_tokens = tokenize(&query_text(&state.channels, &query));

        let mut scored: Vec<(f64, usize, &Document)> = docs
            .iter()
            .enumerate()
            .map(|(i, doc)| {
                let tokens = &doc_tokens[i];
                let len = tokens.len() as f64;
                let mut score = 0.0_f64;
                for q in &q_tokens {
                    let f = tokens.iter().filter(|t| *t == q).count() as f64;
                    if f == 0.0 {
                        continue;
                    }
                    let n_q = *df.get(q).unwrap_or(&0) as f64;
                    // Standard BM25 IDF with the +1 inside the log to stay >= 0.
                    let idf = (((doc_count - n_q + 0.5) / (n_q + 0.5)) + 1.0).ln();
                    let denom = f + k1 * (1.0 - b + b * (len / avg_len.max(1.0)));
                    score += idf * (f * (k1 + 1.0)) / denom;
                }
                (score, i, doc)
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
        });
        // ADR 0044 D2 (ailu#578): everything past `k` used to be dropped with zero trace —
        // split instead of truncating, so the loser half survives on `{into}Discarded`.
        let discarded_at = now_millis_string();
        let (kept, discarded) = scored.split_at(scored.len().min(k));

        // ADR 0044 D1 (ailu#578): an additive provenance step, never an overwritten `score` —
        // the top-level `score` still mirrors this stage's score for any caller that ignores lineage.
        let provenance_step = |score: f64| {
            json!({
                "stage": "bm25",
                "algorithm": "bm25",
                "algorithmVersion": format!("k1={k1},b={b}"),
                "score": json_number(score)
            })
        };
        let results: Vec<Value> = kept
            .iter()
            .map(|(score, _, doc)| {
                json!({
                    "id": doc.id,
                    "content": doc.content,
                    "score": json_number(*score),
                    "provenance": [provenance_step(*score)]
                })
            })
            .collect();
        let discarded_results: Vec<Value> = discarded
            .iter()
            .map(|(score, _, doc)| {
                json!({
                    "id": doc.id,
                    "content": doc.content,
                    "score": json_number(*score),
                    "provenance": [provenance_step(*score)],
                    "discardedAt": discarded_at,
                    "reason": "bm25_rank_below_k"
                })
            })
            .collect();
        NodeOutput::update(BTreeMap::from([
            (into.clone(), Value::Array(results)),
            (format!("{into}Discarded"), Value::Array(discarded_results)),
        ]))
    }))
}

/// Read an optional finite-float param (absent or `null` -> `None`).
fn optional_f64(kind: &str, params: &Value, key: &str) -> Result<Option<f64>, ComponentError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .map(Some)
            .ok_or_else(|| ComponentError::InvalidParam {
                kind: kind.to_string(),
                param: key.to_string(),
                reason: "expected a number".to_string(),
            }),
    }
}

// --- keywordRetriever --------------------------------------------------------

/// `keywordRetriever { query, into, k?, docs:[{id,content}] }` — lexical
/// keyword-overlap ranking: scores each doc by the fraction of distinct query
/// terms it contains (a simple, explainable alternative to BM25), writing the
/// top-`k` `{ id, content, score }` results to `into`.
///
/// Pure and deterministic. The score is `|matched query terms| / |query terms|`
/// in `[0, 1]`; ties break by input order. Docs with no overlap score `0`.
fn build_keyword_retriever(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "keywordRetriever";
    let query = require_string(kind, params, "query")?;
    let into = require_string(kind, params, "into")?;
    let k = optional_usize(kind, params, "k")?.unwrap_or(4);
    let docs = Arc::new(require_docs(kind, params)?);
    let doc_tokens: Arc<Vec<std::collections::HashSet<String>>> = Arc::new(
        docs.iter()
            .map(|d| tokenize(&d.content).into_iter().collect())
            .collect(),
    );

    Ok(sync_handler(move |state: GraphState| {
        let q_terms: Vec<String> = {
            let mut seen = std::collections::HashSet::new();
            tokenize(&query_text(&state.channels, &query))
                .into_iter()
                .filter(|t| seen.insert(t.clone()))
                .collect()
        };
        let denom = q_terms.len().max(1) as f64;

        let mut scored: Vec<(f64, usize, &Document)> = docs
            .iter()
            .enumerate()
            .map(|(i, doc)| {
                let tokens = &doc_tokens[i];
                let matched = q_terms.iter().filter(|t| tokens.contains(*t)).count() as f64;
                (matched / denom, i, doc)
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
        });
        scored.truncate(k);

        let results: Vec<Value> = scored
            .into_iter()
            .map(|(score, _, doc)| {
                json!({ "id": doc.id, "content": doc.content, "score": json_number(score) })
            })
            .collect();
        NodeOutput::update(single(&into, Value::Array(results)))
    }))
}

// --- sentenceWindowSplitter --------------------------------------------------

/// `sentenceWindowSplitter { from, into, windowSize?, stride? }` — split text
/// into overlapping windows of whole sentences, writing a `string[]` to `into`.
///
/// Distinct from `documentSplitter`'s sentence mode: this is a true sliding
/// window with an explicit `stride` (default `1`) so consecutive windows share
/// `windowSize - stride` sentences — the Haystack "sentence window" retrieval
/// pattern. `windowSize` defaults to `3`; `stride` must be `>= 1` and
/// `<= windowSize`. Empty input yields an empty array.
fn build_sentence_window_splitter(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "sentenceWindowSplitter";
    let from = require_string(kind, params, "from")?;
    let into = require_string(kind, params, "into")?;
    let window_size = optional_usize(kind, params, "windowSize")?.unwrap_or(3);
    let stride = optional_usize(kind, params, "stride")?.unwrap_or(1);

    if window_size == 0 {
        return Err(ComponentError::InvalidParam {
            kind: kind.to_string(),
            param: "windowSize".to_string(),
            reason: "must be greater than zero".to_string(),
        });
    }
    if stride == 0 || stride > window_size {
        return Err(ComponentError::InvalidParam {
            kind: kind.to_string(),
            param: "stride".to_string(),
            reason: "must be between 1 and `windowSize`".to_string(),
        });
    }

    Ok(sync_handler(move |state: GraphState| {
        let text = state
            .channels
            .get(&from)
            .map(value_to_text)
            .unwrap_or_default();
        let sentences = segment_sentences(&text);
        let mut windows: Vec<Value> = Vec::new();
        if !sentences.is_empty() {
            let mut start = 0;
            loop {
                let end = (start + window_size).min(sentences.len());
                windows.push(Value::String(sentences[start..end].join(" ")));
                if end == sentences.len() {
                    break;
                }
                start += stride;
            }
        }
        NodeOutput::update(single(&into, Value::Array(windows)))
    }))
}

// --- languageDetector --------------------------------------------------------

/// `languageDetector { from, into, confidenceInto? }` — heuristic language
/// detection over a small set of common languages, writing an ISO-639-1-ish
/// code (`"en" | "fr" | "es" | "de" | "it" | "und"`) to `into`.
///
/// Pure and deterministic: scores by stop-word hits per language; the highest
/// score wins, with ties broken by a fixed language order and `"und"`
/// (undetermined) returned when there are no hits. An optional `confidenceInto`
/// channel receives the winning language's share of total hits in `[0, 1]`.
fn build_language_detector(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "languageDetector";
    let from = require_string(kind, params, "from")?;
    let into = require_string(kind, params, "into")?;
    let confidence_into = optional_string(kind, params, "confidenceInto")?;

    // Fixed language order doubles as the deterministic tie-break order.
    const LANGS: &[(&str, &[&str])] = &[
        ("en", &["the", "and", "is", "of", "to", "in", "that", "it"]),
        ("fr", &["le", "la", "les", "et", "est", "un", "une", "des"]),
        ("es", &["el", "la", "los", "y", "es", "un", "una", "de"]),
        (
            "de",
            &["der", "die", "das", "und", "ist", "ein", "eine", "nicht"],
        ),
        ("it", &["il", "la", "che", "di", "e", "un", "una", "per"]),
    ];

    Ok(sync_handler(move |state: GraphState| {
        let text = state
            .channels
            .get(&from)
            .map(value_to_text)
            .unwrap_or_default();
        let tokens = tokenize(&text);

        let scores: Vec<usize> = LANGS
            .iter()
            .map(|(_, stops)| {
                tokens
                    .iter()
                    .filter(|t| stops.contains(&t.as_str()))
                    .count()
            })
            .collect();
        let total: usize = scores.iter().sum();

        let (lang, best) = scores
            .iter()
            .enumerate()
            .max_by_key(|(i, score)| (**score, std::cmp::Reverse(*i)))
            .map(|(i, score)| (LANGS[i].0, *score))
            .unwrap_or(("und", 0));
        let detected = if best == 0 { "und" } else { lang };

        let mut update = BTreeMap::new();
        update.insert(into.clone(), Value::String(detected.to_string()));
        if let Some(channel) = &confidence_into {
            let confidence = if total == 0 {
                0.0
            } else {
                best as f64 / total as f64
            };
            update.insert(channel.clone(), json_number(confidence));
        }
        NodeOutput::update(update)
    }))
}

// --- metadataFilter ----------------------------------------------------------

/// `metadataFilter { from, into, field, op, value? }` — keep the items of an
/// array channel whose dotted-path `field` satisfies a predicate, writing the
/// filtered array to `into`.
///
/// Pure and deterministic. Supported `op`s: `"equals"` / `"notEquals"`
/// (compared by text form), `"contains"` (substring of the text form), `"exists"`
/// / `"absent"` (whether the path resolves), `"gt"` / `"gte"` / `"lt"` / `"lte"`
/// (numeric). `value` is required except for `exists`/`absent`. Items that are
/// not objects, or whose `field` does not resolve, fail every op except `absent`.
fn build_metadata_filter(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "metadataFilter";
    let from = require_string(kind, params, "from")?;
    let into = require_string(kind, params, "into")?;
    let field = require_string(kind, params, "field")?;
    let op = require_string(kind, params, "op")?;

    let needs_value = !matches!(op.as_str(), "exists" | "absent");
    let numeric = matches!(op.as_str(), "gt" | "gte" | "lt" | "lte");
    match op.as_str() {
        "equals" | "notEquals" | "contains" | "exists" | "absent" | "gt" | "gte" | "lt" | "lte" => {
        }
        other => {
            return Err(ComponentError::InvalidParam {
                kind: kind.to_string(),
                param: "op".to_string(),
                reason: format!("unknown op `{other}`"),
            })
        }
    }
    let value = params.get("value").cloned().unwrap_or(Value::Null);
    if needs_value && matches!(value, Value::Null) {
        return Err(ComponentError::MissingParam {
            kind: kind.to_string(),
            param: "value".to_string(),
        });
    }
    let value_text = value_to_text(&value);
    let value_num = if numeric {
        Some(value.as_f64().ok_or_else(|| ComponentError::InvalidParam {
            kind: kind.to_string(),
            param: "value".to_string(),
            reason: "numeric op requires a numeric value".to_string(),
        })?)
    } else {
        None
    };

    Ok(sync_handler(move |state: GraphState| {
        let items: Vec<Value> = match state.channels.get(&from) {
            Some(Value::Array(arr)) => arr.clone(),
            _ => Vec::new(),
        };
        let kept: Vec<Value> = items
            .into_iter()
            .filter(|item| {
                let resolved = resolve_path(item, &field);
                match op.as_str() {
                    "exists" => resolved.is_some(),
                    "absent" => resolved.is_none(),
                    "equals" => resolved.map(value_to_text).as_deref() == Some(&value_text),
                    "notEquals" => resolved.map(value_to_text).as_deref() != Some(&value_text),
                    "contains" => resolved
                        .map(value_to_text)
                        .is_some_and(|t| t.contains(&value_text)),
                    _ => {
                        // Numeric comparisons.
                        let Some(n) = resolved.and_then(Value::as_f64) else {
                            return false;
                        };
                        let target = value_num.unwrap_or(0.0);
                        match op.as_str() {
                            "gt" => n > target,
                            "gte" => n >= target,
                            "lt" => n < target,
                            "lte" => n <= target,
                            _ => false,
                        }
                    }
                }
            })
            .collect();
        NodeOutput::update(single(&into, Value::Array(kept)))
    }))
}

// --- listJoiner --------------------------------------------------------------

/// `listJoiner { fromChannels:[..], into, mode? }` — combine several array
/// channels into one list using a set `mode`: `"concat"` (default, in channel
/// order), `"union"` (concat then de-duplicate by value), or `"interleave"`
/// (round-robin one item from each list at a time).
///
/// Pure and deterministic. Non-array / missing channels contribute nothing.
/// Distinct from `documentJoiner`, which is object-array oriented with field
/// dedupe; `listJoiner` works on any JSON values and offers interleaving.
fn build_list_joiner(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "listJoiner";
    let into = require_string(kind, params, "into")?;
    let mode = optional_string(kind, params, "mode")?.unwrap_or_else(|| "concat".to_string());
    match mode.as_str() {
        "concat" | "union" | "interleave" => {}
        other => {
            return Err(ComponentError::InvalidParam {
                kind: kind.to_string(),
                param: "mode".to_string(),
                reason: format!(
                    "expected \"concat\", \"union\" or \"interleave\", got \"{other}\""
                ),
            })
        }
    }
    let channels = Arc::new(require_string_array(kind, params, "fromChannels")?);

    Ok(sync_handler(move |state: GraphState| {
        let lists: Vec<Vec<Value>> = channels
            .iter()
            .map(|name| match state.channels.get(name) {
                Some(Value::Array(items)) => items.clone(),
                _ => Vec::new(),
            })
            .collect();

        let merged: Vec<Value> = match mode.as_str() {
            "concat" => lists.into_iter().flatten().collect(),
            "union" => dedupe_array(lists.into_iter().flatten().collect(), None),
            _ => {
                // interleave: round-robin until every list is exhausted.
                let max = lists.iter().map(Vec::len).max().unwrap_or(0);
                let mut out = Vec::new();
                for i in 0..max {
                    for list in &lists {
                        if let Some(v) = list.get(i) {
                            out.push(v.clone());
                        }
                    }
                }
                out
            }
        };
        NodeOutput::update(single(&into, Value::Array(merged)))
    }))
}

/// Read a required array-of-strings param.
fn require_string_array(
    kind: &str,
    params: &Value,
    key: &str,
) -> Result<Vec<String>, ComponentError> {
    let arr = params
        .get(key)
        .ok_or_else(|| ComponentError::MissingParam {
            kind: kind.to_string(),
            param: key.to_string(),
        })?
        .as_array()
        .ok_or_else(|| ComponentError::InvalidParam {
            kind: kind.to_string(),
            param: key.to_string(),
            reason: "expected an array of strings".to_string(),
        })?;
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        match item.as_str() {
            Some(s) => out.push(s.to_string()),
            None => {
                return Err(ComponentError::InvalidParam {
                    kind: kind.to_string(),
                    param: key.to_string(),
                    reason: format!("entry {i} is not a string"),
                })
            }
        }
    }
    Ok(out)
}

// --- mergeRanker -------------------------------------------------------------

/// `mergeRanker { fromChannels:[..], into, idKey?, k?, rrfK? }` — fuse several
/// retrieval-result streams into one ranking with Reciprocal Rank Fusion (RRF),
/// writing the top-`k` merged `{ id, content, score }` array to `into`.
///
/// Pure and deterministic. Each item's RRF contribution from a list is
/// `1 / (rrfK + rank)` (rank 0-based; `rrfK` default `60`, the canonical RRF
/// constant). Items are identified across lists by `idKey` (default `"id"`);
/// the fused `score` is the summed RRF weight. Ties break by first-seen order.
fn build_merge_ranker(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "mergeRanker";
    let into = require_string(kind, params, "into")?;
    let id_key = optional_string(kind, params, "idKey")?.unwrap_or_else(|| "id".to_string());
    let k = optional_usize(kind, params, "k")?;
    let rrf_k = optional_f64(kind, params, "rrfK")?.unwrap_or(60.0);
    let channels = Arc::new(require_string_array(kind, params, "fromChannels")?);

    Ok(sync_handler(move |state: GraphState| {
        // Accumulate per-id: fused score, a representative item, and first-seen order.
        let mut scores: BTreeMap<String, f64> = BTreeMap::new();
        let mut representative: BTreeMap<String, Value> = BTreeMap::new();
        let mut first_seen: BTreeMap<String, usize> = BTreeMap::new();
        // ADR 0044 (ailu#578): a candidate seen in MULTIPLE `fromChannels` (e.g. both the
        // vector and lexical leg) must keep every leg's provenance — `representative` above only
        // ever kept whichever channel was processed FIRST, silently dropping every other leg's
        // lineage. Accumulate provenance across ALL channels an id appears in, independent of
        // which item ends up "representative" for id/content.
        let mut provenance_by_id: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let mut counter = 0usize;

        for name in channels.iter() {
            let Some(Value::Array(items)) = state.channels.get(name) else {
                continue;
            };
            for (rank, item) in items.iter().enumerate() {
                let id = item
                    .get(&id_key)
                    .map(value_to_text)
                    .unwrap_or_else(|| value_to_text(item));
                let weight = 1.0 / (rrf_k + rank as f64);
                *scores.entry(id.clone()).or_insert(0.0) += weight;
                representative
                    .entry(id.clone())
                    .or_insert_with(|| item.clone());
                first_seen.entry(id.clone()).or_insert_with(|| {
                    let c = counter;
                    counter += 1;
                    c
                });
                if let Some(Value::Array(steps)) = item.get("provenance") {
                    provenance_by_id
                        .entry(id.clone())
                        .or_default()
                        .extend(steps.iter().cloned());
                }
            }
        }

        let mut merged: Vec<(f64, usize, String)> = scores
            .iter()
            .map(|(id, score)| (*score, *first_seen.get(id).unwrap_or(&0), id.clone()))
            .collect();
        merged.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
        });
        // ADR 0044 D2 (ailu#578): the RRF losers past `k` used to be dropped with zero
        // trace — split instead of truncating, so they survive on `{into}Discarded`.
        let discarded_at = now_millis_string();
        let split_at = k.unwrap_or(merged.len()).min(merged.len());
        let (kept, discarded) = merged.split_at(split_at);

        let build_item = |score: f64, id: &str, extra_step: bool| {
            let mut item = representative.get(id).cloned().unwrap_or(Value::Null);
            if let Value::Object(map) = &mut item {
                map.insert("score".to_string(), json_number(score));
                let mut provenance = provenance_by_id.get(id).cloned().unwrap_or_default();
                provenance.push(json!({
                    "stage": "rrf",
                    "algorithm": "rrf",
                    "algorithmVersion": format!("k={rrf_k}"),
                    "score": json_number(score)
                }));
                map.insert("provenance".to_string(), Value::Array(provenance));
                if extra_step {
                    map.insert(
                        "discardedAt".to_string(),
                        Value::String(discarded_at.clone()),
                    );
                    map.insert(
                        "reason".to_string(),
                        Value::String("rrf_rank_below_k".to_string()),
                    );
                }
            }
            item
        };
        let results: Vec<Value> = kept
            .iter()
            .map(|(score, _, id)| build_item(*score, id, false))
            .collect();
        let discarded_results: Vec<Value> = discarded
            .iter()
            .map(|(score, _, id)| build_item(*score, id, true))
            .collect();
        NodeOutput::update(BTreeMap::from([
            (into.clone(), Value::Array(results)),
            (format!("{into}Discarded"), Value::Array(discarded_results)),
        ]))
    }))
}

// --- evaluator ---------------------------------------------------------------

/// `evaluator { expectedFrom, actualFrom, into, metric?, passInto?, threshold? }`
/// — score the `actualFrom` text against the `expectedFrom` text and write the
/// numeric score (in `[0, 1]`) to `into`.
///
/// Pure and deterministic. `metric`: `"tokenF1"` (default) is the token-overlap
/// F1 of the two token multisets; `"overlap"` is the Jaccard overlap of the
/// distinct token sets; `"exact"` is `1.0` iff the trimmed texts are equal else
/// `0.0`. With `passInto` set, a boolean `score >= threshold` (default `0.5`) is
/// also written there.
fn build_evaluator(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "evaluator";
    let expected_from = require_string(kind, params, "expectedFrom")?;
    let actual_from = require_string(kind, params, "actualFrom")?;
    let into = require_string(kind, params, "into")?;
    let metric = optional_string(kind, params, "metric")?.unwrap_or_else(|| "tokenF1".to_string());
    let pass_into = optional_string(kind, params, "passInto")?;
    let threshold = optional_f64(kind, params, "threshold")?.unwrap_or(0.5);
    match metric.as_str() {
        "tokenF1" | "overlap" | "exact" => {}
        other => {
            return Err(ComponentError::InvalidParam {
                kind: kind.to_string(),
                param: "metric".to_string(),
                reason: format!("expected \"tokenF1\", \"overlap\" or \"exact\", got \"{other}\""),
            })
        }
    }

    Ok(sync_handler(move |state: GraphState| {
        let expected = state
            .channels
            .get(&expected_from)
            .map(value_to_text)
            .unwrap_or_default();
        let actual = state
            .channels
            .get(&actual_from)
            .map(value_to_text)
            .unwrap_or_default();

        let score = match metric.as_str() {
            "exact" => {
                if expected.trim() == actual.trim() {
                    1.0
                } else {
                    0.0
                }
            }
            "overlap" => {
                let e: std::collections::HashSet<String> =
                    tokenize(&expected).into_iter().collect();
                let a: std::collections::HashSet<String> = tokenize(&actual).into_iter().collect();
                if e.is_empty() && a.is_empty() {
                    1.0
                } else {
                    let inter = e.intersection(&a).count() as f64;
                    let union = e.union(&a).count() as f64;
                    if union == 0.0 {
                        0.0
                    } else {
                        inter / union
                    }
                }
            }
            _ => token_f1(&tokenize(&expected), &tokenize(&actual)),
        };

        let mut update = BTreeMap::new();
        update.insert(into.clone(), json_number(score));
        if let Some(channel) = &pass_into {
            update.insert(channel.clone(), Value::Bool(score >= threshold));
        }
        NodeOutput::update(update)
    }))
}

/// Token-overlap F1 of two token multisets: the harmonic mean of precision
/// (matched / |actual|) and recall (matched / |expected|), where `matched` is
/// the size of the multiset intersection. Two empty inputs score `1.0`; one
/// empty side scores `0.0`.
fn token_f1(expected: &[String], actual: &[String]) -> f64 {
    if expected.is_empty() && actual.is_empty() {
        return 1.0;
    }
    if expected.is_empty() || actual.is_empty() {
        return 0.0;
    }
    let mut expected_counts: BTreeMap<&String, usize> = BTreeMap::new();
    for t in expected {
        *expected_counts.entry(t).or_insert(0) += 1;
    }
    let mut matched = 0usize;
    for t in actual {
        if let Some(count) = expected_counts.get_mut(t) {
            if *count > 0 {
                *count -= 1;
                matched += 1;
            }
        }
    }
    if matched == 0 {
        return 0.0;
    }
    let precision = matched as f64 / actual.len() as f64;
    let recall = matched as f64 / expected.len() as f64;
    2.0 * precision * recall / (precision + recall)
}

// --- chatMessageBuilder ------------------------------------------------------

/// `chatMessageBuilder { into, messages:[{role, contentFrom?|content?}], systemFrom? }`
/// — assemble a role-tagged chat-message array (`[{ role, content }]`) into
/// `into`, the shape an LLM generator consumes.
///
/// Pure and deterministic. Each spec carries a `role` (`"system" | "user" |
/// "assistant"`) and either a literal `content` or a `contentFrom` channel name
/// (rendered through the same `{{var}}` template engine as `promptBuilder`, so a
/// message body can interpolate channels). An optional top-level `systemFrom`
/// channel is prepended as a leading system message when non-empty.
fn build_chat_message_builder(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "chatMessageBuilder";
    let into = require_string(kind, params, "into")?;
    let system_from = optional_string(kind, params, "systemFrom")?;

    let messages_value = params
        .get("messages")
        .ok_or_else(|| ComponentError::MissingParam {
            kind: kind.to_string(),
            param: "messages".to_string(),
        })?;
    let messages_arr = messages_value
        .as_array()
        .ok_or_else(|| ComponentError::InvalidParam {
            kind: kind.to_string(),
            param: "messages".to_string(),
            reason: "expected an array of { role, content|contentFrom }".to_string(),
        })?;

    /// One parsed message spec: role plus either a literal body or a template.
    struct MessageSpec {
        role: String,
        content: Option<String>,
        content_from: Option<String>,
    }

    let mut specs: Vec<MessageSpec> = Vec::with_capacity(messages_arr.len());
    for (i, msg) in messages_arr.iter().enumerate() {
        let role = msg.get("role").and_then(Value::as_str).ok_or_else(|| {
            ComponentError::InvalidParam {
                kind: kind.to_string(),
                param: "messages".to_string(),
                reason: format!("message {i} is missing a string `role`"),
            }
        })?;
        match role {
            "system" | "user" | "assistant" => {}
            other => {
                return Err(ComponentError::InvalidParam {
                    kind: kind.to_string(),
                    param: "messages".to_string(),
                    reason: format!("message {i} has unknown role `{other}`"),
                })
            }
        }
        let content = msg
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_string);
        let content_from = msg
            .get("contentFrom")
            .and_then(Value::as_str)
            .map(str::to_string);
        if content.is_none() && content_from.is_none() {
            return Err(ComponentError::InvalidParam {
                kind: kind.to_string(),
                param: "messages".to_string(),
                reason: format!("message {i} needs `content` or `contentFrom`"),
            });
        }
        specs.push(MessageSpec {
            role: role.to_string(),
            content,
            content_from,
        });
    }
    let specs = Arc::new(specs);

    Ok(sync_handler(move |state: GraphState| {
        let mut messages: Vec<Value> = Vec::new();
        if let Some(channel) = &system_from {
            let system = state
                .channels
                .get(channel)
                .map(value_to_text)
                .unwrap_or_default();
            if !system.is_empty() {
                messages.push(json!({ "role": "system", "content": system }));
            }
        }
        for spec in specs.iter() {
            let content = match (&spec.content, &spec.content_from) {
                // A literal body is rendered as a template so it can interpolate channels.
                (Some(tpl), _) => render_template(tpl, &state.channels),
                (None, Some(channel)) => state
                    .channels
                    .get(channel)
                    .map(value_to_text)
                    .unwrap_or_default(),
                (None, None) => String::new(),
            };
            messages.push(json!({ "role": spec.role, "content": content }));
        }
        NodeOutput::update(single(&into, Value::Array(messages)))
    }))
}

// --- conditionalRouter -------------------------------------------------------

/// `conditionalRouter { into, defaultRoute, branches:[{when:{field,op,value?}, route}] }`
/// — evaluate ordered predicate branches over the channels and write the first
/// matching branch's `route` to `into` (else `defaultRoute`).
///
/// Pure and deterministic, and a richer sibling of `router`: each branch's
/// `when` predicate reads a channel by dotted `field` and applies one of the
/// `metadataFilter` ops (`equals` / `notEquals` / `contains` / `exists` /
/// `absent` / `gt` / `gte` / `lt` / `lte`) against the channel map as a whole.
/// Pairs with a conditional edge keyed on `into`.
fn build_conditional_router(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "conditionalRouter";
    let into = require_string(kind, params, "into")?;
    let default_route = require_string(kind, params, "defaultRoute")?;

    let branches_value = params
        .get("branches")
        .ok_or_else(|| ComponentError::MissingParam {
            kind: kind.to_string(),
            param: "branches".to_string(),
        })?;
    let branches_arr = branches_value
        .as_array()
        .ok_or_else(|| ComponentError::InvalidParam {
            kind: kind.to_string(),
            param: "branches".to_string(),
            reason: "expected an array of { when, route }".to_string(),
        })?;

    /// One parsed branch: a predicate plus the route to emit when it holds.
    struct Branch {
        field: String,
        op: String,
        value: Value,
        route: String,
    }

    let mut branches: Vec<Branch> = Vec::with_capacity(branches_arr.len());
    for (i, branch) in branches_arr.iter().enumerate() {
        let route = branch
            .get("route")
            .and_then(Value::as_str)
            .ok_or_else(|| ComponentError::InvalidParam {
                kind: kind.to_string(),
                param: "branches".to_string(),
                reason: format!("branch {i} is missing a string `route`"),
            })?
            .to_string();
        let when = branch
            .get("when")
            .ok_or_else(|| ComponentError::InvalidParam {
                kind: kind.to_string(),
                param: "branches".to_string(),
                reason: format!("branch {i} is missing a `when` predicate"),
            })?;
        let field = when
            .get("field")
            .and_then(Value::as_str)
            .ok_or_else(|| ComponentError::InvalidParam {
                kind: kind.to_string(),
                param: "branches".to_string(),
                reason: format!("branch {i} `when` is missing a string `field`"),
            })?
            .to_string();
        let op = when
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| ComponentError::InvalidParam {
                kind: kind.to_string(),
                param: "branches".to_string(),
                reason: format!("branch {i} `when` is missing a string `op`"),
            })?
            .to_string();
        match op.as_str() {
            "equals" | "notEquals" | "contains" | "exists" | "absent" | "gt" | "gte" | "lt"
            | "lte" => {}
            other => {
                return Err(ComponentError::InvalidParam {
                    kind: kind.to_string(),
                    param: "branches".to_string(),
                    reason: format!("branch {i} `when` has unknown op `{other}`"),
                })
            }
        }
        branches.push(Branch {
            field,
            op,
            value: when.get("value").cloned().unwrap_or(Value::Null),
            route,
        });
    }
    let branches = Arc::new(branches);

    Ok(sync_handler(move |state: GraphState| {
        let root = Value::Object(state.channels.clone().into_iter().collect());
        let chosen = branches
            .iter()
            .find(|branch| predicate_holds(&root, &branch.field, &branch.op, &branch.value))
            .map(|branch| branch.route.clone())
            .unwrap_or_else(|| default_route.clone());
        NodeOutput::update(single(&into, Value::String(chosen)))
    }))
}

/// Evaluate a single field/op/value predicate against a JSON root, resolving
/// `field` as a dotted path. Shared between `conditionalRouter` branches and the
/// `metadataFilter` op vocabulary.
fn predicate_holds(root: &Value, field: &str, op: &str, value: &Value) -> bool {
    let resolved = resolve_path(root, field);
    match op {
        "exists" => resolved.is_some(),
        "absent" => resolved.is_none(),
        "equals" => resolved.map(value_to_text).as_deref() == Some(&value_to_text(value)),
        "notEquals" => resolved.map(value_to_text).as_deref() != Some(&value_to_text(value)),
        "contains" => resolved
            .map(value_to_text)
            .is_some_and(|t| t.contains(&value_to_text(value))),
        "gt" | "gte" | "lt" | "lte" => {
            let (Some(n), Some(target)) = (resolved.and_then(Value::as_f64), value.as_f64()) else {
                return false;
            };
            match op {
                "gt" => n > target,
                "gte" => n >= target,
                "lt" => n < target,
                "lte" => n <= target,
                _ => false,
            }
        }
        _ => false,
    }
}

// --- documentWriter ----------------------------------------------------------

/// `documentWriter { from, into, store?, dedupeBy? }` — append the documents on
/// the `from` array channel into an in-state document store array on `into`,
/// returning the accumulated store.
///
/// Pure and deterministic — a write to an in-memory (graph-state) store rather
/// than a vendor vector DB. The existing `into`/`store` channel value (when an
/// array) is treated as the current store; new docs are appended after it.
/// With `dedupeBy` set, the merged store is de-duplicated by that object field
/// (keeping the first occurrence). `store` defaults to `into` when omitted.
fn build_document_writer(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "documentWriter";
    let from = require_string(kind, params, "from")?;
    let into = require_string(kind, params, "into")?;
    let store = optional_string(kind, params, "store")?.unwrap_or_else(|| into.clone());
    let dedupe_by = optional_string(kind, params, "dedupeBy")?;

    Ok(sync_handler(move |state: GraphState| {
        let mut docs: Vec<Value> = match state.channels.get(&store) {
            Some(Value::Array(existing)) => existing.clone(),
            _ => Vec::new(),
        };
        if let Some(Value::Array(incoming)) = state.channels.get(&from) {
            docs.extend(incoming.iter().cloned());
        }
        let docs = match &dedupe_by {
            Some(field) => dedupe_array(docs, Some(field)),
            None => docs,
        };
        NodeOutput::update(single(&into, Value::Array(docs)))
    }))
}

// --- council (ADR 0061 E2) ---------------------------------------------------

/// FNV-1a 32-bit hash — for the deterministic (replay-faithful) anonymize shuffle.
fn fnv1a(text: &str) -> u32 {
    let mut hash: u32 = 2166136261;
    for byte in text.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    hash
}

/// The label at position `i`: `A..Z`, then `M{i}` beyond 26 seats.
fn council_label(i: usize) -> String {
    if i < 26 {
        ((b'A' + i as u8) as char).to_string()
    } else {
        format!("M{i}")
    }
}

/// Read an agent-result channel's answer text (string verbatim, else its `content`/`output` field).
fn council_content(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(map)) => map
            .get("content")
            .and_then(Value::as_str)
            .or_else(|| map.get("output").and_then(Value::as_str))
            .map(str::to_owned)
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// Parse a reviewer reply into an ordered list of labels it names (deduped, unknown dropped). Only
/// whole words exactly equal to a label count (case-sensitive): the article "a" is not label `A`,
/// and "Answer" is not label `A` either. Mirrors the TS `parseRanking`.
fn council_parse_ranking(text: &str, labels: &BTreeSet<String>) -> Vec<String> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut out: Vec<String> = Vec::new();
    for token in text.split(|c: char| !c.is_ascii_alphanumeric()) {
        if labels.contains(token) && seen.insert(token) {
            out.push(token.to_owned());
        }
    }
    out
}

/// `councilAnonymize { fromChannels: [member channels], into, keyInto?, seed? }` — strip authorship,
/// relabel A/B/C, and shuffle deterministically by `seed` so a reviewer can't favour its own answer
/// (ADR 0013). `into` receives `{ label, content }` items only: what a reviewer may see. `keyInto`,
/// when set, receives the `{ label, memberId }` key (`memberId` = the source channel) for the audit
/// trail's de-anonymization after ranking; it is never meant to be shown to a reviewer.
fn build_council_anonymize(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "councilAnonymize";
    let into = require_string(kind, params, "into")?;
    let key_into = optional_string(kind, params, "keyInto")?;
    let seed = optional_string(kind, params, "seed")?.unwrap_or_else(|| "council".to_string());
    let channels = Arc::new(require_string_array(kind, params, "fromChannels")?);

    Ok(sync_handler(move |state: GraphState| {
        let mut members: Vec<(String, String)> = channels
            .iter()
            .map(|name| (name.clone(), council_content(state.channels.get(name))))
            .collect();
        members.sort_by(|a, b| {
            let ha = fnv1a(&format!("{seed}:{}", a.0));
            let hb = fnv1a(&format!("{seed}:{}", b.0));
            ha.cmp(&hb).then_with(|| a.0.cmp(&b.0))
        });
        let mut field: Vec<Value> = Vec::with_capacity(members.len());
        let mut key: Vec<Value> = Vec::with_capacity(members.len());
        for (i, (member_id, content)) in members.into_iter().enumerate() {
            let label = council_label(i);
            field.push(json!({ "label": label, "content": content }));
            key.push(json!({ "label": label, "memberId": member_id }));
        }
        let mut update = single(&into, Value::Array(field));
        if let Some(key_into) = &key_into {
            update.insert(key_into.clone(), Value::Array(key));
        }
        NodeOutput::update(update)
    }))
}

/// `councilAggregate { reviewsFrom: [reviewer channels], fieldFrom, into }` — Borda-aggregate the
/// reviewers' rankings of the anonymized `fieldFrom` labels into a consensus order (best-first). A
/// label at position `p` of `n` scores `n - p`; ties break by label asc (deterministic).
fn build_council_aggregate(params: &Value) -> Result<NodeHandler, ComponentError> {
    let kind = "councilAggregate";
    let into = require_string(kind, params, "into")?;
    let field_from = require_string(kind, params, "fieldFrom")?;
    let reviews = Arc::new(require_string_array(kind, params, "reviewsFrom")?);

    Ok(sync_handler(move |state: GraphState| {
        let labels: Vec<String> = match state.channels.get(&field_from) {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|item| item.get("label").and_then(Value::as_str).map(str::to_owned))
                .collect(),
            _ => Vec::new(),
        };
        let label_set: BTreeSet<String> = labels.iter().cloned().collect();
        let mut scores: BTreeMap<String, f64> = labels.iter().map(|l| (l.clone(), 0.0)).collect();
        for name in reviews.iter() {
            let ranking =
                council_parse_ranking(&council_content(state.channels.get(name)), &label_set);
            let n = ranking.len();
            for (position, label) in ranking.iter().enumerate() {
                *scores.entry(label.clone()).or_insert(0.0) += (n - position) as f64;
            }
        }
        let mut ordered = labels.clone();
        ordered.sort_by(|a, b| {
            let sa = scores.get(a).copied().unwrap_or(0.0);
            let sb = scores.get(b).copied().unwrap_or(0.0);
            sb.partial_cmp(&sa)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.cmp(b))
        });
        NodeOutput::update(single(&into, json!(ordered)))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ailu_graph_core::{GraphId, GraphStatus, NodeId, RunId};
    use ailu_graph_runtime::NodeHandler;

    /// Build a `GraphState` with the given channels for driving a handler.
    fn state_with(channels: BTreeMap<String, Value>) -> GraphState {
        GraphState {
            run_id: RunId::from("run-1"),
            graph_id: GraphId::from("graph-1"),
            current_node_id: NodeId::from("node-1"),
            status: GraphStatus::Running,
            channels,
            version: 1,
            checkpoint_id: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    /// Drive a handler to completion on a current-thread tokio runtime and
    /// return its [`NodeOutput`].
    fn run(handler: &NodeHandler, channels: BTreeMap<String, Value>) -> NodeOutput {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime builds");
        rt.block_on(handler(state_with(channels)))
    }

    fn channels(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    /// Extract the `Err` of a build, asserting it was not `Ok` without requiring
    /// `NodeHandler: Debug` (which it is not).
    fn build_err(kind: &str, params: &Value) -> ComponentError {
        match ComponentRegistry::new().build_handler(kind, params) {
            Ok(_) => panic!("expected build_handler({kind}) to fail"),
            Err(e) => e,
        }
    }

    #[test]
    fn unknown_kind_is_rejected() {
        assert_eq!(
            build_err("nope", &json!({})),
            ComponentError::UnknownKind("nope".to_string())
        );
    }

    #[test]
    fn value_to_text_unwraps_agent_result_final_answer() {
        // An agent node writes an AgentResult; downstream components want the final answer, not the
        // stringified wrapper (ADR 0060/0061 — the answerBuilder / outputParser unwrap fix).
        let agent_result = json!({
            "reasoning": "thought:It lists the projects.\nfinal:Next, GitHub, CLI.",
            "approvalRequests": [],
            "requiresHumanReview": false
        });
        assert_eq!(value_to_text(&agent_result), "Next, GitHub, CLI.");
    }

    #[test]
    fn value_to_text_prefers_structured_output_when_present() {
        let agent_result = json!({
            "reasoning": "thought:x\nfinal:ignored",
            "structuredOutput": { "answer": "structured" }
        });
        // Structured output wins over the `final:` tail; it round-trips as compact JSON.
        assert_eq!(value_to_text(&agent_result), "{\"answer\":\"structured\"}");
    }

    #[test]
    fn value_to_text_uses_whole_reasoning_without_a_final_marker() {
        let agent_result =
            json!({ "reasoning": "The facts do not answer.", "approvalRequests": [] });
        assert_eq!(value_to_text(&agent_result), "The facts do not answer.");
    }

    #[test]
    fn value_to_text_keeps_a_non_agentresult_object_as_json() {
        let plain = json!({ "id": "x", "content": "hi" });
        assert_eq!(value_to_text(&plain), plain.to_string());
    }

    #[test]
    fn value_to_text_passes_strings_and_null_through() {
        assert_eq!(value_to_text(&json!("hello")), "hello");
        assert_eq!(value_to_text(&Value::Null), "");
    }

    #[test]
    fn registry_lists_all_thirty_one_kinds() {
        assert_eq!(ComponentRegistry::kinds().len(), 31);
        // The seventeen wave-one kinds plus the eleven wave-two kinds plus
        // semanticRetriever plus the two council kinds, in declaration order.
        assert_eq!(
            ComponentRegistry::kinds(),
            &[
                "promptBuilder",
                "jsonValidator",
                "outputParser",
                "router",
                "retriever",
                "semanticRetriever",
                "reranker",
                "textCleaner",
                "documentSplitter",
                "htmlToText",
                "csvParser",
                "documentJoiner",
                "deduplicator",
                "truncator",
                "regexExtractor",
                "answerBuilder",
                "fieldMapper",
                "fieldExtractor",
                "bm25Retriever",
                "keywordRetriever",
                "sentenceWindowSplitter",
                "languageDetector",
                "metadataFilter",
                "listJoiner",
                "mergeRanker",
                "evaluator",
                "chatMessageBuilder",
                "conditionalRouter",
                "documentWriter",
                "councilAnonymize",
                "councilAggregate",
            ]
        );
    }

    /// Every kind in `kinds()` must build with some valid params, so the list and
    /// the `build_handler` match cannot drift apart.
    #[test]
    fn every_listed_kind_builds() {
        let registry = ComponentRegistry::new();
        for kind in ComponentRegistry::kinds() {
            let params = sample_params(kind);
            assert!(
                registry.build_handler(kind, &params).is_ok(),
                "kind `{kind}` should build with its sample params"
            );
        }
    }

    /// Minimal valid params per kind, used to smoke-test that every listed kind
    /// builds. Only the wave-two kinds need entries; the wave-one kinds are
    /// covered by their own tests but included for completeness.
    fn sample_params(kind: &str) -> Value {
        match kind {
            "promptBuilder" => json!({ "template": "{{x}}", "into": "o" }),
            "jsonValidator" => json!({ "from": "f", "okInto": "ok", "errorsInto": "e" }),
            "outputParser" => json!({ "from": "f", "into": "o" }),
            "router" => json!({ "from": "f", "rules": [], "defaultRoute": "d", "into": "o" }),
            "retriever" => json!({ "query": "q", "into": "o", "docs": [] }),
            "semanticRetriever" => {
                json!({ "queryEmbeddingFrom": "q", "chunksFrom": "c", "into": "o" })
            }
            "reranker" => json!({ "from": "f", "into": "o" }),
            "textCleaner" => json!({ "from": "f", "into": "o" }),
            "documentSplitter" => json!({ "from": "f", "into": "o", "by": "chars", "size": 4 }),
            "htmlToText" => json!({ "from": "f", "into": "o" }),
            "csvParser" => json!({ "from": "f", "into": "o" }),
            "documentJoiner" => json!({ "fromChannels": ["a"], "into": "o" }),
            "deduplicator" => json!({ "from": "f", "into": "o" }),
            "truncator" => json!({ "from": "f", "into": "o", "maxChars": 5 }),
            "regexExtractor" => json!({ "from": "f", "into": "o", "pattern": "x" }),
            "answerBuilder" => json!({ "from": "f", "into": "o" }),
            "fieldMapper" => json!({ "from": "f", "into": "o", "mapping": {} }),
            "fieldExtractor" => json!({ "from": "f", "into": "o" }),
            "bm25Retriever" => json!({ "query": "q", "into": "o", "docs": [] }),
            "keywordRetriever" => json!({ "query": "q", "into": "o", "docs": [] }),
            "sentenceWindowSplitter" => json!({ "from": "f", "into": "o" }),
            "languageDetector" => json!({ "from": "f", "into": "o" }),
            "metadataFilter" => json!({ "from": "f", "into": "o", "field": "k", "op": "exists" }),
            "listJoiner" => json!({ "fromChannels": ["a"], "into": "o" }),
            "mergeRanker" => json!({ "fromChannels": ["a"], "into": "o" }),
            "evaluator" => json!({ "expectedFrom": "e", "actualFrom": "a", "into": "o" }),
            "chatMessageBuilder" => {
                json!({ "into": "o", "messages": [{ "role": "user", "content": "hi" }] })
            }
            "conditionalRouter" => json!({ "into": "o", "defaultRoute": "d", "branches": [] }),
            "documentWriter" => json!({ "from": "f", "into": "o" }),
            "councilAnonymize" => json!({ "fromChannels": ["m0"], "into": "field" }),
            "councilAggregate" => {
                json!({ "reviewsFrom": ["r0"], "fieldFrom": "field", "into": "agg" })
            }
            other => panic!("no sample params for kind `{other}`"),
        }
    }

    #[test]
    fn missing_param_is_reported() {
        assert_eq!(
            build_err("promptBuilder", &json!({ "into": "out" })),
            ComponentError::MissingParam {
                kind: "promptBuilder".to_string(),
                param: "template".to_string()
            }
        );
    }

    #[test]
    fn invalid_param_type_is_reported() {
        let err = build_err("promptBuilder", &json!({ "template": 42, "into": "out" }));
        assert!(matches!(err, ComponentError::InvalidParam { .. }));
    }

    // --- promptBuilder -------------------------------------------------------

    #[test]
    fn prompt_builder_renders_placeholders() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "promptBuilder",
                &json!({ "template": "Hello {{ name }}, you are {{role}}.", "into": "prompt" }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[("name", json!("Ada")), ("role", json!("admin"))]),
        );
        assert_eq!(
            out.update.get("prompt"),
            Some(&Value::String("Hello Ada, you are admin.".to_string()))
        );
    }

    #[test]
    fn prompt_builder_unknown_placeholder_is_empty_and_non_strings_stringify() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "promptBuilder",
                &json!({ "template": "[{{missing}}] count={{count}}", "into": "p" }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("count", json!(3))]));
        assert_eq!(
            out.update.get("p"),
            Some(&Value::String("[] count=3".to_string()))
        );
    }

    #[test]
    fn prompt_builder_assembles_a_retrieval_capsule_when_configured() {
        // ADR 0044 D3 (ailu#578): the exact set that influenced the model, captured at the
        // node that actually templates the prompt.
        let handler = ComponentRegistry::new()
            .build_handler(
                "promptBuilder",
                &json!({
                    "template": "Facts:\n{{reranked}}\n\nQuestion: {{question}}",
                    "into": "prompt",
                    "capsule": {
                        "into": "capsule",
                        "chunksFrom": "reranked",
                        "queryFrom": "question",
                        "discardedFrom": ["fusedDiscarded"]
                    }
                }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[
                ("question", json!("what is cat")),
                (
                    "reranked",
                    json!([
                        { "id": "d1", "content": "cat facts", "score": 0.9,
                          "provenance": [{ "stage": "rerank", "algorithm": "cross-encoder", "algorithmVersion": null, "score": 0.9 }] }
                    ]),
                ),
                (
                    "fusedDiscarded",
                    json!([
                        { "id": "d2", "content": "unrelated", "score": 0.1,
                          "provenance": [{ "stage": "rrf", "algorithm": "rrf", "algorithmVersion": "k=60", "score": 0.1 }],
                          "discardedAt": "1700000000000", "reason": "rrf_rank_below_k" }
                    ]),
                ),
            ]),
        );

        let prompt = out.update.get("prompt").and_then(Value::as_str).unwrap();
        assert!(prompt.starts_with("Facts:\n"));
        assert!(prompt.contains("cat facts"));
        assert!(prompt.ends_with("Question: what is cat"));

        let capsule = out.update.get("capsule").unwrap();
        assert_eq!(capsule.get("order").unwrap(), &json!(["d1"]));
        let chunks = capsule.get("chunks").and_then(Value::as_array).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].get("id").unwrap(), "d1");
        assert_eq!(
            chunks[0]
                .get("provenance")
                .and_then(Value::as_array)
                .unwrap()
                .len(),
            1
        );
        let discarded = capsule.get("discarded").and_then(Value::as_array).unwrap();
        assert_eq!(discarded.len(), 1);
        assert_eq!(discarded[0].get("id").unwrap(), "d2");
        assert_eq!(discarded[0].get("reason").unwrap(), "rrf_rank_below_k");
        // Both hashes are non-empty hex strings, and differ (different inputs).
        let rendered_hash = capsule.get("renderedHash").and_then(Value::as_str).unwrap();
        let query_hash = capsule.get("queryHash").and_then(Value::as_str).unwrap();
        assert_eq!(rendered_hash.len(), 64);
        assert_eq!(query_hash.len(), 64);
        assert_ne!(rendered_hash, query_hash);
    }

    #[test]
    fn prompt_builder_without_capsule_param_writes_only_the_rendered_prompt() {
        // No `capsule` param -> unchanged behavior, no extra channel written.
        let handler = ComponentRegistry::new()
            .build_handler(
                "promptBuilder",
                &json!({ "template": "hi {{name}}", "into": "prompt" }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("name", json!("Ada"))]));
        assert_eq!(out.update.len(), 1);
        assert!(out.update.contains_key("prompt"));
    }

    #[test]
    fn prompt_builder_capsule_hash_changes_with_the_rendered_text() {
        // Proves renderedHash is over the ACTUAL interpolated output, not a static/raw value.
        let spec = json!({
            "template": "hi {{name}}",
            "into": "prompt",
            "capsule": { "into": "capsule", "chunksFrom": "chunks", "queryFrom": "q" }
        });
        let registry = ComponentRegistry::new();
        let a = run(
            &registry.build_handler("promptBuilder", &spec).unwrap(),
            channels(&[
                ("name", json!("Ada")),
                ("q", json!("x")),
                ("chunks", json!([])),
            ]),
        );
        let b = run(
            &registry.build_handler("promptBuilder", &spec).unwrap(),
            channels(&[
                ("name", json!("Bob")),
                ("q", json!("x")),
                ("chunks", json!([])),
            ]),
        );
        let hash_a = a
            .update
            .get("capsule")
            .and_then(|c| c.get("renderedHash"))
            .and_then(Value::as_str)
            .unwrap();
        let hash_b = b
            .update
            .get("capsule")
            .and_then(|c| c.get("renderedHash"))
            .and_then(Value::as_str)
            .unwrap();
        assert_ne!(hash_a, hash_b);
    }

    // --- jsonValidator -------------------------------------------------------

    #[test]
    fn json_validator_passes_valid_object() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "jsonValidator",
                &json!({
                    "from": "payload",
                    "requiredKeys": ["a", "b"],
                    "expectType": "object",
                    "okInto": "ok",
                    "errorsInto": "errs"
                }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[("payload", json!({ "a": 1, "b": 2 }))]),
        );
        assert_eq!(out.update.get("ok"), Some(&Value::Bool(true)));
        assert_eq!(out.update.get("errs"), Some(&Value::Array(vec![])));
    }

    #[test]
    fn json_validator_reports_missing_keys_and_wrong_type() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "jsonValidator",
                &json!({
                    "from": "payload",
                    "requiredKeys": ["a", "b"],
                    "expectType": "object",
                    "okInto": "ok",
                    "errorsInto": "errs"
                }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("payload", json!({ "a": 1 }))]));
        assert_eq!(out.update.get("ok"), Some(&Value::Bool(false)));
        let errs = out.update.get("errs").and_then(Value::as_array).unwrap();
        assert_eq!(errs.len(), 1);
        assert!(errs[0]
            .as_str()
            .unwrap()
            .contains("missing required key `b`"));
    }

    #[test]
    fn json_validator_flags_type_mismatch() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "jsonValidator",
                &json!({ "from": "v", "expectType": "array", "okInto": "ok", "errorsInto": "e" }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("v", json!("a string"))]));
        assert_eq!(out.update.get("ok"), Some(&Value::Bool(false)));
        let errs = out.update.get("e").and_then(Value::as_array).unwrap();
        assert!(errs[0].as_str().unwrap().contains("expected type `array`"));
    }

    // --- outputParser --------------------------------------------------------

    #[test]
    fn output_parser_extracts_object_from_prose() {
        let handler = ComponentRegistry::new()
            .build_handler("outputParser", &json!({ "from": "text", "into": "parsed" }))
            .unwrap();
        let out = run(
            &handler,
            channels(&[(
                "text",
                json!("Sure! Here is the result:\n```json\n{ \"ok\": true, \"n\": 7 }\n``` done"),
            )]),
        );
        assert_eq!(
            out.update.get("parsed"),
            Some(&json!({ "ok": true, "n": 7 }))
        );
    }

    #[test]
    fn output_parser_extracts_array_and_handles_nested_braces() {
        let handler = ComponentRegistry::new()
            .build_handler("outputParser", &json!({ "from": "text", "into": "parsed" }))
            .unwrap();
        let out = run(
            &handler,
            channels(&[("text", json!("noise [ {\"k\": \"}\"}, 2 ] tail"))]),
        );
        assert_eq!(out.update.get("parsed"), Some(&json!([{ "k": "}" }, 2])));
    }

    #[test]
    fn output_parser_writes_null_when_no_json() {
        let handler = ComponentRegistry::new()
            .build_handler("outputParser", &json!({ "from": "text", "into": "parsed" }))
            .unwrap();
        let out = run(&handler, channels(&[("text", json!("just words"))]));
        assert_eq!(out.update.get("parsed"), Some(&Value::Null));
    }

    // --- router --------------------------------------------------------------

    #[test]
    fn router_picks_equals_rule() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "router",
                &json!({
                    "from": "label",
                    "rules": [
                        { "equals": "spam", "route": "drop" },
                        { "contains": "urgent", "route": "escalate" }
                    ],
                    "defaultRoute": "inbox",
                    "into": "route"
                }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("label", json!("spam"))]));
        assert_eq!(out.update.get("route"), Some(&json!("drop")));
    }

    #[test]
    fn router_picks_contains_rule_and_falls_back_to_default() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "router",
                &json!({
                    "from": "label",
                    "rules": [
                        { "equals": "spam", "route": "drop" },
                        { "contains": "urgent", "route": "escalate" }
                    ],
                    "defaultRoute": "inbox",
                    "into": "route"
                }),
            )
            .unwrap();
        let escalate = run(&handler, channels(&[("label", json!("this is urgent!"))]));
        assert_eq!(escalate.update.get("route"), Some(&json!("escalate")));
        let fallback = run(&handler, channels(&[("label", json!("hello"))]));
        assert_eq!(fallback.update.get("route"), Some(&json!("inbox")));
    }

    // --- semanticRetriever ---------------------------------------------------

    #[test]
    fn semantic_retriever_ranks_by_cosine_over_real_embeddings() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "semanticRetriever",
                &json!({
                    "queryEmbeddingFrom": "qvec",
                    "chunksFrom": "corpus",
                    "into": "hits",
                    "k": 2
                }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[
                ("qvec", json!([1.0, 0.0, 0.0])),
                (
                    "corpus",
                    json!([
                        { "id": "near", "content": "aligned", "embedding": [0.9, 0.1, 0.0] },
                        { "id": "far", "content": "orthogonal", "embedding": [0.0, 1.0, 0.0] }
                    ]),
                ),
            ]),
        );
        let hits = out
            .update
            .get("hits")
            .and_then(|v| v.as_array())
            .expect("hits array");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].get("id"), Some(&json!("near")));
        let top = hits[0]
            .get("score")
            .and_then(serde_json::Value::as_f64)
            .unwrap();
        let bottom = hits[1]
            .get("score")
            .and_then(serde_json::Value::as_f64)
            .unwrap();
        assert!(top > bottom, "aligned chunk must outrank orthogonal one");
    }

    // --- retriever -----------------------------------------------------------

    #[test]
    fn retriever_returns_top_k_scored_results() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "retriever",
                &json!({
                    "query": "q",
                    "into": "hits",
                    "k": 2,
                    "docs": [
                        { "id": "d1", "content": "critical risk alert" },
                        { "id": "d2", "content": "general weather update" },
                        { "id": "d3", "content": "critical risk warning" }
                    ]
                }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("q", json!("critical risk"))]));
        let hits = out.update.get("hits").and_then(Value::as_array).unwrap();
        assert_eq!(hits.len(), 2);
        // Each hit has id/content/score and scores are descending.
        let s0 = hits[0].get("score").and_then(Value::as_f64).unwrap();
        let s1 = hits[1].get("score").and_then(Value::as_f64).unwrap();
        assert!(s0 >= s1);
        assert!(hits[0].get("id").is_some());
        assert!(hits[0].get("content").is_some());
    }

    #[test]
    fn retriever_is_deterministic() {
        let registry = ComponentRegistry::new();
        let spec = json!({
            "query": "q",
            "into": "hits",
            "k": 3,
            "docs": [
                { "id": "d1", "content": "alpha beta" },
                { "id": "d2", "content": "gamma delta" }
            ]
        });
        let a = run(
            &registry.build_handler("retriever", &spec).unwrap(),
            channels(&[("q", json!("alpha"))]),
        );
        let b = run(
            &registry.build_handler("retriever", &spec).unwrap(),
            channels(&[("q", json!("alpha"))]),
        );
        assert_eq!(a.update.get("hits"), b.update.get("hits"));
    }

    // --- reranker ------------------------------------------------------------

    #[test]
    fn reranker_sorts_by_existing_score_without_query() {
        let handler = ComponentRegistry::new()
            .build_handler("reranker", &json!({ "from": "hits", "into": "ranked" }))
            .unwrap();
        let out = run(
            &handler,
            channels(&[(
                "hits",
                json!([
                    { "id": "a", "content": "x", "score": 0.1 },
                    { "id": "b", "content": "y", "score": 0.9 },
                    { "id": "c", "content": "z", "score": 0.5 }
                ]),
            )]),
        );
        let ranked = out.update.get("ranked").and_then(Value::as_array).unwrap();
        let ids: Vec<&str> = ranked
            .iter()
            .map(|r| r.get("id").unwrap().as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["b", "c", "a"]);
    }

    #[test]
    fn reranker_appends_a_provenance_step_and_preserves_prior_lineage() {
        // ADR 0044 (ailu#578): the mock-fallback reranker (no AILU_RERANK_ENDPOINT) had the
        // same score-overwrite-with-no-record gap as bm25Retriever/mergeRanker before D1.
        let handler = ComponentRegistry::new()
            .build_handler("reranker", &json!({ "from": "hits", "into": "ranked" }))
            .unwrap();
        let out = run(
            &handler,
            channels(&[(
                "hits",
                json!([
                    { "id": "a", "content": "x", "score": 0.9,
                      "provenance": [{ "stage": "rrf", "algorithm": "rrf", "algorithmVersion": "k=60", "score": 0.9 }] }
                ]),
            )]),
        );
        let ranked = out.update.get("ranked").and_then(Value::as_array).unwrap();
        let provenance = ranked[0]
            .get("provenance")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(provenance.len(), 2);
        let stages: Vec<&str> = provenance
            .iter()
            .map(|s| s.get("stage").unwrap().as_str().unwrap())
            .collect();
        assert_eq!(stages, vec!["rrf", "rerank"]);
        assert_eq!(provenance[1].get("algorithm").unwrap(), "score-passthrough");
    }

    #[test]
    fn reranker_with_a_query_but_no_cross_encoder_keeps_the_upstream_ranking() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "reranker",
                &json!({ "from": "hits", "into": "ranked", "query": "q" }),
            )
            .unwrap();
        let input = channels(&[
            ("q", json!("critical risk")),
            (
                "hits",
                json!([
                    { "id": "a", "content": "general update", "score": 0.9 },
                    { "id": "b", "content": "critical risk alert", "score": 0.1 }
                ]),
            ),
        ]);
        let out = run(&handler, input);
        let ranked = out.update.get("ranked").and_then(Value::as_array).unwrap();
        // No placeholder re-score: the upstream order (by existing score) is kept.
        let ids: Vec<&str> = ranked
            .iter()
            .map(|item| item.get("id").and_then(Value::as_str).unwrap())
            .collect();
        assert_eq!(ids, vec!["a", "b"]);
        assert_eq!(ranked[0].get("score").and_then(Value::as_f64), Some(0.9));
    }

    #[test]
    fn reranker_tolerates_empty_or_missing_channel() {
        let handler = ComponentRegistry::new()
            .build_handler("reranker", &json!({ "from": "hits", "into": "ranked" }))
            .unwrap();
        let out = run(&handler, channels(&[]));
        assert_eq!(out.update.get("ranked"), Some(&Value::Array(vec![])));
    }

    // --- textCleaner ---------------------------------------------------------

    #[test]
    fn text_cleaner_applies_all_transforms_in_order() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "textCleaner",
                &json!({
                    "from": "raw",
                    "into": "clean",
                    "lowercase": true,
                    "stripHtml": true,
                    "collapseWhitespace": true,
                    "trim": true
                }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[("raw", json!("  <b>Hello</b>   WORLD  "))]),
        );
        assert_eq!(
            out.update.get("clean"),
            Some(&Value::String("hello world".to_string()))
        );
    }

    #[test]
    fn text_cleaner_defaults_are_no_ops_and_missing_channel_is_empty() {
        let handler = ComponentRegistry::new()
            .build_handler("textCleaner", &json!({ "from": "raw", "into": "clean" }))
            .unwrap();
        // No transforms: passthrough.
        let out = run(&handler, channels(&[("raw", json!("  Mixed Case  "))]));
        assert_eq!(
            out.update.get("clean"),
            Some(&Value::String("  Mixed Case  ".to_string()))
        );
        // Missing channel -> empty string.
        let empty = run(&handler, channels(&[]));
        assert_eq!(
            empty.update.get("clean"),
            Some(&Value::String(String::new()))
        );
    }

    // --- documentSplitter ----------------------------------------------------

    #[test]
    fn document_splitter_by_chars_with_overlap() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "documentSplitter",
                &json!({ "from": "doc", "into": "chunks", "by": "chars", "size": 4, "overlap": 1 }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("doc", json!("abcdefgh"))]));
        let chunks = out.update.get("chunks").and_then(Value::as_array).unwrap();
        // size 4, step 3 (overlap 1): windows [0..4], [3..7], [6..8] — each window
        // repeats one char of the previous, and the tail is a shorter final chunk.
        let got: Vec<&str> = chunks.iter().map(|c| c.as_str().unwrap()).collect();
        assert_eq!(got, vec!["abcd", "defg", "gh"]);
    }

    #[test]
    fn document_splitter_by_sentences() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "documentSplitter",
                &json!({ "from": "doc", "into": "chunks", "by": "sentences", "size": 2 }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[("doc", json!("One. Two! Three? Four."))]),
        );
        let chunks = out.update.get("chunks").and_then(Value::as_array).unwrap();
        let got: Vec<&str> = chunks.iter().map(|c| c.as_str().unwrap()).collect();
        assert_eq!(got, vec!["One. Two!", "Three? Four."]);
    }

    #[test]
    fn document_splitter_empty_input_yields_empty_array() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "documentSplitter",
                &json!({ "from": "doc", "into": "chunks", "by": "chars", "size": 3 }),
            )
            .unwrap();
        let out = run(&handler, channels(&[]));
        assert_eq!(out.update.get("chunks"), Some(&Value::Array(vec![])));
    }

    #[test]
    fn document_splitter_rejects_overlap_not_smaller_than_size() {
        let err = build_err(
            "documentSplitter",
            &json!({ "from": "d", "into": "c", "by": "chars", "size": 3, "overlap": 3 }),
        );
        assert!(matches!(err, ComponentError::InvalidParam { param, .. } if param == "overlap"));
    }

    #[test]
    fn document_splitter_rejects_unknown_by() {
        let err = build_err(
            "documentSplitter",
            &json!({ "from": "d", "into": "c", "by": "words", "size": 3 }),
        );
        assert!(matches!(err, ComponentError::InvalidParam { param, .. } if param == "by"));
    }

    // --- htmlToText ----------------------------------------------------------

    #[test]
    fn html_to_text_strips_tags_and_decodes_entities() {
        let handler = ComponentRegistry::new()
            .build_handler("htmlToText", &json!({ "from": "html", "into": "text" }))
            .unwrap();
        let out = run(
            &handler,
            channels(&[(
                "html",
                json!("<p>Tom &amp; Jerry say &lt;hi&gt; &quot;there&quot;</p>"),
            )]),
        );
        assert_eq!(
            out.update.get("text"),
            Some(&Value::String("Tom & Jerry say <hi> \"there\"".to_string()))
        );
    }

    #[test]
    fn html_to_text_amp_decodes_last_so_no_double_decode() {
        let handler = ComponentRegistry::new()
            .build_handler("htmlToText", &json!({ "from": "html", "into": "text" }))
            .unwrap();
        // `&amp;lt;` should become `&lt;`, not `<`.
        let out = run(&handler, channels(&[("html", json!("&amp;lt;"))]));
        assert_eq!(
            out.update.get("text"),
            Some(&Value::String("&lt;".to_string()))
        );
    }

    // --- csvParser -----------------------------------------------------------

    #[test]
    fn csv_parser_with_header_yields_objects() {
        let handler = ComponentRegistry::new()
            .build_handler("csvParser", &json!({ "from": "csv", "into": "rows" }))
            .unwrap();
        let out = run(
            &handler,
            channels(&[("csv", json!("name,age\nAda,36\nBob,40"))]),
        );
        let rows = out.update.get("rows").and_then(Value::as_array).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], json!({ "name": "Ada", "age": "36" }));
        assert_eq!(rows[1], json!({ "name": "Bob", "age": "40" }));
    }

    #[test]
    fn csv_parser_without_header_yields_arrays_and_custom_delimiter() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "csvParser",
                &json!({ "from": "csv", "into": "rows", "header": false, "delimiter": ";" }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("csv", json!("a;b;c\n1;2;3"))]));
        let rows = out.update.get("rows").and_then(Value::as_array).unwrap();
        assert_eq!(rows[0], json!(["a", "b", "c"]));
        assert_eq!(rows[1], json!(["1", "2", "3"]));
    }

    #[test]
    fn csv_parser_empty_input_is_empty_array() {
        let handler = ComponentRegistry::new()
            .build_handler("csvParser", &json!({ "from": "csv", "into": "rows" }))
            .unwrap();
        let out = run(&handler, channels(&[]));
        assert_eq!(out.update.get("rows"), Some(&Value::Array(vec![])));
    }

    #[test]
    fn csv_parser_rejects_multi_char_delimiter() {
        let err = build_err(
            "csvParser",
            &json!({ "from": "csv", "into": "rows", "delimiter": "::" }),
        );
        assert!(matches!(err, ComponentError::InvalidParam { param, .. } if param == "delimiter"));
    }

    // --- documentJoiner ------------------------------------------------------

    #[test]
    fn document_joiner_merges_arrays_in_channel_order() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "documentJoiner",
                &json!({ "fromChannels": ["a", "b"], "into": "merged" }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[("a", json!([1, 2])), ("b", json!([3, 4]))]),
        );
        assert_eq!(out.update.get("merged"), Some(&json!([1, 2, 3, 4])));
    }

    #[test]
    fn document_joiner_dedupes_by_field() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "documentJoiner",
                &json!({ "fromChannels": ["a", "b"], "into": "merged", "dedupeBy": "id" }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[
                ("a", json!([{ "id": "x", "v": 1 }, { "id": "y", "v": 2 }])),
                ("b", json!([{ "id": "x", "v": 9 }, { "id": "z", "v": 3 }])),
            ]),
        );
        let merged = out.update.get("merged").and_then(Value::as_array).unwrap();
        let ids: Vec<&str> = merged
            .iter()
            .map(|m| m.get("id").unwrap().as_str().unwrap())
            .collect();
        // First "x" wins; order preserved.
        assert_eq!(ids, vec!["x", "y", "z"]);
        assert_eq!(merged[0].get("v").unwrap(), &json!(1));
    }

    #[test]
    fn document_joiner_ignores_missing_and_non_array_channels() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "documentJoiner",
                &json!({ "fromChannels": ["a", "missing", "scalar"], "into": "merged" }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[("a", json!([1])), ("scalar", json!("not an array"))]),
        );
        assert_eq!(out.update.get("merged"), Some(&json!([1])));
    }

    // --- deduplicator --------------------------------------------------------

    #[test]
    fn deduplicator_by_whole_value_preserves_first_occurrence() {
        let handler = ComponentRegistry::new()
            .build_handler("deduplicator", &json!({ "from": "items", "into": "out" }))
            .unwrap();
        let out = run(
            &handler,
            channels(&[("items", json!(["a", "b", "a", "c", "b"]))]),
        );
        assert_eq!(out.update.get("out"), Some(&json!(["a", "b", "c"])));
    }

    #[test]
    fn deduplicator_by_key_field() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "deduplicator",
                &json!({ "from": "items", "into": "out", "key": "id" }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[(
                "items",
                json!([{ "id": 1, "n": "a" }, { "id": 1, "n": "b" }, { "id": 2, "n": "c" }]),
            )]),
        );
        let deduped = out.update.get("out").and_then(Value::as_array).unwrap();
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].get("n").unwrap(), &json!("a"));
        assert_eq!(deduped[1].get("n").unwrap(), &json!("c"));
    }

    #[test]
    fn deduplicator_missing_channel_is_empty_array() {
        let handler = ComponentRegistry::new()
            .build_handler("deduplicator", &json!({ "from": "items", "into": "out" }))
            .unwrap();
        let out = run(&handler, channels(&[]));
        assert_eq!(out.update.get("out"), Some(&Value::Array(vec![])));
    }

    // --- truncator -----------------------------------------------------------

    #[test]
    fn truncator_truncates_and_appends_ellipsis() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "truncator",
                &json!({ "from": "text", "into": "out", "maxChars": 10, "ellipsis": "..." }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("text", json!("abcdefghijklmnop"))]));
        // keep 10 - 3 = 7 chars + "..." = 10 chars total
        assert_eq!(
            out.update.get("out"),
            Some(&Value::String("abcdefg...".to_string()))
        );
    }

    #[test]
    fn truncator_passes_through_short_text_and_counts_unicode() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "truncator",
                &json!({ "from": "text", "into": "out", "maxChars": 5 }),
            )
            .unwrap();
        // Within limit (default ellipsis is the single char "…").
        let short = run(&handler, channels(&[("text", json!("héllo"))]));
        assert_eq!(
            short.update.get("out"),
            Some(&Value::String("héllo".to_string()))
        );
        // Over limit: 5 - 1 (ellipsis) = 4 kept chars then "…".
        let long = run(&handler, channels(&[("text", json!("héllo world"))]));
        assert_eq!(
            long.update.get("out"),
            Some(&Value::String("héll…".to_string()))
        );
    }

    // --- regexExtractor ------------------------------------------------------

    #[test]
    fn regex_extractor_first_literal_match() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "regexExtractor",
                &json!({ "from": "text", "into": "out", "pattern": "error" }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[("text", json!("warning then error then error"))]),
        );
        assert_eq!(out.update.get("out"), Some(&json!("error")));
    }

    #[test]
    fn regex_extractor_all_returns_every_occurrence() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "regexExtractor",
                &json!({ "from": "text", "into": "out", "pattern": "ab", "all": true }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("text", json!("ab cab dab"))]));
        assert_eq!(out.update.get("out"), Some(&json!(["ab", "ab", "ab"])));
    }

    #[test]
    fn regex_extractor_anchors_and_no_match() {
        let registry = ComponentRegistry::new();
        let start = registry
            .build_handler(
                "regexExtractor",
                &json!({ "from": "t", "into": "out", "pattern": "^foo" }),
            )
            .unwrap();
        assert_eq!(
            run(&start, channels(&[("t", json!("foobar"))]))
                .update
                .get("out"),
            Some(&json!("foo"))
        );
        assert_eq!(
            run(&start, channels(&[("t", json!("barfoo"))]))
                .update
                .get("out"),
            Some(&Value::Null)
        );
        let end = registry
            .build_handler(
                "regexExtractor",
                &json!({ "from": "t", "into": "out", "pattern": "bar$" }),
            )
            .unwrap();
        assert_eq!(
            run(&end, channels(&[("t", json!("foobar"))]))
                .update
                .get("out"),
            Some(&json!("bar"))
        );
    }

    #[test]
    fn regex_extractor_nonzero_group_yields_no_match() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "regexExtractor",
                &json!({ "from": "t", "into": "out", "pattern": "x", "group": 1 }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("t", json!("xxx"))]));
        assert_eq!(out.update.get("out"), Some(&Value::Null));
    }

    #[test]
    fn regex_extractor_rejects_empty_pattern() {
        let err = build_err(
            "regexExtractor",
            &json!({ "from": "t", "into": "out", "pattern": "" }),
        );
        assert!(matches!(err, ComponentError::InvalidParam { param, .. } if param == "pattern"));
    }

    // --- answerBuilder -------------------------------------------------------

    #[test]
    fn answer_builder_appends_citations_with_default_layout() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "answerBuilder",
                &json!({ "from": "answer", "contextFrom": "ctx", "into": "final" }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[
                ("answer", json!("The sky is blue.")),
                (
                    "ctx",
                    json!([
                        { "id": "d1", "content": "Rayleigh scattering." },
                        { "id": "d2", "content": "Sunlight is white." }
                    ]),
                ),
            ]),
        );
        assert_eq!(
            out.update.get("final"),
            Some(&Value::String(
                "The sky is blue.\n\nSources:\n[1] d1: Rayleigh scattering.\n[2] d2: Sunlight is white."
                    .to_string()
            ))
        );
    }

    #[test]
    fn answer_builder_without_context_is_just_the_answer() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "answerBuilder",
                &json!({ "from": "answer", "into": "final" }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("answer", json!("Done."))]));
        assert_eq!(out.update.get("final"), Some(&json!("Done.")));
    }

    #[test]
    fn answer_builder_uses_template_placeholders() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "answerBuilder",
                &json!({
                    "from": "answer",
                    "contextFrom": "ctx",
                    "into": "final",
                    "template": "{{answer}} ||| {{citations}}"
                }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[
                ("answer", json!("Hi")),
                ("ctx", json!([{ "id": "d1", "content": "ref" }])),
            ]),
        );
        assert_eq!(
            out.update.get("final"),
            Some(&Value::String("Hi ||| [1] d1: ref".to_string()))
        );
    }

    // --- fieldMapper ---------------------------------------------------------

    #[test]
    fn field_mapper_remaps_nested_paths() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "fieldMapper",
                &json!({
                    "from": "src",
                    "into": "out",
                    "mapping": { "fullName": "user.name", "city": "address.city" }
                }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[(
                "src",
                json!({ "user": { "name": "Ada" }, "address": { "city": "London" } }),
            )]),
        );
        assert_eq!(
            out.update.get("out"),
            Some(&json!({ "fullName": "Ada", "city": "London" }))
        );
    }

    #[test]
    fn field_mapper_unresolved_path_is_null() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "fieldMapper",
                &json!({ "from": "src", "into": "out", "mapping": { "x": "a.missing" } }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("src", json!({ "a": { "b": 1 } }))]));
        assert_eq!(out.update.get("out"), Some(&json!({ "x": Value::Null })));
    }

    #[test]
    fn field_mapper_missing_source_resolves_to_nulls() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "fieldMapper",
                &json!({ "from": "src", "into": "out", "mapping": { "x": "a", "y": "b" } }),
            )
            .unwrap();
        let out = run(&handler, channels(&[]));
        assert_eq!(
            out.update.get("out"),
            Some(&json!({ "x": Value::Null, "y": Value::Null }))
        );
    }

    // --- fieldExtractor ------------------------------------------------------

    #[test]
    fn field_extractor_follows_dotted_path() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "fieldExtractor",
                &json!({ "from": "src", "into": "out", "path": "user.name" }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[("src", json!({ "user": { "name": "Ada" } }))]),
        );
        assert_eq!(out.update.get("out"), Some(&json!("Ada")));
    }

    #[test]
    fn field_extractor_without_path_passes_value_through() {
        let handler = ComponentRegistry::new()
            .build_handler("fieldExtractor", &json!({ "from": "src", "into": "out" }))
            .unwrap();
        let out = run(&handler, channels(&[("src", json!({ "a": 1 }))]));
        assert_eq!(out.update.get("out"), Some(&json!({ "a": 1 })));
    }

    #[test]
    fn field_extractor_unresolved_path_is_null() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "fieldExtractor",
                &json!({ "from": "src", "into": "out", "path": "a.missing" }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("src", json!({ "a": { "b": 1 } }))]));
        assert_eq!(out.update.get("out"), Some(&Value::Null));
    }

    #[test]
    fn field_extractor_final_only_reduces_reasoning_to_final_answer() {
        // The AgentResult.reasoning shape: a multi-line trace whose last line is
        // `final:<answer>`. finalOnly extracts just the answer text.
        let reasoning = "thought: I should ground my answer in the context.\n\
                         final:Ailu checkpoints after every node [checkpointing].";
        let handler = ComponentRegistry::new()
            .build_handler(
                "fieldExtractor",
                &json!({
                    "from": "ragResult",
                    "into": "finalAnswer",
                    "path": "reasoning",
                    "finalOnly": true
                }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[("ragResult", json!({ "reasoning": reasoning }))]),
        );
        assert_eq!(
            out.update.get("finalAnswer"),
            Some(&json!("Ailu checkpoints after every node [checkpointing]."))
        );
    }

    #[test]
    fn field_extractor_final_only_uses_the_last_marker() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "fieldExtractor",
                &json!({ "from": "src", "into": "out", "finalOnly": true }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[("src", json!("final:first\nfinal:the real answer"))]),
        );
        assert_eq!(out.update.get("out"), Some(&json!("the real answer")));
    }

    #[test]
    fn field_extractor_final_only_without_marker_returns_text_unchanged() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "fieldExtractor",
                &json!({ "from": "src", "into": "out", "finalOnly": true }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("src", json!("just a plain answer"))]));
        assert_eq!(out.update.get("out"), Some(&json!("just a plain answer")));
    }

    #[test]
    fn field_extractor_final_only_leaves_non_strings_alone() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "fieldExtractor",
                &json!({ "from": "src", "into": "out", "path": "n", "finalOnly": true }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("src", json!({ "n": 42 }))]));
        assert_eq!(out.update.get("out"), Some(&json!(42)));
    }

    #[test]
    fn field_extractor_missing_channel_is_null() {
        let handler = ComponentRegistry::new()
            .build_handler("fieldExtractor", &json!({ "from": "src", "into": "out" }))
            .unwrap();
        let out = run(&handler, channels(&[]));
        assert_eq!(out.update.get("out"), Some(&Value::Null));
    }

    // --- bm25Retriever -------------------------------------------------------

    #[test]
    fn bm25_retriever_ranks_lexical_overlap_first() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "bm25Retriever",
                &json!({
                    "query": "q",
                    "into": "hits",
                    "k": 2,
                    "docs": [
                        { "id": "d1", "content": "the cat sat on the mat" },
                        { "id": "d2", "content": "quantum field theory lecture" },
                        { "id": "d3", "content": "a cat and a dog" }
                    ]
                }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("q", json!("cat"))]));
        let hits = out.update.get("hits").and_then(Value::as_array).unwrap();
        assert_eq!(hits.len(), 2);
        // Both returned docs mention "cat"; the unrelated d2 is excluded.
        let ids: Vec<&str> = hits
            .iter()
            .map(|h| h.get("id").unwrap().as_str().unwrap())
            .collect();
        assert!(ids.contains(&"d1") && ids.contains(&"d3"));
        assert!(!ids.contains(&"d2"));
        // Scores are descending.
        let s0 = hits[0].get("score").and_then(Value::as_f64).unwrap();
        let s1 = hits[1].get("score").and_then(Value::as_f64).unwrap();
        assert!(s0 >= s1);
    }

    #[test]
    fn bm25_retriever_attaches_a_provenance_step_instead_of_a_bare_score() {
        // ADR 0044 (ailu#578): score initial + algorithme/version must survive alongside the
        // top-level `score`, not be the only record of how a candidate was scored.
        let handler = ComponentRegistry::new()
            .build_handler(
                "bm25Retriever",
                &json!({
                    "query": "q",
                    "into": "hits",
                    "k": 1,
                    "k1": 1.5,
                    "b": 0.6,
                    "docs": [{ "id": "d1", "content": "cat" }]
                }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("q", json!("cat"))]));
        let hits = out.update.get("hits").and_then(Value::as_array).unwrap();
        let score = hits[0].get("score").and_then(Value::as_f64).unwrap();
        let provenance = hits[0].get("provenance").and_then(Value::as_array).unwrap();
        assert_eq!(provenance.len(), 1);
        assert_eq!(provenance[0].get("stage").unwrap(), "bm25");
        assert_eq!(provenance[0].get("algorithm").unwrap(), "bm25");
        assert_eq!(
            provenance[0].get("algorithmVersion").unwrap(),
            "k1=1.5,b=0.6"
        );
        assert_eq!(
            provenance[0].get("score").and_then(Value::as_f64).unwrap(),
            score
        );
    }

    #[test]
    fn bm25_retriever_writes_the_rank_k_loser_to_a_discarded_channel_with_a_reason() {
        // ADR 0044 D2 (ailu#578): everything past `k` used to be dropped with zero trace.
        let handler = ComponentRegistry::new()
            .build_handler(
                "bm25Retriever",
                &json!({
                    "query": "q",
                    "into": "hits",
                    "k": 1,
                    "docs": [
                        { "id": "d1", "content": "cat cat cat" },
                        { "id": "d2", "content": "cat" }
                    ]
                }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("q", json!("cat"))]));
        let hits = out.update.get("hits").and_then(Value::as_array).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].get("id").unwrap(), "d1");

        let discarded = out
            .update
            .get("hitsDiscarded")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(discarded.len(), 1);
        assert_eq!(discarded[0].get("id").unwrap(), "d2");
        assert_eq!(discarded[0].get("reason").unwrap(), "bm25_rank_below_k");
        assert!(discarded[0].get("discardedAt").unwrap().is_string());
        // The discarded item still carries its own provenance — not just a bare drop record.
        let provenance = discarded[0]
            .get("provenance")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(provenance[0].get("stage").unwrap(), "bm25");
    }

    #[test]
    fn bm25_retriever_is_deterministic_and_falls_back_to_literal_query() {
        let registry = ComponentRegistry::new();
        // No "q" channel: the literal `query` param is used.
        let spec = json!({
            "query": "cat",
            "into": "hits",
            "docs": [ { "id": "d1", "content": "cat" }, { "id": "d2", "content": "dog" } ]
        });
        let a = run(
            &registry.build_handler("bm25Retriever", &spec).unwrap(),
            channels(&[]),
        );
        let b = run(
            &registry.build_handler("bm25Retriever", &spec).unwrap(),
            channels(&[]),
        );
        assert_eq!(a.update.get("hits"), b.update.get("hits"));
        let hits = a.update.get("hits").and_then(Value::as_array).unwrap();
        assert_eq!(hits[0].get("id").unwrap().as_str().unwrap(), "d1");
    }

    // --- keywordRetriever ----------------------------------------------------

    #[test]
    fn keyword_retriever_scores_by_query_term_coverage() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "keywordRetriever",
                &json!({
                    "query": "q",
                    "into": "hits",
                    "docs": [
                        { "id": "d1", "content": "red green blue" },
                        { "id": "d2", "content": "red yellow" }
                    ]
                }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("q", json!("red green"))]));
        let hits = out.update.get("hits").and_then(Value::as_array).unwrap();
        // d1 covers both query terms (1.0); d2 covers one of two (0.5).
        assert_eq!(hits[0].get("id").unwrap().as_str().unwrap(), "d1");
        assert_eq!(hits[0].get("score").and_then(Value::as_f64).unwrap(), 1.0);
        assert_eq!(hits[1].get("score").and_then(Value::as_f64).unwrap(), 0.5);
    }

    // --- sentenceWindowSplitter ----------------------------------------------

    #[test]
    fn sentence_window_splitter_overlaps_by_stride() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "sentenceWindowSplitter",
                &json!({ "from": "doc", "into": "win", "windowSize": 2, "stride": 1 }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[("doc", json!("One. Two. Three. Four."))]),
        );
        let win = out.update.get("win").and_then(Value::as_array).unwrap();
        let got: Vec<&str> = win.iter().map(|w| w.as_str().unwrap()).collect();
        // size 2, stride 1 over 4 sentences: [0..2], [1..3], [2..4].
        assert_eq!(got, vec!["One. Two.", "Two. Three.", "Three. Four."]);
    }

    #[test]
    fn sentence_window_splitter_empty_input_and_invalid_stride() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "sentenceWindowSplitter",
                &json!({ "from": "d", "into": "w" }),
            )
            .unwrap();
        assert_eq!(
            run(&handler, channels(&[])).update.get("w"),
            Some(&Value::Array(vec![]))
        );
        let err = build_err(
            "sentenceWindowSplitter",
            &json!({ "from": "d", "into": "w", "windowSize": 2, "stride": 3 }),
        );
        assert!(matches!(err, ComponentError::InvalidParam { param, .. } if param == "stride"));
    }

    // --- languageDetector ----------------------------------------------------

    #[test]
    fn language_detector_picks_dominant_language() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "languageDetector",
                &json!({ "from": "txt", "into": "lang", "confidenceInto": "conf" }),
            )
            .unwrap();
        let en = run(
            &handler,
            channels(&[("txt", json!("the cat is in the house and it is warm"))]),
        );
        assert_eq!(en.update.get("lang"), Some(&json!("en")));
        assert!(en.update.get("conf").and_then(Value::as_f64).unwrap() > 0.0);

        let fr = run(
            &handler,
            channels(&[("txt", json!("le chat est dans la maison et il fait chaud"))]),
        );
        assert_eq!(fr.update.get("lang"), Some(&json!("fr")));
    }

    #[test]
    fn language_detector_returns_und_with_no_hits() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "languageDetector",
                &json!({ "from": "txt", "into": "lang" }),
            )
            .unwrap();
        let out = run(&handler, channels(&[("txt", json!("zzz qqq wxyz"))]));
        assert_eq!(out.update.get("lang"), Some(&json!("und")));
    }

    // --- metadataFilter ------------------------------------------------------

    #[test]
    fn metadata_filter_equals_and_numeric() {
        let registry = ComponentRegistry::new();
        let items = json!([
            { "id": "a", "lang": "en", "score": 0.9 },
            { "id": "b", "lang": "fr", "score": 0.2 },
            { "id": "c", "lang": "en", "score": 0.5 }
        ]);

        let eq = registry
            .build_handler(
                "metadataFilter",
                &json!({ "from": "docs", "into": "out", "field": "lang", "op": "equals", "value": "en" }),
            )
            .unwrap();
        let out = run(&eq, channels(&[("docs", items.clone())]));
        let kept = out.update.get("out").and_then(Value::as_array).unwrap();
        assert_eq!(kept.len(), 2);

        let gt = registry
            .build_handler(
                "metadataFilter",
                &json!({ "from": "docs", "into": "out", "field": "score", "op": "gte", "value": 0.5 }),
            )
            .unwrap();
        let out = run(&gt, channels(&[("docs", items)]));
        let kept = out.update.get("out").and_then(Value::as_array).unwrap();
        let ids: Vec<&str> = kept
            .iter()
            .map(|d| d.get("id").unwrap().as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["a", "c"]);
    }

    #[test]
    fn metadata_filter_requires_value_for_value_ops_and_rejects_unknown_op() {
        assert!(matches!(
            build_err(
                "metadataFilter",
                &json!({ "from": "d", "into": "o", "field": "k", "op": "equals" }),
            ),
            ComponentError::MissingParam { param, .. } if param == "value"
        ));
        assert!(matches!(
            build_err(
                "metadataFilter",
                &json!({ "from": "d", "into": "o", "field": "k", "op": "matches", "value": "x" }),
            ),
            ComponentError::InvalidParam { param, .. } if param == "op"
        ));
    }

    // --- listJoiner ----------------------------------------------------------

    #[test]
    fn list_joiner_modes() {
        let registry = ComponentRegistry::new();
        let input = channels(&[("a", json!([1, 2])), ("b", json!([2, 3]))]);

        let concat = registry
            .build_handler(
                "listJoiner",
                &json!({ "fromChannels": ["a", "b"], "into": "o" }),
            )
            .unwrap();
        assert_eq!(
            run(&concat, input.clone()).update.get("o"),
            Some(&json!([1, 2, 2, 3]))
        );

        let union = registry
            .build_handler(
                "listJoiner",
                &json!({ "fromChannels": ["a", "b"], "into": "o", "mode": "union" }),
            )
            .unwrap();
        assert_eq!(
            run(&union, input.clone()).update.get("o"),
            Some(&json!([1, 2, 3]))
        );

        let interleave = registry
            .build_handler(
                "listJoiner",
                &json!({ "fromChannels": ["a", "b"], "into": "o", "mode": "interleave" }),
            )
            .unwrap();
        assert_eq!(
            run(&interleave, input).update.get("o"),
            Some(&json!([1, 2, 2, 3]))
        );
    }

    #[test]
    fn list_joiner_rejects_unknown_mode() {
        let err = build_err(
            "listJoiner",
            &json!({ "fromChannels": ["a"], "into": "o", "mode": "zip" }),
        );
        assert!(matches!(err, ComponentError::InvalidParam { param, .. } if param == "mode"));
    }

    // --- mergeRanker ---------------------------------------------------------

    #[test]
    fn merge_ranker_fuses_streams_with_rrf() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "mergeRanker",
                &json!({ "fromChannels": ["lex", "vec"], "into": "fused" }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[
                (
                    "lex",
                    json!([
                        { "id": "a", "content": "x" },
                        { "id": "b", "content": "y" }
                    ]),
                ),
                (
                    "vec",
                    json!([
                        { "id": "b", "content": "y" },
                        { "id": "c", "content": "z" }
                    ]),
                ),
            ]),
        );
        let fused = out.update.get("fused").and_then(Value::as_array).unwrap();
        // b appears in both lists (rank 1 + rank 0) so it should win the fusion.
        assert_eq!(fused[0].get("id").unwrap().as_str().unwrap(), "b");
        assert_eq!(fused.len(), 3);
        assert!(fused[0].get("score").and_then(Value::as_f64).unwrap() > 0.0);
    }

    #[test]
    fn merge_ranker_preserves_both_legs_provenance_for_a_doc_seen_in_two_channels() {
        // ADR 0044 (ailu#578): the pre-fix `representative` map kept only whichever channel was
        // processed FIRST, silently dropping the other leg's lineage entirely for a doc present in
        // both. Both legs' steps must survive, plus mergeRanker's own rrf step — none overwritten.
        let handler = ComponentRegistry::new()
            .build_handler(
                "mergeRanker",
                &json!({ "fromChannels": ["lex", "vec"], "into": "fused" }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[
                (
                    "lex",
                    json!([
                        { "id": "b", "content": "y", "score": 1.1,
                          "provenance": [{ "stage": "bm25", "algorithm": "bm25", "algorithmVersion": null, "score": 1.1 }] }
                    ]),
                ),
                (
                    "vec",
                    json!([
                        { "id": "b", "content": "y", "score": 0.9,
                          "provenance": [{ "stage": "vector", "algorithm": "cosine", "algorithmVersion": null, "score": 0.9 }] },
                        { "id": "c", "content": "z", "score": 0.5, "provenance": [] }
                    ]),
                ),
            ]),
        );
        let fused = out.update.get("fused").and_then(Value::as_array).unwrap();
        let b = fused.iter().find(|r| r.get("id").unwrap() == "b").unwrap();
        let b_provenance = b.get("provenance").and_then(Value::as_array).unwrap();
        assert_eq!(b_provenance.len(), 3);
        let stages: Vec<&str> = b_provenance
            .iter()
            .map(|s| s.get("stage").unwrap().as_str().unwrap())
            .collect();
        assert_eq!(stages, vec!["bm25", "vector", "rrf"]);

        let c = fused.iter().find(|r| r.get("id").unwrap() == "c").unwrap();
        let c_provenance = c.get("provenance").and_then(Value::as_array).unwrap();
        let c_stages: Vec<&str> = c_provenance
            .iter()
            .map(|s| s.get("stage").unwrap().as_str().unwrap())
            .collect();
        assert_eq!(c_stages, vec!["rrf"]);
    }

    #[test]
    fn merge_ranker_writes_rrf_losers_to_a_discarded_channel_with_a_reason() {
        // ADR 0044 D2 (ailu#578): fused candidates past `k` used to be dropped with zero trace.
        let handler = ComponentRegistry::new()
            .build_handler(
                "mergeRanker",
                &json!({ "fromChannels": ["lex", "vec"], "into": "fused", "k": 1 }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[
                (
                    "lex",
                    json!([
                        { "id": "b", "content": "y", "score": 1.1, "provenance": [] }
                    ]),
                ),
                (
                    "vec",
                    json!([
                        { "id": "b", "content": "y", "score": 0.9, "provenance": [] },
                        { "id": "c", "content": "z", "score": 0.5, "provenance": [] }
                    ]),
                ),
            ]),
        );
        let fused = out.update.get("fused").and_then(Value::as_array).unwrap();
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].get("id").unwrap(), "b");

        let discarded = out
            .update
            .get("fusedDiscarded")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(discarded.len(), 1);
        assert_eq!(discarded[0].get("id").unwrap(), "c");
        assert_eq!(discarded[0].get("reason").unwrap(), "rrf_rank_below_k");
        assert!(discarded[0].get("discardedAt").unwrap().is_string());
        // The discarded item still carries its own rrf provenance step.
        let provenance = discarded[0]
            .get("provenance")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(provenance.last().unwrap().get("stage").unwrap(), "rrf");
    }

    #[test]
    fn merge_ranker_with_no_k_limit_discards_nothing() {
        // No `k` means keep everything — the discarded channel is written but empty, not omitted,
        // so a downstream reader can always rely on `{into}Discarded` existing.
        let handler = ComponentRegistry::new()
            .build_handler(
                "mergeRanker",
                &json!({ "fromChannels": ["lex"], "into": "fused" }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[(
                "lex",
                json!([{ "id": "a", "content": "x", "score": 1.0, "provenance": [] }]),
            )]),
        );
        let discarded = out
            .update
            .get("fusedDiscarded")
            .and_then(Value::as_array)
            .unwrap();
        assert!(discarded.is_empty());
    }

    // --- evaluator -----------------------------------------------------------

    #[test]
    fn evaluator_token_f1_and_pass_threshold() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "evaluator",
                &json!({
                    "expectedFrom": "exp",
                    "actualFrom": "act",
                    "into": "score",
                    "passInto": "passed",
                    "threshold": 0.5
                }),
            )
            .unwrap();
        // Identical token sets -> F1 of 1.0 -> passes.
        let perfect = run(
            &handler,
            channels(&[
                ("exp", json!("the quick brown fox")),
                ("act", json!("the quick brown fox")),
            ]),
        );
        assert_eq!(
            perfect.update.get("score").and_then(Value::as_f64),
            Some(1.0)
        );
        assert_eq!(perfect.update.get("passed"), Some(&Value::Bool(true)));

        // No overlap -> 0.0 -> fails.
        let miss = run(
            &handler,
            channels(&[("exp", json!("alpha beta")), ("act", json!("gamma delta"))]),
        );
        assert_eq!(miss.update.get("score").and_then(Value::as_f64), Some(0.0));
        assert_eq!(miss.update.get("passed"), Some(&Value::Bool(false)));
    }

    #[test]
    fn evaluator_exact_and_overlap_metrics() {
        let registry = ComponentRegistry::new();
        let exact = registry
            .build_handler(
                "evaluator",
                &json!({ "expectedFrom": "e", "actualFrom": "a", "into": "s", "metric": "exact" }),
            )
            .unwrap();
        let out = run(
            &exact,
            channels(&[("e", json!("  hello  ")), ("a", json!("hello"))]),
        );
        // `exact` trims before comparing.
        assert_eq!(out.update.get("s").and_then(Value::as_f64), Some(1.0));

        let overlap = registry
            .build_handler(
                "evaluator",
                &json!({ "expectedFrom": "e", "actualFrom": "a", "into": "s", "metric": "overlap" }),
            )
            .unwrap();
        // sets {a,b} vs {b,c}: intersection 1, union 3 -> 1/3.
        let out = run(
            &overlap,
            channels(&[("e", json!("a b")), ("a", json!("b c"))]),
        );
        let s = out.update.get("s").and_then(Value::as_f64).unwrap();
        assert!((s - 1.0 / 3.0).abs() < 1e-9);
    }

    // --- chatMessageBuilder --------------------------------------------------

    #[test]
    fn chat_message_builder_assembles_roles_and_templates() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "chatMessageBuilder",
                &json!({
                    "into": "messages",
                    "systemFrom": "sys",
                    "messages": [
                        { "role": "user", "content": "Hello {{name}}" },
                        { "role": "assistant", "contentFrom": "reply" }
                    ]
                }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[
                ("sys", json!("You are helpful.")),
                ("name", json!("Ada")),
                ("reply", json!("Hi Ada!")),
            ]),
        );
        assert_eq!(
            out.update.get("messages"),
            Some(&json!([
                { "role": "system", "content": "You are helpful." },
                { "role": "user", "content": "Hello Ada" },
                { "role": "assistant", "content": "Hi Ada!" }
            ]))
        );
    }

    #[test]
    fn chat_message_builder_skips_empty_system_and_rejects_bad_role() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "chatMessageBuilder",
                &json!({
                    "into": "messages",
                    "systemFrom": "sys",
                    "messages": [{ "role": "user", "content": "hi" }]
                }),
            )
            .unwrap();
        // No `sys` channel -> no leading system message.
        let out = run(&handler, channels(&[]));
        assert_eq!(
            out.update.get("messages"),
            Some(&json!([{ "role": "user", "content": "hi" }]))
        );

        let err = build_err(
            "chatMessageBuilder",
            &json!({ "into": "m", "messages": [{ "role": "boss", "content": "x" }] }),
        );
        assert!(matches!(err, ComponentError::InvalidParam { param, .. } if param == "messages"));
    }

    // --- conditionalRouter ---------------------------------------------------

    #[test]
    fn conditional_router_picks_first_matching_branch() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "conditionalRouter",
                &json!({
                    "into": "route",
                    "defaultRoute": "fallback",
                    "branches": [
                        { "when": { "field": "score", "op": "gte", "value": 0.8 }, "route": "high" },
                        { "when": { "field": "lang", "op": "equals", "value": "fr" }, "route": "french" }
                    ]
                }),
            )
            .unwrap();
        assert_eq!(
            run(&handler, channels(&[("score", json!(0.9))]))
                .update
                .get("route"),
            Some(&json!("high"))
        );
        assert_eq!(
            run(
                &handler,
                channels(&[("score", json!(0.1)), ("lang", json!("fr"))])
            )
            .update
            .get("route"),
            Some(&json!("french"))
        );
        assert_eq!(
            run(&handler, channels(&[("score", json!(0.1))]))
                .update
                .get("route"),
            Some(&json!("fallback"))
        );
    }

    #[test]
    fn conditional_router_rejects_unknown_op() {
        let err = build_err(
            "conditionalRouter",
            &json!({
                "into": "r",
                "defaultRoute": "d",
                "branches": [{ "when": { "field": "x", "op": "matches", "value": 1 }, "route": "a" }]
            }),
        );
        assert!(matches!(err, ComponentError::InvalidParam { param, .. } if param == "branches"));
    }

    // --- documentWriter ------------------------------------------------------

    #[test]
    fn document_writer_appends_to_existing_store() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "documentWriter",
                &json!({ "from": "incoming", "into": "store" }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[
                ("store", json!([{ "id": "a" }])),
                ("incoming", json!([{ "id": "b" }, { "id": "c" }])),
            ]),
        );
        assert_eq!(
            out.update.get("store"),
            Some(&json!([{ "id": "a" }, { "id": "b" }, { "id": "c" }]))
        );
    }

    #[test]
    fn document_writer_dedupes_by_field_keeping_first() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "documentWriter",
                &json!({ "from": "incoming", "into": "store", "dedupeBy": "id" }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[
                ("store", json!([{ "id": "a", "v": 1 }])),
                (
                    "incoming",
                    json!([{ "id": "a", "v": 9 }, { "id": "b", "v": 2 }]),
                ),
            ]),
        );
        let store = out.update.get("store").and_then(Value::as_array).unwrap();
        assert_eq!(store.len(), 2);
        // First "a" (v=1) is kept.
        assert_eq!(store[0].get("v").unwrap(), &json!(1));
        assert_eq!(store[1].get("id").unwrap(), &json!("b"));
    }

    #[test]
    fn council_anonymize_relabels_shuffles_and_hides_the_author() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "councilAnonymize",
                &json!({
                    "fromChannels": ["member_0", "member_1"],
                    "into": "field",
                    "keyInto": "fieldKey",
                    "seed": "s"
                }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[
                ("member_0", json!({ "content": "alpha" })),
                ("member_1", json!({ "content": "beta" })),
            ]),
        );
        let field = out.update.get("field").and_then(Value::as_array).unwrap();
        assert_eq!(field.len(), 2);
        // Relabeled A/B, content preserved, and nothing in the field names the author.
        let labels: Vec<&str> = field
            .iter()
            .map(|f| f.get("label").and_then(Value::as_str).unwrap())
            .collect();
        assert_eq!(labels, vec!["A", "B"]);
        assert!(
            field.iter().all(|f| f.get("memberId").is_none()),
            "{field:?}"
        );
        // The audit key maps each label back to its member.
        let key = out
            .update
            .get("fieldKey")
            .and_then(Value::as_array)
            .unwrap();
        let member_ids: BTreeSet<&str> = key
            .iter()
            .map(|k| k.get("memberId").and_then(Value::as_str).unwrap())
            .collect();
        assert_eq!(member_ids, ["member_0", "member_1"].into_iter().collect());
    }

    #[test]
    fn council_aggregate_borda_orders_the_field() {
        let handler = ComponentRegistry::new()
            .build_handler(
                "councilAggregate",
                &json!({ "reviewsFrom": ["review_0", "review_1"], "fieldFrom": "field", "into": "aggregate" }),
            )
            .unwrap();
        let out = run(
            &handler,
            channels(&[
                ("field", json!([{ "label": "A" }, { "label": "B" }])),
                ("review_0", json!({ "content": "I rank B then A" })),
                ("review_1", json!({ "content": "B first, A second" })),
            ]),
        );
        let order: Vec<&str> = out
            .update
            .get("aggregate")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(order, vec!["B", "A"]);
    }

    #[test]
    fn council_parse_ranking_counts_whole_word_labels_only() {
        let labels: BTreeSet<String> = ["A", "B", "C"].iter().map(|l| (*l).to_owned()).collect();
        assert_eq!(
            council_parse_ranking("A is clearly better than B", &labels),
            vec!["A", "B"]
        );
        assert_eq!(
            council_parse_ranking("Answer B is the best", &labels),
            vec!["B"]
        );
        assert_eq!(
            council_parse_ranking("C gives a clearer answer than B", &labels),
            vec!["C", "B"]
        );
        assert_eq!(
            council_parse_ranking("I rank B first, then A, then C.", &labels),
            vec!["B", "A", "C"]
        );
    }
}
