//! Lenient raw-YAML → AST normalization — Rust mirror of
//! `packages/lang-ailu/src/parser/{parse-yaml,build-prompt-ast,build-agent-ast,build-chain-ast}.ts`.
//!
//! The builders never fail: like the TS `buildXxxAST`, every missing or
//! ill-typed field falls back to the same default (`""` for strings, `[]` for
//! string lists, `None` for an optional step input). The validator reports what
//! is actually wrong afterwards.

use serde_json::Value;

use crate::ast::{
    AgentAst, AgentKind, ChainAst, ChainKind, ChainStepAst, ChainStepKind, Loc, PromptAst,
    PromptKind,
};

/// Parse YAML into a generic [`serde_json::Value`]. Mirror of the TS
/// `parseYaml`: any parse failure becomes an error carrying the file label.
pub fn parse_yaml(content: &str, file: &str) -> Result<Value, String> {
    serde_yaml::from_str(content).map_err(|error| format!("Invalid YAML in {file}: {error}"))
}

fn as_string_or_empty(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or("").to_owned()
}

/// Filter an array to its string elements only (mirror of the TS
/// `Array.isArray(x) ? x.filter(v => typeof v === "string") : []`).
fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Build the prompt AST from raw parsed YAML. Mirror of the TS `buildPromptAST`.
pub fn build_prompt_ast(raw: &Value, file: &str) -> PromptAst {
    let input = raw.as_object();
    let get = |key: &str| input.and_then(|map| map.get(key));
    PromptAst {
        kind: PromptKind::Tag,
        loc: Loc::start_of(file),
        name: as_string_or_empty(get("name")),
        template: as_string_or_empty(get("template")),
        variables: string_list(get("variables")),
    }
}

/// Build the agent AST from raw parsed YAML. Mirror of the TS `buildAgentAST`.
pub fn build_agent_ast(raw: &Value, file: &str) -> AgentAst {
    let input = raw.as_object();
    let get = |key: &str| input.and_then(|map| map.get(key));
    AgentAst {
        kind: AgentKind::Tag,
        loc: Loc::start_of(file),
        id: as_string_or_empty(get("id")),
        description: as_string_or_empty(get("description")),
        prompt: as_string_or_empty(get("prompt")),
        tools: string_list(get("tools")),
    }
}

/// Build the chain AST from raw parsed YAML. Mirror of the TS `buildChainAST`:
/// a step's `input` survives only as a non-null, non-array object.
pub fn build_chain_ast(raw: &Value, file: &str) -> ChainAst {
    let input = raw.as_object();
    let get = |key: &str| input.and_then(|map| map.get(key));

    let empty: Vec<Value> = Vec::new();
    let steps_raw = get("steps").and_then(Value::as_array).unwrap_or(&empty);
    let steps = steps_raw
        .iter()
        .map(|step_raw| {
            let step = step_raw.as_object();
            let field = |key: &str| step.and_then(|map| map.get(key));
            ChainStepAst {
                kind: ChainStepKind::Tag,
                loc: Loc::start_of(file),
                agent_id: as_string_or_empty(field("agentId")),
                input: field("input").and_then(Value::as_object).cloned(),
            }
        })
        .collect();

    ChainAst {
        kind: ChainKind::Tag,
        loc: Loc::start_of(file),
        id: as_string_or_empty(get("id")),
        steps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_valid_yaml_and_builds_prompt_ast() {
        let raw = parse_yaml(
            "name: Greeting\ntemplate: \"Hello {{name}}\"\nvariables:\n  - name\n",
            "prompt.yaml",
        )
        .expect("valid yaml");
        let ast = build_prompt_ast(&raw, "prompt.yaml");
        assert_eq!(ast.kind, PromptKind::Tag);
        assert_eq!(ast.loc.file, "prompt.yaml");
        assert!(ast.template.contains("{{name}}"));
        assert_eq!(ast.variables, vec!["name".to_owned()]);
    }

    #[test]
    fn applies_prompt_defaults_for_missing_fields() {
        let ast = build_prompt_ast(&json!({}), "p.yaml");
        assert_eq!(ast.name, "");
        assert_eq!(ast.template, "");
        assert!(ast.variables.is_empty());
    }

    #[test]
    fn filters_non_string_variables_and_tools() {
        let prompt = build_prompt_ast(&json!({ "variables": ["a", 1, "b", null] }), "p.yaml");
        assert_eq!(prompt.variables, vec!["a".to_owned(), "b".to_owned()]);
        let agent = build_agent_ast(&json!({ "tools": [true, "search", 2] }), "a.yaml");
        assert_eq!(agent.tools, vec!["search".to_owned()]);
    }

    #[test]
    fn drops_non_object_step_input() {
        let ast = build_chain_ast(
            &json!({
                "id": "c",
                "steps": [
                    { "agentId": "a", "input": { "x": 1 } },
                    { "agentId": "b", "input": [1, 2] },
                    { "agentId": "c", "input": null },
                    { "agentId": "d" }
                ]
            }),
            "chain.yaml",
        );
        assert!(ast.steps[0].input.is_some());
        assert!(ast.steps[1].input.is_none());
        assert!(ast.steps[2].input.is_none());
        assert!(ast.steps[3].input.is_none());
    }

    #[test]
    fn parse_yaml_reports_file_label_on_failure() {
        let error = parse_yaml("id: c\nsteps: [ {agentId: a", "broken.yaml").unwrap_err();
        assert!(error.starts_with("Invalid YAML in broken.yaml:"));
    }
}
