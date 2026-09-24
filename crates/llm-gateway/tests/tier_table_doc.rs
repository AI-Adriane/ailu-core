//! The documentation's model-tier table is generated from [`ModelPolicy::default`], so the docs
//! can never promise a model the engine does not pick.
//!
//! Regenerate it with: `AILU_UPDATE_DOCS=1 cargo test -p ailu-llm-gateway --test tier_table_doc`

use ailu_llm_gateway::{provider_key_env, provider_slug, LlmProvider, ModelPolicy, ModelTier};

const DOC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs-site/docs/reference/_model-tiers.md"
);

const PROVIDERS: [LlmProvider; 9] = [
    LlmProvider::Anthropic,
    LlmProvider::Openai,
    LlmProvider::Google,
    LlmProvider::Mistral,
    LlmProvider::Openrouter,
    LlmProvider::Minimax,
    LlmProvider::Huggingface,
    LlmProvider::Ollama,
    LlmProvider::Lmstudio,
];

const TIERS: [ModelTier; 4] = [
    ModelTier::Fast,
    ModelTier::Balanced,
    ModelTier::Frontier,
    ModelTier::Creative,
];

fn render() -> String {
    let policy = ModelPolicy::default();
    let mut out = String::from(
        "{/* Generated from crates/llm-gateway ModelPolicy::default() by the tier_table_doc test. Do not edit. */}\n\n\
         | Provider | Key | `fast` | `balanced` | `frontier` | `creative` |\n\
         | --- | --- | --- | --- | --- | --- |\n",
    );
    for provider in PROVIDERS {
        let key = match provider {
            LlmProvider::Ollama => "`AILU_USE_OLLAMA=1`".to_owned(),
            LlmProvider::Lmstudio => "`AILU_USE_LMSTUDIO=1`".to_owned(),
            keyed => provider_key_env(keyed)
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(" or "),
        };
        let models: Vec<String> = TIERS
            .iter()
            .map(|tier| {
                let choice = policy.resolve(*tier, &[provider], Some(provider), None);
                format!("`{}`", choice.model)
            })
            .collect();
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            provider_slug(provider),
            key,
            models.join(" | ")
        ));
    }
    out
}

#[test]
fn documented_tier_table_matches_the_policy() {
    let table = render();
    if std::env::var("AILU_UPDATE_DOCS").as_deref() == Ok("1") {
        std::fs::write(DOC, &table).expect("write the tier table");
    }
    let documented = std::fs::read_to_string(DOC).unwrap_or_default();
    assert_eq!(
        documented, table,
        "docs-site/docs/reference/_model-tiers.md is stale: run \
         AILU_UPDATE_DOCS=1 cargo test -p ailu-llm-gateway --test tier_table_doc"
    );
}
