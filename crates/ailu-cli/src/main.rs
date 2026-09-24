//! ailu CLI — compile, validate, run and inspect Ailu graphs via the
//! already-ported Rust engine.
//!
//! Command surface mirrors the TypeScript `@ailu-ai/cli`:
//!
//! - `ailu compile <file.yaml>`  — compile the graph and print the resulting
//!   `GraphDefinition` as pretty JSON.
//! - `ailu validate <file.yaml>` — compile and report `valid`, or the
//!   validation errors (exit 1).
//! - `ailu run <file.yaml> [--input '<json>']` — compile, register a
//!   pass-through handler for every non-human-gate node, stream a one-line event
//!   journal to stderr, then print the final `GraphState` as JSON.
//! - `ailu inspect <file.yaml>` — print a human-readable summary of the graph.
//!
//! Node execution in `run` is intentionally a topology-only walk: action / agent
//! / tool nodes no-op on their channels. The CLI exercises the graph's *routing*
//! and *human-gate suspension* deterministically; wiring real agent/tool handlers
//! is a library concern (the agent crates), not the CLI's.
//!
//! Exit codes: `0` success, `1` user/validation error, `2` usage error.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::process::ExitCode;

use ailu_graph_ailu::{compile_graph_yaml, DslError};
use ailu_graph_core::{GraphDefinition, GraphStatus, NodeType};
use ailu_graph_runtime::{
    sync_handler, GraphRuntime, InMemoryConditionRegistry, InMemoryNodeRegistry, NodeOutput,
    NodeRegistry, RunEvent,
};
use serde_json::Value;

/// Exit codes, named for clarity. `ExitCode::from(u8)` is used at the boundary.
const EXIT_OK: u8 = 0;
const EXIT_USER_ERROR: u8 = 1;
const EXIT_USAGE_ERROR: u8 = 2;

/// A parsed CLI invocation. `args` is the user-supplied argument list (i.e.
/// `std::env::args` with the program name already dropped).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Command {
    Compile {
        file: String,
    },
    Validate {
        file: String,
    },
    Run {
        file: String,
        input: Option<String>,
    },
    Inspect {
        file: String,
    },
    Help,
    /// A usage error: the message is printed to stderr and the process exits 2.
    Usage(String),
}

const USAGE: &str = "\
ailu — Ailu graph CLI

USAGE:
    ailu <COMMAND> [ARGS]

COMMANDS:
    compile <file.yaml>             Compile a graph and print its GraphDefinition as JSON
    validate <file.yaml>            Validate a graph; prints `valid` or the errors
    run <file.yaml> [--input <json>]
                                    Walk the graph topology: routing and human-gate
                                    suspension are REAL; agent/tool/action node handlers
                                    are no-ops (use the @ailu-ai/graph-sdk to run real
                                    agents). Streams an event journal to stderr and prints
                                    the final GraphState as JSON.
    inspect <file.yaml>             Print a human-readable summary of the graph
    --help, -h                      Show this help

EXIT CODES:
    0  success
    1  user / validation error
    2  usage error";

/// Parse the argument list (program name already stripped) into a [`Command`].
///
/// Pure and side-effect-free so it can be unit-tested directly.
fn parse_args(args: &[String]) -> Command {
    let Some(command) = args.first() else {
        return Command::Help;
    };

    match command.as_str() {
        "--help" | "-h" | "help" => Command::Help,
        "compile" => match require_file(&args[1..], "compile") {
            Ok(file) => Command::Compile { file },
            Err(message) => Command::Usage(message),
        },
        "validate" => match require_file(&args[1..], "validate") {
            Ok(file) => Command::Validate { file },
            Err(message) => Command::Usage(message),
        },
        "inspect" => match require_file(&args[1..], "inspect") {
            Ok(file) => Command::Inspect { file },
            Err(message) => Command::Usage(message),
        },
        "run" => parse_run(&args[1..]),
        other => Command::Usage(format!("unknown command '{other}'")),
    }
}

/// Extract a single required `<file>` positional from a command's arguments.
fn require_file(rest: &[String], command: &str) -> Result<String, String> {
    match rest.first() {
        Some(file) if !file.starts_with('-') => Ok(file.clone()),
        Some(flag) => Err(format!("{command}: expected a file path, got '{flag}'")),
        None => Err(format!("{command}: missing required <file> argument")),
    }
}

/// Parse `run <file> [--input <json>]`. The order of the positional file and the
/// `--input` flag does not matter.
fn parse_run(rest: &[String]) -> Command {
    let mut file: Option<String> = None;
    let mut input: Option<String> = None;
    let mut index = 0;

    while index < rest.len() {
        let arg = &rest[index];
        match arg.as_str() {
            "--input" => {
                let Some(value) = rest.get(index + 1) else {
                    return Command::Usage("run: --input requires a JSON argument".to_owned());
                };
                input = Some(value.clone());
                index += 2;
            }
            flag if flag.starts_with("--input=") => {
                input = Some(flag.trim_start_matches("--input=").to_owned());
                index += 1;
            }
            flag if flag.starts_with('-') => {
                return Command::Usage(format!("run: unknown flag '{flag}'"));
            }
            positional => {
                if file.is_some() {
                    return Command::Usage(format!(
                        "run: unexpected extra argument '{positional}'"
                    ));
                }
                file = Some(positional.to_owned());
                index += 1;
            }
        }
    }

    match file {
        Some(file) => Command::Run { file, input },
        None => Command::Usage("run: missing required <file> argument".to_owned()),
    }
}

/// Parse the optional `--input` JSON into the initial channel map. An absent or
/// blank input yields an empty map. The JSON must be an object.
fn parse_input(input: Option<&str>) -> Result<BTreeMap<String, Value>, String> {
    let Some(raw) = input else {
        return Ok(BTreeMap::new());
    };
    if raw.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    let value: Value =
        serde_json::from_str(raw).map_err(|error| format!("invalid --input JSON: {error}"))?;
    match value {
        Value::Object(map) => Ok(map.into_iter().collect()),
        other => Err(format!(
            "--input must be a JSON object, got {}",
            json_kind(&other)
        )),
    }
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Render a [`DslError`] as a clear, multi-line, human-readable message for stderr.
fn render_dsl_error(error: &DslError) -> String {
    match error {
        DslError::Parse(message) => format!("YAML parse error: {message}"),
        DslError::DslValidation(diagnostics) => {
            let mut out = String::from("graph validation failed:");
            for diagnostic in diagnostics {
                out.push_str(&format!(
                    "\n  - {:?} {} ({})",
                    diagnostic.code, diagnostic.message, diagnostic.loc
                ));
            }
            out
        }
        DslError::StructuralValidation(errors) => {
            let mut out = String::from("graph structural validation failed:");
            for error in errors {
                let path = if error.path.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", error.path.join("."))
                };
                out.push_str(&format!(
                    "\n  - {:?}: {}{}",
                    error.code, error.message, path
                ));
            }
            out
        }
    }
}

/// Build a human-readable summary of a compiled graph (the `inspect` body).
///
/// Pure so it can be unit-tested against a known `GraphDefinition`.
fn format_inspect(graph: &GraphDefinition) -> String {
    let mut out = String::new();
    out.push_str(&format!("Graph: {} ({})\n", graph.name, graph.id));
    out.push_str(&format!("Version: {}\n", graph.version));
    out.push_str(&format!("Entry node: {}\n", graph.entry_node_id));
    if let Some(limit) = graph.recursion_limit {
        out.push_str(&format!("Recursion limit: {limit}\n"));
    }

    out.push_str(&format!("\nChannels ({}):\n", graph.channels.len()));
    if graph.channels.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for (name, channel) in &graph.channels {
            out.push_str(&format!(
                "  - {name}: {} (reducer: {})\n",
                channel.channel_type,
                reducer_label(&channel.reducer)
            ));
        }
    }

    out.push_str(&format!("\nNodes ({}):\n", graph.nodes.len()));
    for node in &graph.nodes {
        out.push_str(&format!(
            "  - {} [{}] \"{}\"\n",
            node.id,
            node_type_label(&node.node_type),
            node.label
        ));
    }

    out.push_str(&format!("\nEdges ({}):\n", graph.edges.len()));
    if graph.edges.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for edge in &graph.edges {
            let condition = match &edge.condition {
                Some(condition) => format!(" when `{condition}`"),
                None => String::new(),
            };
            out.push_str(&format!(
                "  - {}: {} -> {} [{}]{}\n",
                edge.id,
                edge.from,
                edge.to,
                edge_type_label(&edge.edge_type),
                condition
            ));
        }
    }

    out
}

fn node_type_label(node_type: &NodeType) -> &'static str {
    match node_type {
        NodeType::Action => "action",
        NodeType::Agent => "agent",
        NodeType::Tool => "tool",
        NodeType::HumanGate => "human-gate",
        NodeType::Subgraph => "subgraph",
    }
}

fn edge_type_label(edge_type: &ailu_graph_core::EdgeType) -> &'static str {
    match edge_type {
        ailu_graph_core::EdgeType::Default => "default",
        ailu_graph_core::EdgeType::Conditional => "conditional",
        ailu_graph_core::EdgeType::Error => "error",
    }
}

fn reducer_label(reducer: &ailu_graph_core::ChannelReducer) -> &'static str {
    match reducer {
        ailu_graph_core::ChannelReducer::Replace => "replace",
        ailu_graph_core::ChannelReducer::Append => "append",
        ailu_graph_core::ChannelReducer::Merge => "merge",
    }
}

/// Render a single [`RunEvent`] as a one-line journal entry for stderr.
fn format_event(event: &RunEvent) -> String {
    match event {
        RunEvent::NodeStarted { node_id, .. } => format!("node_started   {node_id}"),
        RunEvent::NodeCompleted { node_id, .. } => format!("node_completed {node_id}"),
        RunEvent::NodeFailed {
            node_id,
            error,
            attempt,
            ..
        } => format!("node_failed    {node_id} (attempt {attempt}): {error}"),
        RunEvent::RunSuspended {
            node_id, reason, ..
        } => format!("run_suspended  {node_id} ({reason})"),
        RunEvent::RunResumed { node_id, .. } => format!("run_resumed    {node_id}"),
        RunEvent::RunCompleted { .. } => "run_completed".to_owned(),
        RunEvent::RunFailed { error, .. } => format!("run_failed     {error}"),
        // ADR 0044 — cooperative cancellation. `node_id` is the node that did NOT run.
        RunEvent::RunCancelled { node_id, .. } => format!("run_cancelled  before {node_id}"),
        // ADR 0076 — retries exhausted but an error edge rerouted instead of failing the run.
        RunEvent::NodeErrorRouted {
            node_id,
            to_node_id,
            category,
            ..
        } => format!("node_error_routed {node_id} -> {to_node_id} ({category:?})"),
        // ADR 0033 phase 13: observational per-token delta. Rendered compactly (and
        // only ever reachable here if a caller opts into streaming); never durable.
        RunEvent::TokenDelta {
            node_id, spawn_id, ..
        } => match spawn_id {
            Some(spawn) => format!("token_delta    {node_id} (spawn {spawn})"),
            None => format!("token_delta    {node_id}"),
        },
    }
}

// --- I/O command handlers ---------------------------------------------------

/// Read a file, compiling its contents. Maps both the read error and the DSL
/// error to a printable message.
fn compile_file(path: &str) -> Result<GraphDefinition, String> {
    let yaml = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
    compile_graph_yaml(&yaml).map_err(|error| render_dsl_error(&error))
}

fn cmd_compile(path: &str) -> u8 {
    match compile_file(path) {
        Ok(graph) => match serde_json::to_string_pretty(&graph) {
            Ok(json) => {
                println!("{json}");
                EXIT_OK
            }
            Err(error) => {
                eprintln!("failed to serialize graph: {error}");
                EXIT_USER_ERROR
            }
        },
        Err(message) => {
            eprintln!("{message}");
            EXIT_USER_ERROR
        }
    }
}

fn cmd_validate(path: &str) -> u8 {
    match compile_file(path) {
        Ok(_) => {
            println!("valid");
            EXIT_OK
        }
        Err(message) => {
            eprintln!("{message}");
            EXIT_USER_ERROR
        }
    }
}

fn cmd_inspect(path: &str) -> u8 {
    match compile_file(path) {
        Ok(graph) => {
            print!("{}", format_inspect(&graph));
            EXIT_OK
        }
        Err(message) => {
            eprintln!("{message}");
            EXIT_USER_ERROR
        }
    }
}

async fn cmd_run(path: &str, input: Option<&str>) -> u8 {
    let graph = match compile_file(path) {
        Ok(graph) => graph,
        Err(message) => {
            eprintln!("{message}");
            return EXIT_USER_ERROR;
        }
    };

    let initial = match parse_input(input) {
        Ok(initial) => initial,
        Err(message) => {
            eprintln!("{message}");
            return EXIT_USER_ERROR;
        }
    };

    // Register a pass-through handler for every non-human-gate node. Human-gate
    // nodes never reach a handler — the runtime suspends before invoking one — so
    // they need no registration. Action / agent / tool nodes simply no-op on the
    // channels: the CLI walks the graph's topology and routing deterministically.
    let mut registry = InMemoryNodeRegistry::new();
    for node in &graph.nodes {
        if node.node_type == NodeType::HumanGate {
            continue;
        }
        registry.register(
            node.id.clone(),
            sync_handler(|_state| NodeOutput::default()),
        );
    }

    let runtime = GraphRuntime::new(graph, registry, InMemoryConditionRegistry::new());

    // Stream a one-line-per-event journal to stderr as the run advances.
    runtime.on_event(Box::new(|event| {
        eprintln!("{}", format_event(event));
    }));

    let run_id = ailu_graph_core::RunId::from(format!("cli-run:{}", now_millis()));
    match runtime.start(run_id, initial).await {
        Ok(state) => {
            match serde_json::to_string_pretty(&state) {
                Ok(json) => println!("{json}"),
                Err(error) => {
                    eprintln!("failed to serialize final state: {error}");
                    return EXIT_USER_ERROR;
                }
            }
            // A human gate legitimately pauses the run: report it and exit 0.
            match state.status {
                GraphStatus::Suspended => {
                    eprintln!(
                        "run suspended at node '{}' (awaiting human gate)",
                        state.current_node_id
                    );
                    EXIT_OK
                }
                GraphStatus::Failed => {
                    eprintln!("run failed");
                    EXIT_USER_ERROR
                }
                // ADR 0044: cancellation is a deliberate stop, not an error — exit 0, but say so
                // rather than letting it pass silently as if the run had completed.
                GraphStatus::Cancelled => {
                    eprintln!(
                        "run cancelled before node '{}' (resumable from its last checkpoint)",
                        state.current_node_id
                    );
                    EXIT_OK
                }
                _ => EXIT_OK,
            }
        }
        Err(error) => {
            eprintln!("run error: {error}");
            EXIT_USER_ERROR
        }
    }
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

/// The runtime's event bus uses interior mutability that is not `Send`, so the
/// async run is driven on a current-thread executor.
#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match parse_args(&args) {
        Command::Compile { file } => cmd_compile(&file),
        Command::Validate { file } => cmd_validate(&file),
        Command::Inspect { file } => cmd_inspect(&file),
        Command::Run { file, input } => cmd_run(&file, input.as_deref()).await,
        Command::Help => {
            println!("{USAGE}");
            EXIT_OK
        }
        Command::Usage(message) => {
            eprintln!("error: {message}\n");
            eprintln!("{USAGE}");
            EXIT_USAGE_ERROR
        }
    };
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ailu_graph_core::{
        ChannelDefinition, ChannelReducer, EdgeDefinition, EdgeId, EdgeType, GraphId,
        NodeDefinition, NodeId, RunId,
    };
    use serde_json::json;

    fn s(value: &str) -> String {
        value.to_owned()
    }

    #[test]
    fn parse_args_no_args_is_help() {
        assert_eq!(parse_args(&[]), Command::Help);
    }

    #[test]
    fn parse_args_help_flags() {
        assert_eq!(parse_args(&[s("--help")]), Command::Help);
        assert_eq!(parse_args(&[s("-h")]), Command::Help);
        assert_eq!(parse_args(&[s("help")]), Command::Help);
    }

    #[test]
    fn parse_args_compile_requires_a_file() {
        assert_eq!(
            parse_args(&[s("compile"), s("g.yaml")]),
            Command::Compile { file: s("g.yaml") }
        );
        assert!(matches!(parse_args(&[s("compile")]), Command::Usage(_)));
    }

    #[test]
    fn parse_args_validate_and_inspect() {
        assert_eq!(
            parse_args(&[s("validate"), s("g.yaml")]),
            Command::Validate { file: s("g.yaml") }
        );
        assert_eq!(
            parse_args(&[s("inspect"), s("g.yaml")]),
            Command::Inspect { file: s("g.yaml") }
        );
    }

    #[test]
    fn parse_args_unknown_command_is_usage_error() {
        assert!(matches!(parse_args(&[s("frobnicate")]), Command::Usage(_)));
    }

    #[test]
    fn parse_run_accepts_input_in_either_order() {
        assert_eq!(
            parse_args(&[s("run"), s("g.yaml"), s("--input"), s("{\"a\":1}")]),
            Command::Run {
                file: s("g.yaml"),
                input: Some(s("{\"a\":1}"))
            }
        );
        assert_eq!(
            parse_args(&[s("run"), s("--input"), s("{}"), s("g.yaml")]),
            Command::Run {
                file: s("g.yaml"),
                input: Some(s("{}"))
            }
        );
        assert_eq!(
            parse_args(&[s("run"), s("g.yaml"), s("--input={\"a\":1}")]),
            Command::Run {
                file: s("g.yaml"),
                input: Some(s("{\"a\":1}"))
            }
        );
    }

    #[test]
    fn parse_run_without_a_file_is_a_usage_error() {
        assert!(matches!(parse_args(&[s("run")]), Command::Usage(_)));
        assert!(matches!(
            parse_args(&[s("run"), s("--input")]),
            Command::Usage(_)
        ));
    }

    #[test]
    fn parse_input_defaults_to_empty_map() {
        assert_eq!(parse_input(None).unwrap(), BTreeMap::new());
        assert_eq!(parse_input(Some("   ")).unwrap(), BTreeMap::new());
    }

    #[test]
    fn parse_input_parses_a_json_object() {
        let parsed = parse_input(Some("{\"count\": 3}")).unwrap();
        assert_eq!(parsed.get("count"), Some(&json!(3)));
    }

    #[test]
    fn parse_input_rejects_non_objects_and_garbage() {
        assert!(parse_input(Some("[1,2,3]")).is_err());
        assert!(parse_input(Some("42")).is_err());
        assert!(parse_input(Some("not json")).is_err());
    }

    fn demo_graph() -> GraphDefinition {
        let mut channels = BTreeMap::new();
        channels.insert(
            s("messages"),
            ChannelDefinition {
                channel_type: s("messages"),
                reducer: ChannelReducer::Append,
                default: None,
                no_log: false,
            },
        );
        GraphDefinition {
            id: GraphId::from("demo"),
            version: s("1.0.0"),
            name: s("Demo"),
            recursion_limit: Some(10),
            channels,
            nodes: vec![
                NodeDefinition {
                    id: NodeId::from("a"),
                    node_type: NodeType::Action,
                    label: s("Start"),
                    subgraph_id: None,
                    input_mapping: None,
                    output_mapping: None,
                    fan_out: None,
                    map_subgraph: None,
                    retry_policy: None,
                    metadata: None,
                },
                NodeDefinition {
                    id: NodeId::from("review"),
                    node_type: NodeType::HumanGate,
                    label: s("Review"),
                    subgraph_id: None,
                    input_mapping: None,
                    output_mapping: None,
                    fan_out: None,
                    map_subgraph: None,
                    retry_policy: None,
                    metadata: None,
                },
            ],
            edges: vec![EdgeDefinition {
                id: EdgeId::from("e1"),
                from: NodeId::from("a"),
                to: NodeId::from("review"),
                edge_type: EdgeType::Default,
                condition: None,
            }],
            entry_node_id: NodeId::from("a"),
            metadata: None,
        }
    }

    #[test]
    fn format_inspect_lists_the_graph_surface() {
        let summary = format_inspect(&demo_graph());
        assert!(summary.contains("Graph: Demo (demo)"));
        assert!(summary.contains("Version: 1.0.0"));
        assert!(summary.contains("Entry node: a"));
        assert!(summary.contains("Recursion limit: 10"));
        assert!(summary.contains("Channels (1):"));
        assert!(summary.contains("- messages: messages (reducer: append)"));
        assert!(summary.contains("Nodes (2):"));
        assert!(summary.contains("- a [action] \"Start\""));
        assert!(summary.contains("- review [human-gate] \"Review\""));
        assert!(summary.contains("Edges (1):"));
        assert!(summary.contains("- e1: a -> review [default]"));
    }

    #[test]
    fn format_event_renders_one_line_per_variant() {
        let started = RunEvent::NodeStarted {
            run_id: RunId::from("r"),
            node_id: NodeId::from("a"),
            timestamp: s("0"),
        };
        assert_eq!(format_event(&started), "node_started   a");

        let suspended = RunEvent::RunSuspended {
            run_id: RunId::from("r"),
            node_id: NodeId::from("review"),
            reason: s("human-gate"),
            timestamp: s("0"),
        };
        assert_eq!(
            format_event(&suspended),
            "run_suspended  review (human-gate)"
        );

        let completed = RunEvent::RunCompleted {
            run_id: RunId::from("r"),
            timestamp: s("0"),
        };
        assert_eq!(format_event(&completed), "run_completed");
    }

    #[test]
    fn render_dsl_error_formats_each_variant() {
        let parse = render_dsl_error(&DslError::Parse(s("boom")));
        assert!(parse.contains("YAML parse error: boom"));

        let structural = render_dsl_error(&DslError::StructuralValidation(vec![
            ailu_graph_core::ValidationError::new(
                ailu_graph_core::ValidationErrorCode::DuplicateNodeId,
                "duplicate node id 'n1'",
                vec![s("nodes"), s("n1")],
            ),
        ]));
        assert!(structural.contains("structural validation failed"));
        assert!(structural.contains("DuplicateNodeId"));
        assert!(structural.contains("[nodes.n1]"));
    }
}
