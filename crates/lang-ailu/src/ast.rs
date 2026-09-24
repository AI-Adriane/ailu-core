//! AST types for the prompt/agent/chain DSL — Rust mirror of
//! `packages/lang-ailu/src/ast/types.ts`.
//!
//! Every AST node carries a discriminant `_kind` and a source location `_loc`.
//! The TS parser pins every location to `line: 1, col: 1` (it does not track
//! YAML positions yet); we mirror that via [`Loc::start_of`].

use serde::{Deserialize, Serialize};

/// Source location attached to every AST node and diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Loc {
    pub line: u32,
    pub col: u32,
    pub file: String,
}

impl Loc {
    /// The `{ line: 1, col: 1, file }` location the TS `createLoc` returns by
    /// default for inline content.
    pub fn start_of(file: &str) -> Self {
        Loc {
            line: 1,
            col: 1,
            file: file.to_owned(),
        }
    }
}

impl std::fmt::Display for Loc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.col)
    }
}

/// A `prompt` DSL document — mirror of the TS `PromptAST`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptAst {
    #[serde(rename = "_kind")]
    pub kind: PromptKind,
    #[serde(rename = "_loc")]
    pub loc: Loc,
    pub name: String,
    pub template: String,
    pub variables: Vec<String>,
}

/// An `agent` DSL document — mirror of the TS `AgentAST`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAst {
    #[serde(rename = "_kind")]
    pub kind: AgentKind,
    #[serde(rename = "_loc")]
    pub loc: Loc,
    pub id: String,
    pub description: String,
    pub prompt: String,
    pub tools: Vec<String>,
}

/// A single step of a `chain` DSL document — mirror of the TS `ChainStepAST`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChainStepAst {
    #[serde(rename = "_kind")]
    pub kind: ChainStepKind,
    #[serde(rename = "_loc")]
    pub loc: Loc,
    #[serde(rename = "agentId")]
    pub agent_id: String,
    /// Present iff the YAML step carried an `input` object (an absent or
    /// non-object `input` collapses to `None`, like the TS `?:` field).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Map<String, serde_json::Value>>,
}

/// A `chain` DSL document — mirror of the TS `ChainAST`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChainAst {
    #[serde(rename = "_kind")]
    pub kind: ChainKind,
    #[serde(rename = "_loc")]
    pub loc: Loc,
    pub id: String,
    pub steps: Vec<ChainStepAst>,
}

macro_rules! kind_tag {
    ($name:ident, $value:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name {
            #[serde(rename = $value)]
            Tag,
        }

        impl Default for $name {
            fn default() -> Self {
                $name::Tag
            }
        }
    };
}

kind_tag!(
    PromptKind,
    "prompt",
    "The constant `\"prompt\"` discriminant."
);
kind_tag!(AgentKind, "agent", "The constant `\"agent\"` discriminant.");
kind_tag!(
    ChainStepKind,
    "chain_step",
    "The constant `\"chain_step\"` discriminant."
);
kind_tag!(ChainKind, "chain", "The constant `\"chain\"` discriminant.");
