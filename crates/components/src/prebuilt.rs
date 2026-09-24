//! Prebuilt micro-task agent definitions.
//!
//! A [`PrebuiltAgent`] is a *definition*, not a runtime: it carries the system
//! prompt, the capability [`ModelTier`] it wants, the tool names it may call,
//! whether it must suspend for human approval before acting, and the channel its
//! result lands in. The concrete model is resolved later (Phase C) by the
//! `ModelPolicy` in `ailu-llm-gateway`, given the providers actually
//! available — so a definition stays portable across "I only have Mistral" and
//! "only Anthropic" deployments.
//!
//! All field names serialise as `camelCase` to stay wire-compatible with the TS
//! `@ailu/*` types.

use ailu_llm_gateway::ModelTier;
use serde::{Deserialize, Serialize};

/// A prebuilt, tier-tagged micro-agent definition.
///
/// This is a pure data record: it describes *what* the agent is, leaving model
/// resolution and execution to the runtime layers. `tier` is the abstract
/// capability the agent needs; `tool_names` are the tools it is allowed to call
/// (looked up in a tool registry at run time); `suspend_for_approval` gates a
/// human approval before any tool runs; `output_channel` is where its result is
/// written into graph state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrebuiltAgent {
    /// Stable lookup name (the key for [`prebuilt`]).
    pub name: String,
    /// One-line human description of the agent's job.
    pub description: String,
    /// The capability tier the agent needs; mapped to a concrete model later.
    pub tier: ModelTier,
    /// The system prompt that defines the agent's behaviour.
    pub system_prompt: String,
    /// Names of the tools the agent may call (resolved against a tool registry).
    pub tool_names: Vec<String>,
    /// When `true`, the run suspends for human approval before any tool runs.
    pub suspend_for_approval: bool,
    /// The graph-state channel the agent writes its result into.
    pub output_channel: String,
}

impl PrebuiltAgent {
    /// Convenience constructor keeping the field order readable at the call site.
    fn new(
        name: &str,
        description: &str,
        tier: ModelTier,
        system_prompt: &str,
        tool_names: &[&str],
        suspend_for_approval: bool,
        output_channel: &str,
    ) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            tier,
            system_prompt: system_prompt.to_string(),
            tool_names: tool_names.iter().map(|t| (*t).to_string()).collect(),
            suspend_for_approval,
            output_channel: output_channel.to_string(),
        }
    }
}

/// The summarizer: condense input text into a short, faithful summary.
fn summarizer() -> PrebuiltAgent {
    PrebuiltAgent::new(
        "summarizer",
        "Condenses input text into a short, faithful summary.",
        ModelTier::Fast,
        "You are a precise summarizer. Read the user's text and produce a concise \
         summary that preserves the key facts and intent. Do not add information \
         that is not present. Respond with the summary only, no preamble.",
        &[],
        false,
        "summary",
    )
}

/// The classifier: assign the input to exactly one label from a fixed set.
fn classifier() -> PrebuiltAgent {
    PrebuiltAgent::new(
        "classifier",
        "Assigns the input to exactly one label from a fixed set.",
        ModelTier::Fast,
        "You are a text classifier. Classify the user's input into exactly one of \
         the following labels: \"positive\", \"negative\", \"neutral\", \"question\", \
         \"spam\". Respond with the single label only, lowercase, no punctuation or \
         explanation.",
        &[],
        false,
        "label",
    )
}

/// The extractor: pull structured fields out of unstructured text as JSON.
fn extractor() -> PrebuiltAgent {
    PrebuiltAgent::new(
        "extractor",
        "Extracts structured fields from unstructured text as JSON.",
        ModelTier::Fast,
        "You extract structured data from text. Return a single JSON object that \
         matches this schema: { \"name\": string | null, \"email\": string | null, \
         \"organization\": string | null, \"intent\": string | null }. Use null for \
         any field not present in the text. Respond with the JSON object only, no \
         code fences and no commentary.",
        &[],
        false,
        "extracted",
    )
}

/// The SQL generator: turn a natural-language request into a SQL query.
fn sql_generator() -> PrebuiltAgent {
    PrebuiltAgent::new(
        "sqlGenerator",
        "Generates a SQL query from a natural-language request and schema.",
        ModelTier::Balanced,
        "You are a SQL generator. Given a natural-language request and an optional \
         schema description, produce a single, syntactically valid SQL query that \
         answers the request. Prefer standard SQL. Do not modify data unless the \
         request explicitly asks for it. Respond with the SQL query only, no code \
         fences and no explanation.",
        &[],
        false,
        "sql",
    )
}

/// The RAG answerer: compose the retriever + reranker components with an agent
/// step to answer grounded in retrieved documents.
fn rag_answerer() -> PrebuiltAgent {
    PrebuiltAgent::new(
        "ragAnswerer",
        "Answers a question grounded in retrieved documents. Composed as a graph: \
         the `retriever` component fetches candidate documents, the `reranker` \
         component reorders them, and this agent step writes a grounded answer \
         citing the supplied context.",
        ModelTier::Balanced,
        "You are a retrieval-augmented answerer. You are given a question and a set \
         of retrieved context passages. Answer the question using only the \
         provided context. If the context does not contain the answer, say you do \
         not know. Cite the passage ids you relied on. Respond with the answer \
         only.",
        &[],
        false,
        "answer",
    )
}

/// The refund approver: decides on a refund and routes it through a human
/// approval gate before calling the `refund` tool.
fn refund_approver() -> PrebuiltAgent {
    PrebuiltAgent::new(
        "refundApprover",
        "Decides whether to issue a refund and routes the action through a human \
         approval gate before calling the `refund` tool.",
        ModelTier::Balanced,
        "You are a refund assistant. Review the customer's request and the order \
         details, decide whether a refund is warranted, and if so prepare a call \
         to the `refund` tool with the order id and amount. You may not issue a \
         refund without human approval. Explain your reasoning before requesting \
         the tool.",
        &["refund"],
        true,
        "refundDecision",
    )
}

/// The translator: render the input text into a target language.
fn translator() -> PrebuiltAgent {
    PrebuiltAgent::new(
        "translator",
        "Translates the input text into a target language, preserving meaning.",
        ModelTier::Fast,
        "You are a translator. Translate the user's text into the requested target \
         language, preserving meaning, tone, and formatting. If no target language \
         is given, translate into English. Do not add explanations. Respond with \
         the translated text only.",
        &[],
        false,
        "translation",
    )
}

/// The sentiment analyzer: classify the emotional tone of the input.
fn sentiment_analyzer() -> PrebuiltAgent {
    PrebuiltAgent::new(
        "sentimentAnalyzer",
        "Classifies the emotional tone of the input text.",
        ModelTier::Fast,
        "You are a sentiment analyzer. Read the user's text and classify its \
         overall sentiment as exactly one of: \"positive\", \"negative\", \
         \"neutral\", \"mixed\". Respond with the single label only, lowercase, no \
         punctuation or explanation.",
        &[],
        false,
        "sentiment",
    )
}

/// The entity extractor: pull named entities out of text as a JSON array.
fn entity_extractor() -> PrebuiltAgent {
    PrebuiltAgent::new(
        "entityExtractor",
        "Extracts named entities from text as a JSON array.",
        ModelTier::Fast,
        "You extract named entities from text. Return a single JSON array where \
         each element is an object { \"text\": string, \"type\": one of \
         \"person\" | \"organization\" | \"location\" | \"date\" | \"other\" }. \
         Return an empty array if there are no entities. Respond with the JSON \
         array only, no code fences and no commentary.",
        &[],
        false,
        "entities",
    )
}

/// The PII redactor: mask personal data while preserving the rest of the text.
fn pii_redactor() -> PrebuiltAgent {
    PrebuiltAgent::new(
        "piiRedactor",
        "Redacts personally identifiable information from the input text.",
        ModelTier::Fast,
        "You are a PII redactor. Rewrite the user's text replacing any personally \
         identifiable information (names, emails, phone numbers, addresses, \
         government ids, payment details) with a bracketed placeholder such as \
         [REDACTED_EMAIL] or [REDACTED_NAME]. Preserve all other text exactly. \
         Respond with the redacted text only.",
        &[],
        false,
        "redacted",
    )
}

/// The intent classifier: map the input to a single conversational intent.
fn intent_classifier() -> PrebuiltAgent {
    PrebuiltAgent::new(
        "intentClassifier",
        "Maps the input to a single conversational intent label.",
        ModelTier::Fast,
        "You are an intent classifier. Classify the user's message into exactly \
         one intent label: \"question\", \"request\", \"complaint\", \"feedback\", \
         \"chitchat\", \"other\". Respond with the single label only, lowercase, \
         no punctuation or explanation.",
        &[],
        false,
        "intent",
    )
}

/// The title generator: produce a short, descriptive title for the input.
fn title_generator() -> PrebuiltAgent {
    PrebuiltAgent::new(
        "titleGenerator",
        "Generates a short, descriptive title for the input text.",
        ModelTier::Fast,
        "You generate titles. Read the user's text and produce a single concise, \
         descriptive title of at most ten words that captures its main topic. Use \
         title case. Respond with the title only, no quotation marks and no \
         explanation.",
        &[],
        false,
        "title",
    )
}

/// The keyword extractor: pull the key terms out of the input as a JSON array.
fn keyword_extractor() -> PrebuiltAgent {
    PrebuiltAgent::new(
        "keywordExtractor",
        "Extracts the key terms from the input text as a JSON array.",
        ModelTier::Fast,
        "You extract keywords. Read the user's text and return a single JSON array \
         of the most important keywords or key phrases (lowercase strings), most \
         significant first, with no duplicates. Respond with the JSON array only, \
         no code fences and no commentary.",
        &[],
        false,
        "keywords",
    )
}

/// The question answerer: answer a question directly and concisely.
fn question_answerer() -> PrebuiltAgent {
    PrebuiltAgent::new(
        "questionAnswerer",
        "Answers a question directly and concisely from its own knowledge.",
        ModelTier::Balanced,
        "You are a question answerer. Read the user's question and answer it \
         directly, accurately, and concisely. If you are not certain of the \
         answer, say so rather than guessing. Respond with the answer only.",
        &[],
        false,
        "answer",
    )
}

/// The code reviewer: review a code diff/snippet for correctness and quality.
fn code_reviewer() -> PrebuiltAgent {
    PrebuiltAgent::new(
        "codeReviewer",
        "Reviews a code snippet or diff for correctness, security, and quality.",
        ModelTier::Frontier,
        "You are a senior code reviewer. Review the supplied code or diff for \
         correctness bugs, security issues, performance problems, and readability. \
         Be specific and reference concrete lines or constructs. Prioritise \
         high-impact findings; do not invent issues. Respond with a concise, \
         ordered list of findings, each with a short rationale and a suggested \
         fix.",
        &[],
        false,
        "review",
    )
}

/// The copy editor: polish prose for clarity, grammar, and tone.
fn copy_editor() -> PrebuiltAgent {
    PrebuiltAgent::new(
        "copyEditor",
        "Polishes prose for clarity, grammar, flow, and tone.",
        ModelTier::Creative,
        "You are a copy editor. Rewrite the user's text to improve clarity, \
         grammar, flow, and tone while preserving the original meaning and voice. \
         Do not change the language or invent facts. Respond with the edited text \
         only, no commentary.",
        &[],
        false,
        "edited",
    )
}

/// All prebuilt agents, in a stable declaration order.
fn all() -> Vec<PrebuiltAgent> {
    vec![
        summarizer(),
        classifier(),
        extractor(),
        sql_generator(),
        rag_answerer(),
        refund_approver(),
        translator(),
        sentiment_analyzer(),
        entity_extractor(),
        pii_redactor(),
        intent_classifier(),
        title_generator(),
        keyword_extractor(),
        question_answerer(),
        code_reviewer(),
        copy_editor(),
    ]
}

/// Look up a prebuilt agent definition by its `name`.
///
/// Returns `None` if no agent is registered under that name.
#[must_use]
pub fn prebuilt(name: &str) -> Option<PrebuiltAgent> {
    all().into_iter().find(|agent| agent.name == name)
}

/// List every prebuilt agent definition, in a stable declaration order.
#[must_use]
pub fn list_prebuilt() -> Vec<PrebuiltAgent> {
    all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_all_sixteen_prebuilt_agents() {
        let agents = list_prebuilt();
        assert_eq!(agents.len(), 16);
        let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "summarizer",
                "classifier",
                "extractor",
                "sqlGenerator",
                "ragAnswerer",
                "refundApprover",
                "translator",
                "sentimentAnalyzer",
                "entityExtractor",
                "piiRedactor",
                "intentClassifier",
                "titleGenerator",
                "keywordExtractor",
                "questionAnswerer",
                "codeReviewer",
                "copyEditor"
            ]
        );
    }

    #[test]
    fn new_fast_tier_micro_agents_have_correct_tiers_and_channels() {
        let expected = [
            ("translator", "translation"),
            ("sentimentAnalyzer", "sentiment"),
            ("entityExtractor", "entities"),
            ("piiRedactor", "redacted"),
            ("intentClassifier", "intent"),
            ("titleGenerator", "title"),
            ("keywordExtractor", "keywords"),
        ];
        for (name, channel) in expected {
            let agent = prebuilt(name).expect("registered");
            assert_eq!(agent.tier, ModelTier::Fast, "{name} should be fast tier");
            assert_eq!(agent.output_channel, channel, "{name} output channel");
            assert!(agent.tool_names.is_empty(), "{name} should have no tools");
            assert!(!agent.suspend_for_approval, "{name} should not suspend");
            assert!(!agent.system_prompt.is_empty(), "{name} needs a prompt");
        }
    }

    #[test]
    fn question_answerer_is_balanced() {
        let agent = prebuilt("questionAnswerer").expect("registered");
        assert_eq!(agent.tier, ModelTier::Balanced);
        assert_eq!(agent.output_channel, "answer");
        assert!(agent.tool_names.is_empty());
    }

    #[test]
    fn code_reviewer_is_frontier() {
        let agent = prebuilt("codeReviewer").expect("registered");
        assert_eq!(agent.tier, ModelTier::Frontier);
        assert_eq!(agent.output_channel, "review");
    }

    #[test]
    fn copy_editor_is_creative() {
        let agent = prebuilt("copyEditor").expect("registered");
        assert_eq!(agent.tier, ModelTier::Creative);
        assert_eq!(agent.output_channel, "edited");
    }

    #[test]
    fn every_tier_is_represented_across_the_catalog() {
        let agents = list_prebuilt();
        for tier in ModelTier::ALL {
            assert!(
                agents.iter().any(|a| a.tier == tier),
                "no agent uses tier {tier:?}"
            );
        }
    }

    #[test]
    fn all_new_agents_round_trip_camel_case() {
        for name in [
            "translator",
            "sentimentAnalyzer",
            "entityExtractor",
            "piiRedactor",
            "intentClassifier",
            "titleGenerator",
            "keywordExtractor",
            "questionAnswerer",
            "codeReviewer",
            "copyEditor",
        ] {
            let agent = prebuilt(name).expect("registered");
            let json = serde_json::to_string(&agent).expect("serialises");
            assert!(json.contains("\"systemPrompt\""), "{name} camelCase prompt");
            assert!(json.contains("\"toolNames\""), "{name} camelCase tools");
            assert!(
                json.contains("\"suspendForApproval\""),
                "{name} camelCase suspend"
            );
            assert!(
                json.contains("\"outputChannel\""),
                "{name} camelCase channel"
            );
            let back: PrebuiltAgent = serde_json::from_str(&json).expect("deserialises");
            assert_eq!(back, agent, "{name} round trip");
        }
    }

    #[test]
    fn lookup_by_name_returns_the_definition() {
        let agent = prebuilt("summarizer").expect("summarizer is registered");
        assert_eq!(agent.name, "summarizer");
        assert_eq!(agent.tier, ModelTier::Fast);
        assert!(agent.tool_names.is_empty());
        assert!(!agent.suspend_for_approval);
        assert_eq!(agent.output_channel, "summary");
    }

    #[test]
    fn unknown_name_is_none() {
        assert!(prebuilt("does-not-exist").is_none());
    }

    #[test]
    fn fast_tier_agents_carry_their_tier_and_no_tools() {
        for name in ["summarizer", "classifier", "extractor"] {
            let agent = prebuilt(name).expect("registered");
            assert_eq!(agent.tier, ModelTier::Fast, "{name} should be fast tier");
            assert!(agent.tool_names.is_empty(), "{name} should have no tools");
            assert!(!agent.suspend_for_approval);
        }
    }

    #[test]
    fn sql_generator_is_balanced() {
        let agent = prebuilt("sqlGenerator").expect("registered");
        assert_eq!(agent.tier, ModelTier::Balanced);
        assert_eq!(agent.output_channel, "sql");
    }

    #[test]
    fn rag_answerer_is_balanced_and_documents_component_composition() {
        let agent = prebuilt("ragAnswerer").expect("registered");
        assert_eq!(agent.tier, ModelTier::Balanced);
        // The description documents that it composes the retriever + reranker
        // components plus an agent step.
        assert!(agent.description.contains("retriever"));
        assert!(agent.description.contains("reranker"));
        assert_eq!(agent.output_channel, "answer");
    }

    #[test]
    fn refund_approver_has_refund_tool_and_suspends() {
        let agent = prebuilt("refundApprover").expect("registered");
        assert_eq!(agent.tier, ModelTier::Balanced);
        assert_eq!(agent.tool_names, vec!["refund".to_string()]);
        assert!(agent.suspend_for_approval);
        assert_eq!(agent.output_channel, "refundDecision");
    }

    #[test]
    fn tier_serialises_as_camel_case() {
        let agent = prebuilt("summarizer").expect("registered");
        let json = serde_json::to_string(&agent).expect("serialises");
        assert!(json.contains("\"tier\":\"fast\""));
        assert!(json.contains("\"systemPrompt\""));
        assert!(json.contains("\"toolNames\""));
        assert!(json.contains("\"suspendForApproval\""));
        assert!(json.contains("\"outputChannel\""));
    }

    #[test]
    fn definition_round_trips_through_json() {
        let agent = prebuilt("refundApprover").expect("registered");
        let json = serde_json::to_string(&agent).expect("serialises");
        let back: PrebuiltAgent = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back, agent);
    }
}
