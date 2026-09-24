//! `ailu-skills` — the governed skills registry (ADR 0035, phase 12).
//!
//! A **skill** is procedural know-how loaded **progressively**: a SKILL.md = YAML frontmatter
//! (`name` + `description` — a cheap, always-resident index) plus a markdown **body** loaded on
//! demand only when the task matches. Skills are the playbooks a deep agent and its sub-agents
//! pull in (the LangGraph-`deepagents` shape) — but governed: attributable ([`SkillProvenance`]),
//! namespace-scoped, deterministic, version-pinned. A skill is **data, never code**: the body is
//! opaque markdown injected as seed/system text, never executed.
//!
//! Selection is **hybrid** (ADR 0035 D2): `required` skills are explicitly pinned by `name@version`
//! (deterministic), advisory skills are vector-selected over descriptions (top-k, score-desc with
//! an insertion-order tie-break — reusing the `rag-pipeline` cosine). Retrieval is tenant-scoped at
//! the seam (the namespace is supplied by the caller) and version-pinned + tombstone-not-mutate, so
//! a governed run re-selects the same skills on resume.
//!
//! Engine-OSS ships the in-memory default ([`InMemorySkillStore`]); real persistence lives in the
//! control plane behind the same [`SkillStore`] seam.

use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use ailu_rag_pipeline::{cosine_similarity, Embedder, Embedding, MockEmbedder};

/// Attribution stamped on every registered skill (ADR 0035 D4). Empty-ish fields stay off the wire.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillProvenance {
    /// Who registered it — a human principal (registration is approver+, RBAC-enforced upstream).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    /// Ed25519 attestation-chain entry id (governed registration; control-plane).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attestation_id: Option<String>,
    /// ISO-8601 registration time (stamped by the caller — the engine has no clock).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub registered_at: String,
    /// `"asserted" | "verified" | "rejected"` — the governed review lifecycle (control-plane).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// An L3 resource a skill body may reference, resolved **on demand** when the skill is selected
/// (ADR 0035 D3) via the artifact-store / governed-fs seam. `kind` ∈ `artifact | kb | path`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillResource {
    pub kind: String,
    /// The reference (an artifact ref, a KB doc id, or a relative path) — resolved lazily.
    #[serde(rename = "ref")]
    pub reference: String,
}

/// A registered skill. The `description` is the embeddable, always-resident index; `body` is the
/// on-demand markdown. Pinned `name@version`; `embedding`/`embedding_model` anchor deterministic
/// vector selection (re-embed only on a model change).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub name: String,
    /// Semver; the skill is referenced as `name@version`.
    pub version: String,
    pub namespace: String,
    /// The load-bearing, embeddable task-match paragraph (L1 — always available).
    pub description: String,
    /// The on-demand markdown body (L2 — loaded only when selected).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    /// Tools/profiles the body assumes — the governance trigger: a `requires`-bearing skill is
    /// approval-gated when selected (content-scoped by `name@version`, ADR 0024 pattern).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    /// The open-standard `allowed-tools` field (ADR 0065 D1): an allow-list gating what the
    /// skill's OWN instructions may invoke. Distinct from `requires` (a whole-skill capability
    /// gate) — stored and round-tripped through import/export, but **not** enforced by
    /// [`SkillMiddleware`] yet (ADR 0065 explicitly defers that to a follow-up).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    /// Open-standard SPDX license identifier (ADR 0065 D1). Optional, informational only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Open-standard free-form frontmatter metadata map (ADR 0065 D1). Opaque to the engine —
    /// round-tripped through import/export, never interpreted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// L3 bundled resource references (resolved on demand).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<SkillResource>,
    /// Description embedding for vector selection. `None` → never vector-ranked (pin-only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f64>>,
    /// The embedding model id pinned per skill (anti-drift; re-embed only on change).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
    #[serde(default)]
    pub provenance: SkillProvenance,
    /// Tombstoned skills are never selected (versions are immutable — to change, register a new one).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tombstoned: bool,
}

impl Skill {
    /// The `name@version` reference used for pinning + content-scoped approval grants.
    pub fn reference(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("skill store error: {0}")]
    Store(String),
}

/// The DB-free skills seam (ADR 0035 D1): register (per-version, tombstone-not-mutate), hybrid
/// select (pinned + vector), get, tombstone. Object-safe (`Arc<dyn SkillStore>`); the control
/// plane plugs a Postgres impl behind it.
#[async_trait]
pub trait SkillStore: Send + Sync {
    /// Register (or replace, in dev) a skill version, keyed by `(namespace, name, version)`.
    async fn register(&self, skill: Skill) -> Result<(), SkillError>;

    /// Resolve one skill by `(namespace, name, version)` — `None` if absent or tombstoned.
    async fn get(
        &self,
        namespace: &str,
        name: &str,
        version: &str,
    ) -> Result<Option<Skill>, SkillError>;

    /// Hybrid selection (ADR 0035 D2): the `required` skills (each a `"name@version"`, returned in
    /// order, skipping tombstoned/missing) **then** up to `k` advisory skills vector-ranked over
    /// descriptions by cosine to `query` (score-desc, insertion-order tie-break), excluding
    /// tombstoned skills, those already in `required`, and embedding-less skills. `query` `None`
    /// (or `k` = 0) yields the pinned set only.
    async fn select(
        &self,
        namespace: &str,
        required: &[String],
        query: Option<&[f64]>,
        k: usize,
    ) -> Result<Vec<Skill>, SkillError>;

    /// Mark a `(namespace, name, version)` tombstoned (no longer selectable).
    async fn tombstone(&self, namespace: &str, name: &str, version: &str)
        -> Result<(), SkillError>;
}

/// The in-memory default — insertion-ordered (deterministic tie-break), `Mutex`-guarded, zero DB.
#[derive(Default)]
pub struct InMemorySkillStore {
    skills: Mutex<Vec<Skill>>,
}

impl InMemorySkillStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a store pre-populated from a fixed set — the control plane's per-run tenant skills
    /// (ADR 0049 B-3). Sync (no async `register` loop) so the napi bridge can construct a run-scoped,
    /// tenant-isolated store inline while assembling the runtime.
    pub fn from_skills(skills: Vec<Skill>) -> Self {
        Self {
            skills: Mutex::new(skills),
        }
    }
}

/// Split a `"name@version"` reference; the last `@` separates them (versions have no `@`).
fn split_ref(reference: &str) -> Option<(&str, &str)> {
    reference.rsplit_once('@')
}

#[async_trait]
impl SkillStore for InMemorySkillStore {
    async fn register(&self, skill: Skill) -> Result<(), SkillError> {
        let mut skills = self.skills.lock().expect("skills mutex");
        if let Some(existing) = skills.iter_mut().find(|s| {
            s.namespace == skill.namespace && s.name == skill.name && s.version == skill.version
        }) {
            *existing = skill;
        } else {
            skills.push(skill);
        }
        Ok(())
    }

    async fn get(
        &self,
        namespace: &str,
        name: &str,
        version: &str,
    ) -> Result<Option<Skill>, SkillError> {
        let skills = self.skills.lock().expect("skills mutex");
        Ok(skills
            .iter()
            .find(|s| {
                s.namespace == namespace && s.name == name && s.version == version && !s.tombstoned
            })
            .cloned())
    }

    async fn select(
        &self,
        namespace: &str,
        required: &[String],
        query: Option<&[f64]>,
        k: usize,
    ) -> Result<Vec<Skill>, SkillError> {
        let skills = self.skills.lock().expect("skills mutex");
        let live = |s: &Skill| s.namespace == namespace && !s.tombstoned;

        // Pinned, in declared order.
        let mut selected: Vec<Skill> = Vec::new();
        for reference in required {
            if let Some((name, version)) = split_ref(reference) {
                if let Some(skill) = skills
                    .iter()
                    .find(|s| live(s) && s.name == name && s.version == version)
                {
                    selected.push(skill.clone());
                }
            }
        }

        // Advisory: vector top-k over descriptions, excluding pinned + embedding-less.
        if let (Some(query), true) = (query, k > 0) {
            let pinned: Vec<String> = selected.iter().map(Skill::reference).collect();
            let mut scored: Vec<(f64, &Skill)> = skills
                .iter()
                .filter(|s| live(s) && !pinned.contains(&s.reference()))
                .filter_map(|s| {
                    s.embedding
                        .as_ref()
                        .map(|e| (cosine_similarity(query, e), s))
                })
                .collect();
            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            selected.extend(scored.into_iter().take(k).map(|(_, s)| s.clone()));
        }

        Ok(selected)
    }

    async fn tombstone(
        &self,
        namespace: &str,
        name: &str,
        version: &str,
    ) -> Result<(), SkillError> {
        let mut skills = self.skills.lock().expect("skills mutex");
        if let Some(s) = skills
            .iter_mut()
            .find(|s| s.namespace == namespace && s.name == name && s.version == version)
        {
            s.tombstoned = true;
        }
        Ok(())
    }
}

/// Parse a SKILL.md document (YAML-frontmatter + markdown body) into its parts (ADR 0035, frontmatter
/// parity with the open standard added in ADR 0065 D1). A real YAML parser (`serde_yaml`) reads the
/// frontmatter block, so any spec-compliant SKILL.md — block or flow arrays, quoted or bare scalars —
/// round-trips correctly. The body is everything after the closing `---`. `namespace` + embedding +
/// provenance are supplied by the registrar, not the file.
pub struct ParsedSkillMd {
    pub name: String,
    pub version: String,
    pub description: String,
    pub requires: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub license: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub body: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
struct SkillFrontmatter {
    name: String,
    version: String,
    description: String,
    requires: Vec<String>,
    allowed_tools: Vec<String>,
    license: Option<String>,
    metadata: Option<serde_json::Value>,
}

pub fn parse_skill_md(input: &str) -> Result<ParsedSkillMd, SkillError> {
    let trimmed = input.trim_start();
    let rest = trimmed.strip_prefix("---").ok_or_else(|| {
        SkillError::Store("SKILL.md must start with a `---` frontmatter block".to_owned())
    })?;
    let end = rest.find("\n---").ok_or_else(|| {
        SkillError::Store("SKILL.md frontmatter is not closed with `---`".to_owned())
    })?;
    let front = &rest[..end];
    let body = rest[end + 4..].trim_start_matches(['\n', '\r']).to_owned();

    let parsed: SkillFrontmatter = serde_yaml::from_str(front)
        .map_err(|e| SkillError::Store(format!("SKILL.md frontmatter is not valid YAML: {e}")))?;

    if parsed.name.is_empty() || parsed.version.is_empty() {
        return Err(SkillError::Store(
            "SKILL.md frontmatter needs `name` and `version`".to_owned(),
        ));
    }
    Ok(ParsedSkillMd {
        name: parsed.name,
        version: parsed.version,
        description: parsed.description,
        requires: parsed.requires,
        allowed_tools: parsed.allowed_tools,
        license: parsed.license,
        metadata: parsed.metadata,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, version: &str, desc: &str, emb: Option<Vec<f64>>) -> Skill {
        Skill {
            name: name.to_owned(),
            version: version.to_owned(),
            namespace: "skill:t:org".to_owned(),
            description: desc.to_owned(),
            body: format!("# {name}\n…"),
            requires: vec![],
            allowed_tools: vec![],
            license: None,
            metadata: None,
            resources: vec![],
            embedding: emb,
            embedding_model: Some("mock-v1".to_owned()),
            provenance: SkillProvenance {
                principal: Some("approver:jane".to_owned()),
                registered_at: "2026-06-24T00:00:00Z".to_owned(),
                ..Default::default()
            },
            tombstoned: false,
        }
    }

    #[tokio::test]
    async fn select_pins_required_then_vector_advisory() {
        let store = InMemorySkillStore::new();
        store
            .register(skill(
                "refund-policy",
                "1.0.0",
                "how to issue refunds",
                Some(vec![1.0, 0.0]),
            ))
            .await
            .unwrap();
        store
            .register(skill(
                "web-research",
                "1.0.0",
                "search and cite the web",
                Some(vec![0.0, 1.0]),
            ))
            .await
            .unwrap();
        store
            .register(skill(
                "house-style",
                "2.0.0",
                "writing tone and voice",
                Some(vec![0.9, 0.1]),
            ))
            .await
            .unwrap();

        // required pin + advisory top-1 nearest [1,0] (excluding the pinned refund-policy).
        let got = store
            .select(
                "skill:t:org",
                &["refund-policy@1.0.0".to_owned()],
                Some(&[1.0, 0.0]),
                1,
            )
            .await
            .unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].reference(), "refund-policy@1.0.0"); // pinned first
        assert_eq!(got[1].reference(), "house-style@2.0.0"); // nearest advisory (refund-policy excluded)
    }

    #[tokio::test]
    async fn select_is_namespace_scoped_and_skips_tombstoned() {
        let store = InMemorySkillStore::new();
        store
            .register(skill("a", "1.0.0", "x", Some(vec![1.0, 0.0])))
            .await
            .unwrap();
        let mut other = skill("b", "1.0.0", "y", Some(vec![1.0, 0.0]));
        other.namespace = "skill:other:org".to_owned();
        store.register(other).await.unwrap();
        store.tombstone("skill:t:org", "a", "1.0.0").await.unwrap();
        // a tombstoned, b in another namespace → nothing selectable here.
        let got = store
            .select("skill:t:org", &[], Some(&[1.0, 0.0]), 5)
            .await
            .unwrap();
        assert!(got.is_empty());
        assert!(store
            .get("skill:t:org", "a", "1.0.0")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn register_replaces_same_version() {
        let store = InMemorySkillStore::new();
        store
            .register(skill("s", "1.0.0", "old", None))
            .await
            .unwrap();
        let mut updated = skill("s", "1.0.0", "new", None);
        updated.body = "# new body".to_owned();
        store.register(updated).await.unwrap();
        let got = store
            .get("skill:t:org", "s", "1.0.0")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.description, "new");
        assert_eq!(got.body, "# new body");
    }

    #[test]
    fn parse_skill_md_splits_frontmatter_and_body() {
        let md = "---\nname: refund-policy\nversion: 1.2.0\ndescription: how to issue refunds\nrequires: [refund, lookup_order]\n---\n# Refund policy\n\nAlways verify the order first.\n";
        let parsed = parse_skill_md(md).unwrap();
        assert_eq!(parsed.name, "refund-policy");
        assert_eq!(parsed.version, "1.2.0");
        assert_eq!(parsed.description, "how to issue refunds");
        assert_eq!(parsed.requires, vec!["refund", "lookup_order"]);
        assert!(parsed.body.starts_with("# Refund policy"));
    }

    #[test]
    fn parse_skill_md_reads_open_standard_fields_stored_not_enforced() {
        let md = "---\nname: refund-policy\nversion: 1.2.0\ndescription: how to issue refunds\nallowed-tools: [lookup_order, issue_refund]\nlicense: MIT\nmetadata:\n  author: finance-team\n  tags: [refunds, policy]\n---\nbody\n";
        let parsed = parse_skill_md(md).unwrap();
        assert_eq!(parsed.allowed_tools, vec!["lookup_order", "issue_refund"]);
        assert_eq!(parsed.license, Some("MIT".to_owned()));
        assert_eq!(
            parsed.metadata.unwrap()["author"],
            serde_json::json!("finance-team")
        );
    }

    #[test]
    fn parse_skill_md_block_array_requires_still_parses() {
        let md =
            "---\nname: s\nversion: 1.0.0\ndescription: d\nrequires:\n  - a\n  - b\n---\nbody\n";
        let parsed = parse_skill_md(md).unwrap();
        assert_eq!(parsed.requires, vec!["a", "b"]);
    }

    #[test]
    fn parse_skill_md_rejects_missing_frontmatter() {
        assert!(parse_skill_md("no frontmatter here").is_err());
        assert!(parse_skill_md("---\ndescription: x\n---\nbody").is_err()); // no name/version
    }
}
