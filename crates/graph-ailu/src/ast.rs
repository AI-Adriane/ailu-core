//! AST types for the graph DSL — Rust mirror of `packages/graph-ailu/src/ast/types.ts`.

use ailu_graph_core::{ChannelReducer, EdgeType, NodeType};
use serde::{Deserialize, Serialize};

/// Source location attached to every AST node and diagnostic. The TS parser
/// currently pins everything to `line: 1, col: 1` (it does not track YAML
/// positions yet); we mirror that.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Loc {
    pub line: u32,
    pub col: u32,
    pub file: String,
}

impl Loc {
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

/// A `<id>@<semver>` reference (e.g. `risk-agent@1.0.0`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionedRef {
    pub id: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConditionAst {
    pub value: String,
    pub loc: Loc,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChannelAst {
    pub name: String,
    /// Channel value type (free-form string; defaults to `"unknown"`).
    pub channel_type: String,
    pub reducer: ChannelReducer,
    /// Present iff the YAML channel definition carried a `default` key
    /// (a present-but-null key yields `Some(Value::Null)`, like in TS).
    pub default: Option<serde_json::Value>,
    pub loc: Loc,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeAst {
    pub id: String,
    pub node_type: NodeType,
    pub label: String,
    pub subgraph: Option<VersionedRef>,
    pub loc: Loc,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeAst {
    pub id: String,
    pub from: String,
    pub to: String,
    pub edge_type: EdgeType,
    pub condition: Option<ConditionAst>,
    pub loc: Loc,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GraphAst {
    pub id: String,
    pub version: String,
    pub name: String,
    pub recursion_limit: Option<u32>,
    pub entry_node_id: String,
    pub channels: Vec<ChannelAst>,
    pub nodes: Vec<NodeAst>,
    pub edges: Vec<EdgeAst>,
    pub loc: Loc,
}
