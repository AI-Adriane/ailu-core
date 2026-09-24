//! DSL-level validation of the prompt/agent/chain ASTs — Rust mirror of
//! `packages/lang-ailu/src/validator/{types,validate-prompt-ast,validate-agent-ast,validate-chain-ast}.ts`.

use serde::{Deserialize, Serialize};

use crate::ast::{AgentAst, ChainAst, Loc, PromptAst};

/// Stable machine-readable diagnostic codes — the TS validator's and template
/// engine's string codes. Serializes as the exact SCREAMING_SNAKE_CASE strings
/// the TS pipeline emits in the `code` field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DiagnosticCode {
    PromptNameRequired,
    PromptTemplateRequired,
    AgentIdRequired,
    AgentPromptRequired,
    ChainIdRequired,
    ChainStepsRequired,
    ChainStepAgentRequired,
    /// Emitted at render time when a `{{ var }}` token has no provided value.
    UnresolvedVariable,
    /// Emitted at transform time when a template uses a token not declared in
    /// the prompt's `variables` list.
    UndeclaredTemplateVariable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    Error,
    Warning,
}

/// One validation finding — mirrors the TS `Diagnostic` shape
/// (`{ code, message, loc, severity }`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub message: String,
    pub loc: Loc,
    pub severity: Severity,
}

/// Validate a prompt AST. Mirror of the TS `validatePromptAST`.
pub fn validate_prompt_ast(ast: &PromptAst) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if ast.name.trim().is_empty() {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::PromptNameRequired,
            message: "Prompt name is required.".to_owned(),
            loc: ast.loc.clone(),
            severity: Severity::Error,
        });
    }
    if ast.template.trim().is_empty() {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::PromptTemplateRequired,
            message: "Prompt template is required.".to_owned(),
            loc: ast.loc.clone(),
            severity: Severity::Error,
        });
    }
    diagnostics
}

/// Validate an agent AST. Mirror of the TS `validateAgentAST`.
pub fn validate_agent_ast(ast: &AgentAst) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if ast.id.trim().is_empty() {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::AgentIdRequired,
            message: "Agent id is required.".to_owned(),
            loc: ast.loc.clone(),
            severity: Severity::Error,
        });
    }
    if ast.prompt.trim().is_empty() {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::AgentPromptRequired,
            message: "Agent prompt reference is required.".to_owned(),
            loc: ast.loc.clone(),
            severity: Severity::Error,
        });
    }
    diagnostics
}

/// Validate a chain AST. Mirror of the TS `validateChainAST`: emits the missing
/// id and empty-steps errors, then one error per step whose `agentId` is blank.
pub fn validate_chain_ast(ast: &ChainAst) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if ast.id.trim().is_empty() {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::ChainIdRequired,
            message: "Chain id is required.".to_owned(),
            loc: ast.loc.clone(),
            severity: Severity::Error,
        });
    }
    if ast.steps.is_empty() {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::ChainStepsRequired,
            message: "Chain must contain at least one step.".to_owned(),
            loc: ast.loc.clone(),
            severity: Severity::Error,
        });
    }
    for step in &ast.steps {
        if step.agent_id.trim().is_empty() {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::ChainStepAgentRequired,
                message: "Each chain step must reference an agentId.".to_owned(),
                loc: step.loc.clone(),
                severity: Severity::Error,
            });
        }
    }
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{build_agent_ast, build_chain_ast, build_prompt_ast};
    use serde_json::json;

    #[test]
    fn returns_diagnostics_with_location_for_invalid_agent_ast() {
        let ast = build_agent_ast(
            &json!({ "description": "missing id and prompt" }),
            "agent.yaml",
        );
        let diagnostics = validate_agent_ast(&ast);
        assert!(!diagnostics.is_empty());
        assert_eq!(diagnostics[0].loc.file, "agent.yaml");
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[0].code, DiagnosticCode::AgentIdRequired);
        assert!(diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::AgentPromptRequired));
    }

    #[test]
    fn flags_missing_prompt_name_and_template() {
        let ast = build_prompt_ast(&json!({}), "p.yaml");
        let diagnostics = validate_prompt_ast(&ast);
        let codes: Vec<_> = diagnostics.iter().map(|d| d.code).collect();
        assert!(codes.contains(&DiagnosticCode::PromptNameRequired));
        assert!(codes.contains(&DiagnosticCode::PromptTemplateRequired));
    }

    #[test]
    fn flags_empty_chain_and_blank_step_agents() {
        let empty = build_chain_ast(&json!({}), "c.yaml");
        let diagnostics = validate_chain_ast(&empty);
        let codes: Vec<_> = diagnostics.iter().map(|d| d.code).collect();
        assert!(codes.contains(&DiagnosticCode::ChainIdRequired));
        assert!(codes.contains(&DiagnosticCode::ChainStepsRequired));

        let blank_step = build_chain_ast(
            &json!({ "id": "c", "steps": [{ "agentId": "" }] }),
            "c.yaml",
        );
        let diagnostics = validate_chain_ast(&blank_step);
        assert!(diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::ChainStepAgentRequired));
    }
}
