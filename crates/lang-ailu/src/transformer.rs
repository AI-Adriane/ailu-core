//! AST → compiled-definition transforms plus the prompt template engine —
//! Rust mirror of
//! `packages/lang-ailu/src/transformer/{types,template-engine,transform-prompt,transform-agent,transform-chain}.ts`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ast::{AgentAst, ChainAst, Loc, PromptAst};
use crate::validator::{Diagnostic, DiagnosticCode, Severity};

/// Compiled prompt — mirror of the serializable fields of the TS
/// `PromptTemplate` (its `render` closure is a function and is dropped by
/// `JSON.stringify`, so it is exposed here as the free [`render_template`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptTemplate {
    pub name: String,
    pub template: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// Compiled agent — mirror of the TS `AgentConfig`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentConfig {
    pub id: String,
    pub description: String,
    pub prompt: String,
    pub tools: Vec<String>,
}

/// One step of a compiled chain — mirror of an element of the TS
/// `ChainDefinition.steps`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChainStep {
    #[serde(rename = "agentId")]
    pub agent_id: String,
    /// Dropped from the JSON when absent, matching `JSON.stringify` over an
    /// `undefined` `input` field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Map<String, Value>>,
}

/// Compiled chain — mirror of the TS `ChainDefinition`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChainDefinition {
    pub id: String,
    pub steps: Vec<ChainStep>,
}

/// The result of rendering a template — mirror of the TS
/// `{ content, diagnostics }` render return.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderResult {
    pub content: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// Transform a prompt AST into a [`PromptTemplate`]. Mirror of the TS
/// `transformPrompt`: the carried `diagnostics` are the undeclared-variable
/// warnings detected over the template against the declared variable list.
pub fn transform_prompt(ast: &PromptAst) -> PromptTemplate {
    PromptTemplate {
        name: ast.name.clone(),
        template: ast.template.clone(),
        diagnostics: detect_unresolved_template_variables(&ast.template, &ast.variables, &ast.loc),
    }
}

/// Transform an agent AST into an [`AgentConfig`]. Mirror of `transformAgent`.
pub fn transform_agent(ast: &AgentAst) -> AgentConfig {
    AgentConfig {
        id: ast.id.clone(),
        description: ast.description.clone(),
        prompt: ast.prompt.clone(),
        tools: ast.tools.clone(),
    }
}

/// Transform a chain AST into a [`ChainDefinition`]. Mirror of `transformChain`.
pub fn transform_chain(ast: &ChainAst) -> ChainDefinition {
    ChainDefinition {
        id: ast.id.clone(),
        steps: ast
            .steps
            .iter()
            .map(|step| ChainStep {
                agent_id: step.agent_id.clone(),
                input: step.input.clone(),
            })
            .collect(),
    }
}

/// Iterate `{{ ... }}` tokens, yielding each inner expression. Mirror of the TS
/// `/\{\{\s*([^}]+)\s*\}\}/g`: the inner run is one-or-more non-`}` characters
/// (so an empty `{{}}` does not match), and `String.replace` advances past the
/// full match each time.
fn for_each_token(template: &str, mut visit: impl FnMut(&str)) {
    let bytes = template.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'{' && bytes[i + 1] == b'{' {
            // Find the inner run of non-`}` characters, then the closing `}}`.
            let inner_start = i + 2;
            let mut j = inner_start;
            while j < bytes.len() && bytes[j] != b'}' {
                j += 1;
            }
            if j > inner_start && j + 1 < bytes.len() && bytes[j] == b'}' && bytes[j + 1] == b'}' {
                visit(&template[inner_start..j]);
                i = j + 2;
                continue;
            }
        }
        i += 1;
    }
}

/// Split a token expression into its trimmed name part and optional trimmed
/// filter part on the first `|`. Mirror of the TS `expr.split("|").map(trim)`.
fn split_expr(raw_expr: &str) -> (&str, Option<&str>) {
    let expr = raw_expr.trim();
    match expr.split_once('|') {
        Some((name, filter)) => (name.trim(), Some(filter.trim())),
        None => (expr, None),
    }
}

/// JS `String(value)` for the value shapes a YAML scalar can take. Strings pass
/// through verbatim; the others mirror `String()`'s primitive coercion.
fn js_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        // Arrays/objects are out of the documented template-variable contract;
        // mirror JS's `String([...])` / `String({...})` coercions defensively.
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if item.is_null() {
                    String::new()
                } else {
                    js_string(item)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

/// Render a template against a variable map. Mirror of the TS `renderTemplate`:
/// unknown variables push an `UNRESOLVED_VARIABLE` warning and render empty; a
/// `truncate:N` filter clips the stringified value to the first `N` characters.
pub fn render_template(
    template: &str,
    variables: &serde_json::Map<String, Value>,
    loc: &Loc,
) -> RenderResult {
    let mut diagnostics = Vec::new();
    let mut content = String::new();
    let mut last = 0usize;
    let bytes = template.as_bytes();
    let mut i = 0;

    while i + 1 < bytes.len() {
        if bytes[i] == b'{' && bytes[i + 1] == b'{' {
            let inner_start = i + 2;
            let mut j = inner_start;
            while j < bytes.len() && bytes[j] != b'}' {
                j += 1;
            }
            if j > inner_start && j + 1 < bytes.len() && bytes[j] == b'}' && bytes[j + 1] == b'}' {
                content.push_str(&template[last..i]);
                let (name_part, filter_part) = split_expr(&template[inner_start..j]);
                content.push_str(&render_token(
                    name_part,
                    filter_part,
                    variables,
                    loc,
                    &mut diagnostics,
                ));
                i = j + 2;
                last = i;
                continue;
            }
        }
        i += 1;
    }
    content.push_str(&template[last..]);

    RenderResult {
        content,
        diagnostics,
    }
}

fn render_token(
    name_part: &str,
    filter_part: Option<&str>,
    variables: &serde_json::Map<String, Value>,
    loc: &Loc,
    diagnostics: &mut Vec<Diagnostic>,
) -> String {
    if name_part.is_empty() {
        return String::new();
    }
    let Some(raw_value) = variables.get(name_part) else {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::UnresolvedVariable,
            message: format!("Variable '{name_part}' is not resolved."),
            loc: loc.clone(),
            severity: Severity::Warning,
        });
        return String::new();
    };
    let mut value = js_string(raw_value);
    if let Some(filter) = filter_part {
        if let Some(amount_raw) = filter.strip_prefix("truncate:") {
            // `Number.parseInt(x, 10)` reads a leading signed integer run.
            if let Some(amount) = parse_int_prefix(amount_raw.trim()) {
                if amount >= 0 {
                    value = truncate(&value, amount as usize);
                }
            }
        }
    }
    value
}

/// `value.length <= max ? value : value.slice(0, max)` over UTF-16-free input.
/// The TS `slice` counts UTF-16 code units; for the BMP-only inputs the DSL
/// targets this matches a `char`-count slice.
fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_owned()
    } else {
        value.chars().take(max).collect()
    }
}

/// `Number.parseInt(s, 10)` semantics: read an optional sign then a leading run
/// of ASCII digits; `None` mirrors the `NaN` case (which the caller treats as
/// "no truncation", since `Number.isFinite(NaN)` is false).
fn parse_int_prefix(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    let mut idx = 0;
    let mut sign = 1i64;
    if idx < bytes.len() && (bytes[idx] == b'+' || bytes[idx] == b'-') {
        if bytes[idx] == b'-' {
            sign = -1;
        }
        idx += 1;
    }
    let digits_start = idx;
    let mut acc: i64 = 0;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        acc = acc
            .saturating_mul(10)
            .saturating_add((bytes[idx] - b'0') as i64);
        idx += 1;
    }
    if idx == digits_start {
        return None;
    }
    Some(sign * acc)
}

/// Detect `{{ var }}` tokens that are not declared in `declared_variables`.
/// Mirror of the TS `detectUnresolvedTemplateVariables`.
pub fn detect_unresolved_template_variables(
    template: &str,
    declared_variables: &[String],
    loc: &Loc,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for_each_token(template, |raw_expr| {
        let name = split_expr(raw_expr).0;
        if !name.is_empty() && !declared_variables.iter().any(|v| v == name) {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::UndeclaredTemplateVariable,
                message: format!("Variable '{name}' is used but not declared in prompt variables."),
                loc: loc.clone(),
                severity: Severity::Warning,
            });
        }
    });
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::build_prompt_ast;
    use serde_json::json;

    fn vars(value: Value) -> serde_json::Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn transforms_and_renders_template_with_truncate_filter() {
        let ast = build_prompt_ast(
            &json!({ "name": "P", "template": "Hello {{name | truncate: 3}}", "variables": ["name"] }),
            "prompt.yaml",
        );
        let template = transform_prompt(&ast);
        let rendered = render_template(
            &template.template,
            &vars(json!({ "name": "Ailu" })),
            &ast.loc,
        );
        assert_eq!(rendered.content, "Hello Ail");
        assert!(rendered.diagnostics.is_empty());
    }

    #[test]
    fn reports_warning_for_unresolved_variable() {
        let ast = build_prompt_ast(
            &json!({ "name": "P", "template": "Hello {{missing}}", "variables": [] }),
            "prompt.yaml",
        );
        let template = transform_prompt(&ast);
        assert!(template
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Warning));
        let rendered = render_template(&template.template, &vars(json!({})), &ast.loc);
        assert_eq!(rendered.content, "Hello ");
        assert_eq!(
            rendered.diagnostics[0].code,
            DiagnosticCode::UnresolvedVariable
        );
    }

    #[test]
    fn render_leaves_short_values_untouched_by_truncate() {
        let result = render_template(
            "{{ x | truncate: 99 }}",
            &vars(json!({ "x": "hi" })),
            &Loc::start_of("p.yaml"),
        );
        assert_eq!(result.content, "hi");
    }

    #[test]
    fn detect_undeclared_template_variable_warns() {
        let diagnostics = detect_unresolved_template_variables(
            "Hi {{a}} and {{b}}",
            &["a".to_owned()],
            &Loc::start_of("p.yaml"),
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].code,
            DiagnosticCode::UndeclaredTemplateVariable
        );
        assert!(diagnostics[0].message.contains("'b'"));
    }

    #[test]
    fn transform_chain_preserves_step_input() {
        use crate::parser::build_chain_ast;
        let ast = build_chain_ast(
            &json!({ "id": "c", "steps": [{ "agentId": "a", "input": { "k": 1 } }, { "agentId": "b" }] }),
            "c.yaml",
        );
        let def = transform_chain(&ast);
        assert_eq!(
            def.steps[0].input.as_ref().unwrap().get("k"),
            Some(&json!(1))
        );
        assert!(def.steps[1].input.is_none());
    }
}
