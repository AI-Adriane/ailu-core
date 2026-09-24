//! Governed skills middleware (ADR 0035 phase 12) — progressive disclosure for deep agents.
//!
//! Wires the [`ailu_skills`] seam into the agent loop via the proven `before_run` hook
//! (mirroring [`crate::memory::MemoryMiddleware`]): embed the seed, hybrid-**select** skills from
//! the run owner's sealed namespace (explicit `required` pins + advisory vector top-k over
//! descriptions), load the selected **L2 bodies**, resolve their **L3 resources** on demand, and
//! **prepend** them to the seed conversation as one message — a seed-only mutation, no runtime
//! state change, no new checkpoint path.
//!
//! Because `mapAgents`/`taskNode` sub-agents are built through the **same** `build_react_agent`
//! path as the main agent, each agent in a deep-agent composition gets its own `SkillMiddleware`
//! with its own pins + advisory scope — the LangGraph-`deepagents` shape, governed by construction.
//!
//! `namespace` + the approval set are **sealed by the bridge** (never from user data), so selection
//! is tenant-scoped by construction. Fail-open: a select/load/resolve error never sinks a good run.
//!
//! Governance of capability-granting skills (ADR 0035 D4): a selected skill carrying `requires`
//! must be **granted** (its `skill:{name}@{version}` key present in the run's approval set) before
//! its body enters the context. An ungranted capability-granting skill is **omitted** (never
//! injected — the "ungoverned injection is unrepresentable" invariant holds) and recorded as
//! `gated` in the selected set. (The resumable human-gate *suspend* on select — the tool-gate UX —
//! is a follow-up: `before_run` has no approval channel yet; omission is the fail-safe today.)

use std::sync::{Arc, Mutex};

use ailu_llm_gateway::{LlmError, LlmMessage};
use ailu_skills::{Embedder, Skill, SkillResource, SkillStore};
use async_trait::async_trait;

use crate::middleware::{AgentMiddleware, Flow, RunCtx};

/// Per-resource and per-body injection cap (chars). Bounds context bloat (ADR 0035 risk 2).
const MAX_BODY_CHARS: usize = 8_000;
const MAX_RESOURCE_CHARS: usize = 4_000;

/// On-demand L3 resource resolution (ADR 0035 D3), kept behind a seam so the engine middleware
/// stays free of the artifact-store / fs deps; the control plane plugs a real resolver. Returns
/// the resolved text, or `None` (skipped, fail-open) on any miss/denial.
#[async_trait]
pub trait SkillResourceResolver: Send + Sync {
    async fn resolve(&self, resource: &SkillResource) -> Option<String>;
}

/// What the middleware selected for one run — recorded for AI-Act provenance ("which know-how
/// informed this action"). The bridge / control plane reads [`SkillMiddleware::selected`] after
/// the run and writes it to run provenance.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectedSkill {
    pub reference: String,
    pub embedding_model: Option<String>,
    /// `true` when the skill was selected but **withheld** because its capability grant was missing.
    pub gated: bool,
}

/// Select-and-inject skills over the [`SkillStore`] seam.
pub struct SkillMiddleware {
    store: Arc<dyn SkillStore>,
    embedder: Arc<dyn Embedder>,
    /// Tenant-scoped skill namespace, sealed at construction (the bridge supplies it).
    namespace: String,
    /// Explicit `name@version` pins (the governed / must-apply playbooks).
    required: Vec<String>,
    /// Advisory vector top-k cap (0 → pins only).
    advisory_k: usize,
    /// Optional L3 resolver (control-plane); `None` → resources are listed by ref, not fetched.
    resolver: Option<Arc<dyn SkillResourceResolver>>,
    /// The selected set captured in `before_run`, exposed for provenance recording.
    selected: Mutex<Vec<SelectedSkill>>,
}

impl SkillMiddleware {
    pub fn new(
        store: Arc<dyn SkillStore>,
        embedder: Arc<dyn Embedder>,
        namespace: impl Into<String>,
        required: Vec<String>,
        advisory_k: usize,
        resolver: Option<Arc<dyn SkillResourceResolver>>,
    ) -> Self {
        Self {
            store,
            embedder,
            namespace: namespace.into(),
            required,
            advisory_k,
            resolver,
            selected: Mutex::new(Vec::new()),
        }
    }

    /// The skills selected (and whether each was gated) in the last `before_run` — for the bridge
    /// to record to run provenance.
    pub fn selected(&self) -> Vec<SelectedSkill> {
        self.selected.lock().expect("skills selected mutex").clone()
    }

    /// The content-scoped grant key for a capability-granting skill (ADR 0024 pattern).
    fn grant_key(skill: &Skill) -> String {
        format!("skill:{}", skill.reference())
    }

    async fn embed_one(&self, text: &str) -> Option<Vec<f64>> {
        self.embedder
            .embed(&[text.to_owned()])
            .await
            .ok()
            .and_then(|vectors| vectors.into_iter().next())
    }

    /// Render one selected skill into an injectable block (L2 body + resolved L3 resources).
    async fn render(&self, skill: &Skill) -> String {
        let mut block = format!("## Skill: {}\n", skill.reference());
        let body: String = skill.body.chars().take(MAX_BODY_CHARS).collect();
        block.push_str(&body);
        for resource in &skill.resources {
            match &self.resolver {
                Some(resolver) => {
                    if let Some(text) = resolver.resolve(resource).await {
                        let text: String = text.chars().take(MAX_RESOURCE_CHARS).collect();
                        block.push_str(&format!(
                            "\n\n### Resource ({}): {}\n{text}",
                            resource.kind, resource.reference
                        ));
                    }
                }
                None => block.push_str(&format!(
                    "\n\n### Resource ({}): {}",
                    resource.kind, resource.reference
                )),
            }
        }
        block
    }
}

#[async_trait]
impl AgentMiddleware for SkillMiddleware {
    fn name(&self) -> &str {
        "skills"
    }

    async fn before_run(
        &self,
        conversation: &mut Vec<LlmMessage>,
        ctx: &RunCtx<'_>,
    ) -> Result<Flow, LlmError> {
        // Embed the seed for advisory selection (fail-open — a recall failure never sinks the run).
        let query = match conversation.first().map(|m| m.content.clone()) {
            Some(seed_text) => self.embed_one(&seed_text).await,
            None => None,
        };
        let skills = match self
            .store
            .select(
                &self.namespace,
                &self.required,
                query.as_deref(),
                self.advisory_k,
            )
            .await
        {
            Ok(skills) if !skills.is_empty() => skills,
            _ => {
                self.selected.lock().expect("skills selected mutex").clear();
                return Ok(Flow::Continue);
            }
        };

        // Partition: capability-granting skills must be granted; ungranted ones are withheld.
        let mut record: Vec<SelectedSkill> = Vec::new();
        let mut blocks: Vec<String> = Vec::new();
        for skill in &skills {
            let gated = !skill.requires.is_empty()
                && !ctx.approved_tool_names.contains(&Self::grant_key(skill));
            record.push(SelectedSkill {
                reference: skill.reference(),
                embedding_model: skill.embedding_model.clone(),
                gated,
            });
            if !gated {
                blocks.push(self.render(skill).await);
            }
        }
        *self.selected.lock().expect("skills selected mutex") = record;

        if !blocks.is_empty() {
            let block = format!(
                "Applicable skills (loaded from '{}'):\n\n{}",
                self.namespace,
                blocks.join("\n\n")
            );
            if let Some(seed) = conversation.first_mut() {
                seed.content = format!("{block}\n\n{}", seed.content);
            }
        }
        Ok(Flow::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ailu_skills::{InMemorySkillStore, MockEmbedder, SkillProvenance};
    use std::collections::{BTreeMap, HashSet};

    fn ctx<'a>(
        approved: &'a HashSet<String>,
        channels: &'a BTreeMap<String, serde_json::Value>,
    ) -> RunCtx<'a> {
        RunCtx {
            iteration: 0,
            approved_tool_names: approved,
            channels,
            run_id: None,
        }
    }

    fn skill(name: &str, body: &str, requires: Vec<String>, emb: Vec<f64>) -> Skill {
        Skill {
            name: name.to_owned(),
            version: "1.0.0".to_owned(),
            namespace: "skill:t:org".to_owned(),
            description: format!("{name} description"),
            body: body.to_owned(),
            requires,
            allowed_tools: vec![],
            license: None,
            metadata: None,
            resources: vec![],
            embedding: Some(emb),
            embedding_model: Some("mock-v1".to_owned()),
            provenance: SkillProvenance::default(),
            tombstoned: false,
        }
    }

    async fn store_with(skills: Vec<Skill>) -> Arc<InMemorySkillStore> {
        let store = Arc::new(InMemorySkillStore::new());
        for s in skills {
            store.register(s).await.unwrap();
        }
        store
    }

    #[tokio::test]
    async fn injects_pinned_body_into_the_seed() {
        let store = store_with(vec![skill(
            "house-style",
            "Write in plain language.",
            vec![],
            vec![1.0, 0.0],
        )])
        .await;
        let mw = SkillMiddleware::new(
            store,
            Arc::new(MockEmbedder),
            "skill:t:org",
            vec!["house-style@1.0.0".to_owned()],
            0,
            None,
        );
        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let mut conversation = vec![LlmMessage::text("user", "Input: draft a reply")];
        mw.before_run(&mut conversation, &ctx(&approved, &channels))
            .await
            .unwrap();
        assert!(conversation[0].content.contains("Applicable skills"));
        assert!(conversation[0].content.contains("house-style@1.0.0"));
        assert!(conversation[0].content.contains("Write in plain language."));
        assert!(conversation[0].content.contains("Input: draft a reply"));
        assert_eq!(
            mw.selected(),
            vec![SelectedSkill {
                reference: "house-style@1.0.0".to_owned(),
                embedding_model: Some("mock-v1".to_owned()),
                gated: false,
            }]
        );
    }

    #[tokio::test]
    async fn capability_granting_skill_is_withheld_until_granted() {
        let store = store_with(vec![skill(
            "deploy-runbook",
            "Run the deploy.",
            vec!["deploy".to_owned()],
            vec![1.0, 0.0],
        )])
        .await;
        let mw = SkillMiddleware::new(
            store,
            Arc::new(MockEmbedder),
            "skill:t:org",
            vec!["deploy-runbook@1.0.0".to_owned()],
            0,
            None,
        );
        let channels = BTreeMap::new();

        // Ungranted → withheld (never injected), recorded gated.
        let ungranted = HashSet::new();
        let mut conversation = vec![LlmMessage::text("user", "Input: ship it")];
        mw.before_run(&mut conversation, &ctx(&ungranted, &channels))
            .await
            .unwrap();
        assert!(!conversation[0].content.contains("Run the deploy."));
        assert!(!conversation[0].content.contains("Applicable skills"));
        assert!(mw.selected()[0].gated);

        // Granted → injected.
        let mut granted = HashSet::new();
        granted.insert("skill:deploy-runbook@1.0.0".to_owned());
        let mut conversation = vec![LlmMessage::text("user", "Input: ship it")];
        mw.before_run(&mut conversation, &ctx(&granted, &channels))
            .await
            .unwrap();
        assert!(conversation[0].content.contains("Run the deploy."));
        assert!(!mw.selected()[0].gated);
    }

    #[tokio::test]
    async fn empty_selection_leaves_the_seed_unchanged() {
        let store = store_with(vec![]).await;
        let mw = SkillMiddleware::new(
            store,
            Arc::new(MockEmbedder),
            "skill:t:org",
            vec![],
            5,
            None,
        );
        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let mut conversation = vec![LlmMessage::text("user", "Input: hi")];
        mw.before_run(&mut conversation, &ctx(&approved, &channels))
            .await
            .unwrap();
        assert_eq!(conversation[0].content, "Input: hi");
        assert!(mw.selected().is_empty());
    }

    struct StaticResolver;
    #[async_trait]
    impl SkillResourceResolver for StaticResolver {
        async fn resolve(&self, resource: &SkillResource) -> Option<String> {
            Some(format!("resolved:{}", resource.reference))
        }
    }

    #[tokio::test]
    async fn resolves_l3_resources_on_demand() {
        let mut s = skill("with-res", "Body.", vec![], vec![1.0, 0.0]);
        s.resources = vec![SkillResource {
            kind: "artifact".to_owned(),
            reference: "art-1".to_owned(),
        }];
        let store = store_with(vec![s]).await;
        let mw = SkillMiddleware::new(
            store,
            Arc::new(MockEmbedder),
            "skill:t:org",
            vec!["with-res@1.0.0".to_owned()],
            0,
            Some(Arc::new(StaticResolver)),
        );
        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let mut conversation = vec![LlmMessage::text("user", "Input: go")];
        mw.before_run(&mut conversation, &ctx(&approved, &channels))
            .await
            .unwrap();
        assert!(conversation[0].content.contains("resolved:art-1"));
    }
}
