//! Plan-and-execute — the Rust port of `@ailu/agents-core`'s
//! `PlannerAgent` + `ExecutorAgent` pair (`plan-execute.ts`).
//!
//! The TS ships two cooperating agents:
//! - `PlannerAgent` makes one LLM call (`Plan objective: <objective>`), splits the
//!   completion on newlines, trims, drops empty lines, and turns each surviving line
//!   into a `{ id: "step-<n>", text }` step (1-based). If nothing survives it falls
//!   back to a single step whose text is the objective itself.
//! - `ExecutorAgent` runs each step in order through an injected `executeStep` seam
//!   and accumulates the per-step results into a log.
//!
//! This module mirrors that behaviour as one orchestrator: [`PlanExecuteAgent::run`]
//! does the planner LLM call + identical parsing, then drives an executor seam over
//! the ordered steps. The seam is the Rust analogue of the TS `executeStep`
//! callback — caller-supplied so a step can be an LLM call, a tool call, or anything
//! else, exactly as the TS leaves that choice to its constructor argument.

use std::sync::Arc;

use ailu_llm_gateway::{LlmError, LlmGateway, LlmMessage, LlmProvider, LlmRequest};
use serde::{Deserialize, Serialize};

/// Model used for the planning call. Matches the TS `model: "planner"`.
pub const PLANNER_MODEL: &str = "planner";

/// One ordered plan step. Wire-compatible (camelCase, though both fields are
/// single words) with the TS `PlanStep` (`{ id, text }`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    pub id: String,
    pub text: String,
}

/// What a plan-execute run produces. Mirrors the information the TS pair persists
/// and returns: the parsed `steps` (the planner's reasoning, as JSON in TS) and the
/// ordered execution `logs` (the executor's `reasoning`, newline-joined in TS).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanExecuteResult {
    pub steps: Vec<PlanStep>,
    pub logs: Vec<String>,
}

/// Parse a planner completion into ordered steps, byte-for-byte like the TS:
/// split on `\r?\n`, trim each line, drop empty lines, then 1-based
/// `step-<n>` ids. Empty result falls back to a single step carrying the objective.
fn parse_plan(content: &str, objective: &str) -> Vec<PlanStep> {
    let steps: Vec<PlanStep> = content
        .split('\n')
        .map(|line| line.trim_end_matches('\r').trim())
        .filter(|line| !line.is_empty())
        .enumerate()
        .map(|(index, line)| PlanStep {
            id: format!("step-{}", index + 1),
            text: line.to_owned(),
        })
        .collect();
    if steps.is_empty() {
        vec![PlanStep {
            id: "step-1".to_owned(),
            text: objective.to_owned(),
        }]
    } else {
        steps
    }
}

/// A plan-and-execute agent: plan via one LLM call, then execute each step in order
/// through an injected seam. Cheap to share behind an `Arc`.
pub struct PlanExecuteAgent {
    gateway: Arc<dyn LlmGateway>,
    provider: LlmProvider,
    model: String,
}

impl PlanExecuteAgent {
    /// An agent with the Rust defaults: Anthropic provider, the TS [`PLANNER_MODEL`].
    pub fn new(gateway: Arc<dyn LlmGateway>) -> Self {
        PlanExecuteAgent {
            gateway,
            provider: LlmProvider::Anthropic,
            model: PLANNER_MODEL.to_owned(),
        }
    }

    pub fn with_provider(mut self, provider: LlmProvider) -> Self {
        self.provider = provider;
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Produce the ordered plan for `objective` via a single planner LLM call,
    /// parsed exactly as the TS `PlannerAgent` does.
    pub async fn plan(&self, objective: &str) -> Result<Vec<PlanStep>, LlmError> {
        let response = self
            .gateway
            .complete(LlmRequest {
                provider: self.provider,
                model: self.model.clone(),
                messages: vec![LlmMessage::text(
                    "user",
                    format!("Plan objective: {objective}"),
                )],
                system: None,
                tools: None,
                max_tokens: None,
                temperature: None,
                response_format: None,
                run_id: None,
            })
            .await?;
        Ok(parse_plan(&response.content, objective))
    }

    /// Plan, then execute each step in order through `execute_step`, accumulating
    /// one log entry per step in plan order. `execute_step` is the Rust analogue of
    /// the TS `ExecutorAgent`'s `executeStep` callback — its error short-circuits
    /// the run (the TS `await`s each step, so a rejection would do the same).
    pub async fn run<F, Fut>(
        &self,
        objective: &str,
        mut execute_step: F,
    ) -> Result<PlanExecuteResult, LlmError>
    where
        F: FnMut(&PlanStep) -> Fut,
        Fut: std::future::Future<Output = Result<String, LlmError>>,
    {
        let steps = self.plan(objective).await?;
        let mut logs: Vec<String> = Vec::with_capacity(steps.len());
        for step in &steps {
            logs.push(execute_step(step).await?);
        }
        Ok(PlanExecuteResult { steps, logs })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use ailu_llm_gateway::{DefaultLlmGateway, LlmResponse, LlmUsage, MockAdapter};
    use serde_json::json;

    use super::*;

    fn text(content: &str) -> LlmResponse {
        LlmResponse {
            content: content.to_owned(),
            tool_calls: None,
            stop_reason: Some("end_turn".to_owned()),
            usage: LlmUsage::default(),
            model: "mock".to_owned(),
            provider: LlmProvider::Anthropic,
            content_blocks: None,
        }
    }

    fn gateway_with(responses: Vec<LlmResponse>) -> Arc<DefaultLlmGateway> {
        let mut gateway = DefaultLlmGateway::new();
        gateway.register_adapter(Box::new(MockAdapter::new(
            LlmProvider::Anthropic,
            responses,
        )));
        Arc::new(gateway)
    }

    #[tokio::test]
    async fn plan_is_parsed_into_ordered_trimmed_steps_then_executed_in_order() {
        // Blank lines and surrounding whitespace are dropped/trimmed exactly as TS.
        let agent = PlanExecuteAgent::new(gateway_with(vec![text(
            "  gather data \n\n  draft report \r\n ship it ",
        )]));

        let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&order);
        let result = agent
            .run("write the report", |step| {
                let recorder = Arc::clone(&recorder);
                let step = step.clone();
                async move {
                    recorder.lock().expect("lock").push(step.id.clone());
                    Ok(format!("did:{}", step.text))
                }
            })
            .await
            .unwrap();

        assert_eq!(
            result.steps,
            vec![
                PlanStep {
                    id: "step-1".to_owned(),
                    text: "gather data".to_owned()
                },
                PlanStep {
                    id: "step-2".to_owned(),
                    text: "draft report".to_owned()
                },
                PlanStep {
                    id: "step-3".to_owned(),
                    text: "ship it".to_owned()
                },
            ]
        );
        assert_eq!(
            result.logs,
            vec![
                "did:gather data".to_owned(),
                "did:draft report".to_owned(),
                "did:ship it".to_owned(),
            ]
        );
        // Executed strictly in plan order.
        assert_eq!(
            *order.lock().expect("lock"),
            vec![
                "step-1".to_owned(),
                "step-2".to_owned(),
                "step-3".to_owned()
            ]
        );
    }

    #[tokio::test]
    async fn empty_plan_falls_back_to_a_single_objective_step() {
        let agent = PlanExecuteAgent::new(gateway_with(vec![text("   \n  \r\n  ")]));
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let result = agent
            .run("solve world hunger", |step| {
                let counter = Arc::clone(&counter);
                let text = step.text.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok(text)
                }
            })
            .await
            .unwrap();

        assert_eq!(
            result.steps,
            vec![PlanStep {
                id: "step-1".to_owned(),
                text: "solve world hunger".to_owned()
            }]
        );
        // Exactly one step executed: the objective fallback.
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(result.logs, vec!["solve world hunger".to_owned()]);
    }

    #[test]
    fn result_serializes_camel_case() {
        let result = PlanExecuteResult {
            steps: vec![PlanStep {
                id: "step-1".to_owned(),
                text: "do it".to_owned(),
            }],
            logs: vec!["done".to_owned()],
        };
        let wire = serde_json::to_value(&result).expect("serializes");
        assert_eq!(
            wire,
            json!({ "steps": [{ "id": "step-1", "text": "do it" }], "logs": ["done"] })
        );
    }
}
