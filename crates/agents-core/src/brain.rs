//! Governed Brain context injection (ADR 0046 S4).
//!
//! Auto-injects the tenant's GOVERNED entity graph into every agent's seed: the control plane recalls
//! the active brain (tenant-sealed) and seeds it into the [`BRAIN_RECALL_CHANNEL`] before the run; this
//! middleware reads that channel and prepends a "Governed knowledge" block to the seed. Always installed
//! (a strict no-op when the channel is absent), so brain context needs no per-agent overlay — the control
//! plane alone governs WHETHER to seed.
//!
//! Determinism: like memory recall, it only mutates the SEED conversation (no runtime state change), and
//! the seeded set is journaled in the run's entry state — a replay reuses the exact same snapshot (no
//! re-recall), so replay-as-evidence stays sound even as the brain evolves. Read-only: brain WRITES are
//! the governed control-plane path (ADR 0046 S2), never the agent loop.

use std::collections::BTreeMap;

use ailu_llm_gateway::{LlmError, LlmMessage};
use serde_json::Value;

use crate::middleware::{AgentMiddleware, Flow, RunCtx};

/// Reserved input channel (ADR 0046 S4): the active governed-brain entries the CONTROL PLANE recalled
/// (tenant-sealed) before the run, as a JSON array of strings (formatted entities + typed relations).
pub const BRAIN_RECALL_CHANNEL: &str = "__brainRecall";

/// Read `__brainRecall` as a list of formatted brain entries. Tolerant: a missing/malformed channel
/// yields an empty list (fail-open — never sinks the run).
fn read_brain_channel(channels: &BTreeMap<String, Value>) -> Vec<String> {
    channels
        .get(BRAIN_RECALL_CHANNEL)
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Injects the control-plane-recalled governed brain into the agent seed. Always installed; a no-op when
/// `__brainRecall` is absent/empty. Read-only (writes are the governed control-plane path, ADR 0046 S2).
#[derive(Default)]
pub struct BrainMiddleware;

impl BrainMiddleware {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl AgentMiddleware for BrainMiddleware {
    fn name(&self) -> &str {
        "brain"
    }

    async fn before_run(
        &self,
        conversation: &mut Vec<LlmMessage>,
        ctx: &RunCtx<'_>,
    ) -> Result<Flow, LlmError> {
        let entries = read_brain_channel(ctx.channels);
        if entries.is_empty() {
            return Ok(Flow::Continue);
        }
        let body = entries
            .iter()
            .map(|entry| format!("- {entry}"))
            .collect::<Vec<_>>()
            .join("\n");
        let block = format!("Governed knowledge (organisation brain):\n{body}");
        // Prepend into the seed message — one message, no role-alternation break (mirrors memory recall).
        if let Some(seed) = conversation.first_mut() {
            seed.content = format!("{block}\n\n{}", seed.content);
        }
        Ok(Flow::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn ctx<'a>(channels: &'a BTreeMap<String, Value>, approved: &'a HashSet<String>) -> RunCtx<'a> {
        RunCtx {
            iteration: 0,
            approved_tool_names: approved,
            channels,
            run_id: None,
        }
    }

    #[tokio::test]
    async fn injects_the_recalled_brain_into_the_seed() {
        let mw = BrainMiddleware::new();
        let approved = HashSet::new();
        let mut channels = BTreeMap::new();
        channels.insert(
            BRAIN_RECALL_CHANNEL.to_owned(),
            serde_json::json!(["person:jane — CREATED_BY → project:pay", "decision:migrate"]),
        );
        let mut conversation = vec![LlmMessage::text("user", "Who owns payments?")];
        let flow = mw
            .before_run(&mut conversation, &ctx(&channels, &approved))
            .await
            .unwrap();
        assert!(matches!(flow, Flow::Continue));
        let content = &conversation[0].content;
        assert!(content.contains("Governed knowledge"));
        assert!(content.contains("project:pay"));
        // The original seed is preserved after the injected block.
        assert!(content.contains("Who owns payments?"));
    }

    #[tokio::test]
    async fn no_op_when_channel_absent() {
        let mw = BrainMiddleware::new();
        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let mut conversation = vec![LlmMessage::text("user", "plain seed")];
        mw.before_run(&mut conversation, &ctx(&channels, &approved))
            .await
            .unwrap();
        assert_eq!(conversation[0].content, "plain seed");
    }
}
