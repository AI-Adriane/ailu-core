//! Supervisor — the Rust port of `@ailu/agents-core`'s `SupervisorAgent`
//! (`supervisor.ts`).
//!
//! The TS supervisor routes among named worker agents within a round budget. Each
//! round it:
//!
//! 1. checks `__supervisorRounds`; if `>= maxRounds`, returns `FINISH`;
//! 2. builds a candidate list — one `- <id>: <description>` line per worker
//!    (description falls back to `"No description"`);
//! 3. asks the router LLM with the prompt
//!    `Objective: <objective>\nAgents:\n<candidates>\nReply with AGENT:<id> or FINISH`;
//! 4. parses the (trimmed) response: a leading `FINISH` ends routing; otherwise the
//!    `AGENT:` prefix is stripped and the remainder is matched against the known
//!    worker ids. An unknown id (or one without a node mapping) also yields `FINISH`.
//!    On a match it routes to that worker and increments the round counter.
//!
//! In the TS each routing decision is one node step; the runtime invokes the chosen
//! worker node and re-enters the supervisor. This module folds that into one driver:
//! [`SupervisorAgent::run`] loops decide → invoke worker until `FINISH` or the budget,
//! invoking each chosen worker through a caller-supplied seam keyed by worker id, and
//! reports which workers ran (in order) plus the final answer.

use std::sync::Arc;

use ailu_llm_gateway::{LlmError, LlmGateway, LlmMessage, LlmProvider, LlmRequest};
use serde::{Deserialize, Serialize};

/// Model used for the routing call. Matches the TS `model: "supervisor-router"`.
pub const SUPERVISOR_MODEL: &str = "supervisor-router";

/// Description fallback when a worker has none. Matches the TS `"No description"`.
pub const NO_DESCRIPTION: &str = "No description";

/// A worker the supervisor can route to: its id and a human description used to
/// build the router prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worker {
    pub id: String,
    pub description: String,
}

impl Worker {
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        Worker {
            id: id.into(),
            description: description.into(),
        }
    }
}

/// One routing decision, mirroring the TS `Command | "FINISH"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Routing {
    /// Route to this worker id (and, in TS, increment `__supervisorRounds`).
    Agent(String),
    /// End routing — budget spent, explicit `FINISH`, or an unroutable choice.
    Finish,
}

/// What a supervised run produces: the ordered list of worker ids that ran and the
/// final answer (the last worker's output, or empty if none ran).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupervisorResult {
    pub workers_run: Vec<String>,
    pub final_answer: String,
}

/// A supervisor agent: route among named workers within a round budget. Cheap to
/// share behind an `Arc`.
pub struct SupervisorAgent {
    gateway: Arc<dyn LlmGateway>,
    provider: LlmProvider,
    model: String,
    workers: Vec<Worker>,
    max_rounds: usize,
}

impl SupervisorAgent {
    /// A supervisor over `workers` with the given round budget. Defaults to the
    /// Anthropic provider and the TS [`SUPERVISOR_MODEL`].
    pub fn new(gateway: Arc<dyn LlmGateway>, workers: Vec<Worker>, max_rounds: usize) -> Self {
        SupervisorAgent {
            gateway,
            provider: LlmProvider::Anthropic,
            model: SUPERVISOR_MODEL.to_owned(),
            workers,
            max_rounds,
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

    /// Decide the next routing for `objective` given the rounds already taken. One
    /// LLM call; parsing is byte-for-byte the TS rule.
    pub async fn next_routing(&self, objective: &str, rounds: usize) -> Result<Routing, LlmError> {
        if rounds >= self.max_rounds {
            return Ok(Routing::Finish);
        }
        let candidates = self
            .workers
            .iter()
            .map(|worker| {
                let description = if worker.description.is_empty() {
                    NO_DESCRIPTION
                } else {
                    &worker.description
                };
                format!("- {}: {}", worker.id, description)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let response = self
            .gateway
            .complete(LlmRequest {
                provider: self.provider,
                model: self.model.clone(),
                messages: vec![LlmMessage::text(
                    "user",
                    format!("Objective: {objective}\nAgents:\n{candidates}\nReply with AGENT:<id> or FINISH"),
                )],
                system: None,
                tools: None,
                max_tokens: None,
                temperature: None,
                response_format: None,
                run_id: None,
            })
            .await?;

        let response = response.content.trim();
        if response.starts_with("FINISH") {
            return Ok(Routing::Finish);
        }
        // TS: `response.replace("AGENT:", "").trim()` — replaces the first occurrence.
        let selected = match response.split_once("AGENT:") {
            Some((before, after)) => format!("{before}{after}"),
            None => response.to_owned(),
        };
        let selected = selected.trim();
        match self.workers.iter().find(|worker| worker.id == selected) {
            Some(worker) => Ok(Routing::Agent(worker.id.clone())),
            // Unknown id → FINISH, exactly as TS (no matching agent / node).
            None => Ok(Routing::Finish),
        }
    }

    /// Route → invoke the chosen worker → repeat, until `FINISH` or the round budget.
    /// `invoke_worker` is the seam the TS runtime fills by executing the worker's
    /// node; it receives the chosen worker id and the objective and returns that
    /// worker's answer. The last answer produced becomes the run's `final_answer`.
    pub async fn run<F, Fut>(
        &self,
        objective: &str,
        mut invoke_worker: F,
    ) -> Result<SupervisorResult, LlmError>
    where
        F: FnMut(&str, &str) -> Fut,
        Fut: std::future::Future<Output = Result<String, LlmError>>,
    {
        let mut workers_run: Vec<String> = Vec::new();
        let mut final_answer = String::new();
        let mut rounds = 0usize;

        loop {
            match self.next_routing(objective, rounds).await? {
                Routing::Finish => break,
                Routing::Agent(worker_id) => {
                    final_answer = invoke_worker(&worker_id, objective).await?;
                    workers_run.push(worker_id);
                    rounds += 1;
                }
            }
        }

        Ok(SupervisorResult {
            workers_run,
            final_answer,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use ailu_llm_gateway::{DefaultLlmGateway, LlmResponse, LlmUsage, MockAdapter};

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

    fn workers() -> Vec<Worker> {
        vec![
            Worker::new("researcher", "Finds facts."),
            Worker::new("writer", "Drafts prose."),
        ]
    }

    #[tokio::test]
    async fn routes_to_scripted_workers_in_order_then_finishes() {
        let agent = SupervisorAgent::new(
            gateway_with(vec![
                text("AGENT:researcher"),
                text("AGENT:writer"),
                text("FINISH"),
            ]),
            workers(),
            5,
        );

        let invoked: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&invoked);
        let result = agent
            .run("write a report", |worker_id, _objective| {
                let recorder = Arc::clone(&recorder);
                let worker_id = worker_id.to_owned();
                async move {
                    recorder.lock().expect("lock").push(worker_id.clone());
                    Ok(format!("{worker_id} did its part"))
                }
            })
            .await
            .unwrap();

        assert_eq!(
            result.workers_run,
            vec!["researcher".to_owned(), "writer".to_owned()]
        );
        assert_eq!(result.final_answer, "writer did its part");
        assert_eq!(
            *invoked.lock().expect("lock"),
            vec!["researcher".to_owned(), "writer".to_owned()]
        );
    }

    #[tokio::test]
    async fn round_budget_stops_routing() {
        // Router always wants the writer, but the budget caps it at one round.
        let agent = SupervisorAgent::new(gateway_with(vec![text("AGENT:writer")]), workers(), 1);
        let result = agent
            .run("endless", |worker_id, _| {
                let worker_id = worker_id.to_owned();
                async move { Ok(format!("{worker_id} ran")) }
            })
            .await
            .unwrap();
        assert_eq!(result.workers_run, vec!["writer".to_owned()]);
        assert_eq!(result.final_answer, "writer ran");
    }

    #[tokio::test]
    async fn unknown_agent_choice_finishes_without_running_a_worker() {
        let agent = SupervisorAgent::new(gateway_with(vec![text("AGENT:ghost")]), workers(), 5);
        let result = agent
            .run("whatever", |worker_id, _| {
                let worker_id = worker_id.to_owned();
                async move { Ok(worker_id) }
            })
            .await
            .unwrap();
        assert!(result.workers_run.is_empty());
        assert_eq!(result.final_answer, "");
    }

    #[tokio::test]
    async fn worker_without_description_uses_the_fallback_in_the_prompt() {
        // Smoke: an empty description still routes correctly (prompt uses fallback).
        let agent = SupervisorAgent::new(
            gateway_with(vec![text("AGENT:solo"), text("FINISH")]),
            vec![Worker::new("solo", "")],
            5,
        );
        let result = agent
            .run("task", |id, _| {
                let id = id.to_owned();
                async move { Ok(format!("{id}!")) }
            })
            .await
            .unwrap();
        assert_eq!(result.workers_run, vec!["solo".to_owned()]);
        assert_eq!(result.final_answer, "solo!");
    }
}
