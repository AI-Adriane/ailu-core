//! Compiler entry point — Rust mirror of
//! `packages/lang-ailu/src/compiler/compile-file.ts`.
//!
//! Pipeline: `serde_yaml` parse -> `detectKind` -> per-kind
//! `build*AST` -> `validate*AST` -> on no error-severity diagnostic,
//! `transform*`. The TS `compileFile` returns `{ result?, diagnostics }`; here
//! the "no result because of an error diagnostic" branch is surfaced as
//! [`DslError::DslValidation`], and a YAML failure as [`DslError::Parse`].
//! Non-fatal warnings ride along on the successful output's `diagnostics`.

use serde_json::Value;

use crate::ast::{AgentAst, ChainAst, PromptAst};
use crate::parser::{build_agent_ast, build_chain_ast, build_prompt_ast, parse_yaml};
use crate::transformer::{
    transform_agent, transform_chain, transform_prompt, AgentConfig, ChainDefinition,
    PromptTemplate,
};
use crate::validator::{
    validate_agent_ast, validate_chain_ast, validate_prompt_ast, Diagnostic, Severity,
};

/// Which top-level DSL document a YAML file describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DslKind {
    Prompt,
    Agent,
    Chain,
}

/// A successfully compiled DSL document plus any non-fatal warnings collected
/// along the way — the `{ result, diagnostics }` of the TS `compileFile`.
#[derive(Clone, Debug, PartialEq)]
pub enum CompileOutput {
    Prompt {
        result: PromptTemplate,
        diagnostics: Vec<Diagnostic>,
    },
    Agent {
        result: AgentConfig,
        diagnostics: Vec<Diagnostic>,
    },
    Chain {
        result: ChainDefinition,
        diagnostics: Vec<Diagnostic>,
    },
}

/// Failure modes of the compile entry points.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DslError {
    /// The input was not well-formed YAML.
    #[error("{0}")]
    Parse(String),
    /// The AST failed DSL-level validation. Carries every error-severity
    /// diagnostic (warnings are not fatal and are not collected here), matching
    /// the TS `compileFile` branch that returns `{ diagnostics }` with no result.
    #[error("DSL validation failed: {}", format_diagnostics(.0))]
    DslValidation(Vec<Diagnostic>),
}

fn format_diagnostics(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| format!("{} ({})", diagnostic.message, diagnostic.loc))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Classify a raw YAML document. Mirror of the TS `detectKind`: an explicit
/// `_kind` wins; otherwise a `template` key means prompt, a `steps` key means
/// chain, and anything else is an agent.
pub fn detect_kind(raw: &Value) -> DslKind {
    if let Some(kind) = raw.get("_kind").and_then(Value::as_str) {
        match kind {
            "prompt" => return DslKind::Prompt,
            "agent" => return DslKind::Agent,
            "chain" => return DslKind::Chain,
            _ => {}
        }
    }
    let map = raw.as_object();
    if map.is_some_and(|m| m.contains_key("template")) {
        DslKind::Prompt
    } else if map.is_some_and(|m| m.contains_key("steps")) {
        DslKind::Chain
    } else {
        DslKind::Agent
    }
}

fn split_errors(diagnostics: Vec<Diagnostic>) -> Result<Vec<Diagnostic>, DslError> {
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        let errors = diagnostics
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        Err(DslError::DslValidation(errors))
    } else {
        Ok(diagnostics)
    }
}

/// Compile DSL content of an unknown kind, dispatching on [`detect_kind`].
/// Mirror of the TS `compileFile`.
pub fn compile_file(content: &str, file: &str) -> Result<CompileOutput, DslError> {
    let raw = parse_yaml(content, file).map_err(DslError::Parse)?;
    match detect_kind(&raw) {
        DslKind::Prompt => {
            let ast = build_prompt_ast(&raw, file);
            let (result, diagnostics) = compile_prompt_ast(ast)?;
            Ok(CompileOutput::Prompt {
                result,
                diagnostics,
            })
        }
        DslKind::Chain => {
            let ast = build_chain_ast(&raw, file);
            let (result, diagnostics) = compile_chain_ast(ast)?;
            Ok(CompileOutput::Chain {
                result,
                diagnostics,
            })
        }
        DslKind::Agent => {
            let ast = build_agent_ast(&raw, file);
            let (result, diagnostics) = compile_agent_ast(ast)?;
            Ok(CompileOutput::Agent {
                result,
                diagnostics,
            })
        }
    }
}

fn compile_prompt_ast(ast: PromptAst) -> Result<(PromptTemplate, Vec<Diagnostic>), DslError> {
    let diagnostics = split_errors(validate_prompt_ast(&ast))?;
    let result = transform_prompt(&ast);
    // The TS prompt branch concatenates the validator diagnostics with the
    // transform's undeclared-variable warnings: `[...diagnostics, ...result.diagnostics]`.
    let mut all = diagnostics;
    all.extend(result.diagnostics.clone());
    Ok((result, all))
}

fn compile_agent_ast(ast: AgentAst) -> Result<(AgentConfig, Vec<Diagnostic>), DslError> {
    let diagnostics = split_errors(validate_agent_ast(&ast))?;
    Ok((transform_agent(&ast), diagnostics))
}

fn compile_chain_ast(ast: ChainAst) -> Result<(ChainDefinition, Vec<Diagnostic>), DslError> {
    let diagnostics = split_errors(validate_chain_ast(&ast))?;
    Ok((transform_chain(&ast), diagnostics))
}

/// Compile prompt YAML directly into a [`PromptTemplate`] (plus its warnings).
/// The file label `"prompt.yaml"` is attached to every diagnostic location.
pub fn compile_prompt_yaml(yaml: &str) -> Result<(PromptTemplate, Vec<Diagnostic>), DslError> {
    let raw = parse_yaml(yaml, "prompt.yaml").map_err(DslError::Parse)?;
    compile_prompt_ast(build_prompt_ast(&raw, "prompt.yaml"))
}

/// Compile agent YAML directly into an [`AgentConfig`] (plus its warnings).
pub fn compile_agent_yaml(yaml: &str) -> Result<(AgentConfig, Vec<Diagnostic>), DslError> {
    let raw = parse_yaml(yaml, "agent.yaml").map_err(DslError::Parse)?;
    compile_agent_ast(build_agent_ast(&raw, "agent.yaml"))
}

/// Compile chain YAML directly into a [`ChainDefinition`] (plus its warnings).
pub fn compile_chain_yaml(yaml: &str) -> Result<(ChainDefinition, Vec<Diagnostic>), DslError> {
    let raw = parse_yaml(yaml, "chain.yaml").map_err(DslError::Parse)?;
    compile_chain_ast(build_chain_ast(&raw, "chain.yaml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validator::DiagnosticCode;
    use serde_json::json;

    #[test]
    fn compiles_prompt_file_through_full_pipeline_to_expected_definition() {
        let output = compile_file(
            "name: Demo\ntemplate: \"Hello {{name}}\"\nvariables: [name]\n",
            "demo.prompt.yaml",
        )
        .expect("prompt should compile");
        let CompileOutput::Prompt {
            result,
            diagnostics,
        } = output
        else {
            panic!("expected a prompt result");
        };
        assert!(diagnostics.iter().all(|d| d.severity != Severity::Error));
        let actual = serde_json::to_value(&result).unwrap();
        let expected = json!({
            "name": "Demo",
            "template": "Hello {{name}}",
            "diagnostics": []
        });
        assert_eq!(actual, expected);
    }

    #[test]
    fn compiles_agent_yaml_to_expected_definition() {
        let yaml = "id: risk-agent\ndescription: Assess risk\nprompt: risk-prompt\ntools:\n  - search\n  - calc\n";
        let (result, diagnostics) = compile_agent_yaml(yaml).expect("agent should compile");
        assert!(diagnostics.is_empty());
        let actual = serde_json::to_value(&result).unwrap();
        let expected = json!({
            "id": "risk-agent",
            "description": "Assess risk",
            "prompt": "risk-prompt",
            "tools": ["search", "calc"]
        });
        assert_eq!(actual, expected);
    }

    #[test]
    fn compiles_chain_yaml_to_expected_definition_with_and_without_input() {
        let yaml = r#"
id: pipeline
steps:
  - agentId: collect
    input:
      source: db
  - agentId: summarize
"#;
        let (result, diagnostics) = compile_chain_yaml(yaml).expect("chain should compile");
        assert!(diagnostics.is_empty());
        let actual = serde_json::to_value(&result).unwrap();
        let expected = json!({
            "id": "pipeline",
            "steps": [
                { "agentId": "collect", "input": { "source": "db" } },
                { "agentId": "summarize" }
            ]
        });
        assert_eq!(actual, expected);
    }

    #[test]
    fn prompt_with_undeclared_variable_compiles_with_a_warning() {
        let (result, diagnostics) =
            compile_prompt_yaml("name: P\ntemplate: \"Hi {{who}}\"\nvariables: []\n")
                .expect("prompt should still compile");
        // The warning rides along on the returned diagnostics, never blocking.
        assert!(diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::UndeclaredTemplateVariable
                && d.severity == Severity::Warning));
        assert_eq!(result.diagnostics, diagnostics);
    }

    #[test]
    fn detect_kind_classifies_each_document() {
        assert_eq!(detect_kind(&json!({ "template": "x" })), DslKind::Prompt);
        assert_eq!(detect_kind(&json!({ "steps": [] })), DslKind::Chain);
        assert_eq!(detect_kind(&json!({ "id": "a" })), DslKind::Agent);
        assert_eq!(
            detect_kind(&json!({ "_kind": "chain", "template": "x" })),
            DslKind::Chain
        );
    }

    #[test]
    fn rejects_malformed_yaml() {
        let result = compile_file("name: Demo\nvariables: [ name", "demo.prompt.yaml");
        let Err(DslError::Parse(message)) = result else {
            panic!("expected a parse error, got {result:?}");
        };
        assert!(message.starts_with("Invalid YAML in demo.prompt.yaml:"));
    }

    #[test]
    fn rejects_a_dsl_validation_failure_for_a_prompt_missing_name_and_template() {
        let result = compile_prompt_yaml("variables: []\n");
        let Err(DslError::DslValidation(diagnostics)) = result else {
            panic!("expected a DSL validation error, got {result:?}");
        };
        assert!(diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::PromptNameRequired));
        assert!(diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::PromptTemplateRequired));
        assert!(diagnostics.iter().all(|d| d.severity == Severity::Error));
    }

    #[test]
    fn rejects_a_chain_with_no_steps() {
        let result = compile_chain_yaml("id: empty\nsteps: []\n");
        let Err(DslError::DslValidation(diagnostics)) = result else {
            panic!("expected a DSL validation error, got {result:?}");
        };
        assert!(diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::ChainStepsRequired));
    }
}
