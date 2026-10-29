use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct UnscopedHost;

#[async_trait]
impl HostCallbacks for UnscopedHost {
    async fn on_node(&self, _payload: Value) -> BridgeResult<String> {
        Ok("{}".to_owned())
    }

    fn on_condition(&self, _payload: Value) -> BridgeResult<bool> {
        Ok(false)
    }

    fn on_event(&self, _payload_json: String) {}
}

fn memory_spec(graph: &str, node: &str, namespace: &str) -> EngineSpec {
    serde_json::from_value(json!({
        "graph": {
            "id": graph, "version": "0.0.0", "name": graph,
            "channels": {"agentResult": {"type": "json", "reducer": "replace"}},
            "nodes": [{"id": node, "type": "agent", "label": node}],
            "edges": [], "entryNodeId": node
        },
        "runId": "same-run",
        "agents": {node: {"provider": "mock", "memory": {"namespace": namespace}}}
    }))
    .expect("memory spec parses")
}

async fn recorded_memory_run(spec: &EngineSpec, host: SharedCallbacks) -> Vec<RecordedCall> {
    let journal = Arc::new(Mutex::new(Vec::new()));
    let mode = ReplayMode::Record {
        journal: journal.clone(),
        clock: Arc::new(Mutex::new(Vec::new())),
        tools: Arc::new(Mutex::new(Vec::new())),
        nodes: Arc::new(Mutex::new(Vec::new())),
    };
    let runtime = build_runtime(spec, host, &mode).expect("runtime builds");
    let state = drive(&runtime, spec, Entry::Start)
        .await
        .expect("run drives");
    assert_eq!(state.status, ailu_graph_core::GraphStatus::Completed);
    let calls = journal.lock().unwrap().clone();
    assert!(!calls.is_empty());
    calls
}

fn recalled(call: &RecordedCall) -> bool {
    call.request
        .messages
        .iter()
        .any(|message| message.content.contains("Relevant memory (recalled from"))
}

struct ScopedHost {
    scope: MemoryScope,
    selections: AtomicUsize,
}

impl ScopedHost {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            scope: MemoryScope::default(),
            selections: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl HostCallbacks for ScopedHost {
    async fn on_node(&self, _payload: Value) -> BridgeResult<String> {
        Ok("{}".to_owned())
    }

    fn on_condition(&self, _payload: Value) -> BridgeResult<bool> {
        Ok(false)
    }

    fn on_event(&self, _payload_json: String) {}

    fn memory_scope(&self) -> Option<MemoryScope> {
        self.selections.fetch_add(1, Ordering::SeqCst);
        Some(self.scope.clone())
    }
}

#[tokio::test]
async fn independent_runtime_builds_cannot_recall_colliding_graph_memory() {
    for namespace in [
        "isolation-regression-default",
        "",
        "tenant:victim/a",
        "tenant%3Avictim%2Fa",
    ] {
        let spec = memory_spec("copied-graph", "copied-node", namespace);
        let host: SharedCallbacks = Arc::new(UnscopedHost);
        let first = recorded_memory_run(&spec, host.clone()).await;
        assert!(!recalled(&first[0]));
        let second = recorded_memory_run(&spec, host).await;
        assert!(
            !recalled(&second[0]),
            "a second build recalled the first graph's memory"
        );
    }
}

#[tokio::test]
async fn only_host_capability_reuse_enables_cross_run_recall() {
    let victim = ScopedHost::new();
    let attacker = ScopedHost::new();
    let spec = memory_spec("victim-graph", "victim-agent", "tenant:victim:memory");
    assert!(!recalled(
        &recorded_memory_run(&spec, victim.clone()).await[0]
    ));

    // Copying every serialized identity, including the namespace, grants nothing.
    assert!(!recalled(
        &recorded_memory_run(&spec, attacker.clone()).await[0]
    ));

    // Nor does changing graph/run/node identity select a different host's store.
    let mut other = memory_spec("other-graph", "other-agent", "tenant:victim:memory");
    other.run_id = Some("other-run".to_owned());
    assert!(!recalled(
        &recorded_memory_run(&other, ScopedHost::new()).await[0]
    ));

    // A clone is an explicit host grant. It works across freshly built runtimes.
    assert!(recalled(
        &recorded_memory_run(&spec, victim.clone()).await[0]
    ));
    assert_eq!(victim.selections.load(Ordering::SeqCst), 2);
    let items = victim
        .scope
        .store
        .recall_by_vector("tenant:victim:memory", &[1.0], 10)
        .await
        .unwrap();
    assert!(!items.is_empty());
    assert!(items
        .iter()
        .all(|item| item.provenance.principal.as_deref() == Some("agent:victim-agent")));
}

#[tokio::test]
async fn agents_in_one_default_runtime_still_share_logical_memory() {
    let mut spec = memory_spec("g", "first", "shared-in-one-build");
    let mut second = spec.graph.nodes[0].clone();
    second.id = NodeId::from("second");
    spec.graph.nodes.push(second);
    spec.graph.edges.push(
        serde_json::from_value(json!({
            "id": "next", "from": "first", "to": "second", "type": "default"
        }))
        .expect("edge parses"),
    );
    spec.agents
        .insert("second".to_owned(), spec.agents["first"].clone());
    let calls = recorded_memory_run(&spec, Arc::new(UnscopedHost)).await;
    assert_eq!(calls.len(), 2);
    assert!(!recalled(&calls[0]));
    assert!(recalled(&calls[1]));
}

#[test]
fn scope_is_selected_once_for_ordinary_nested_and_map_agents() {
    let mut spec = memory_spec("g", "ordinary", "shared");
    let nested = memory_spec("nested", "child", "shared");
    spec.subgraphs.push(nested.graph);
    spec.agents.extend(nested.agents);
    let map = memory_spec("map", "mapped", "shared");
    spec.graph.nodes.extend(map.graph.nodes);
    spec.map_agents.insert(
        "mapped".to_owned(),
        serde_json::from_value(json!({
            "overChannel": "items", "joinAt": "joined",
            "agent": {"provider": "mock", "memory": {"namespace": "shared"}}
        }))
        .expect("map spec parses"),
    );
    let host = ScopedHost::new();
    build_runtime(&spec, host.clone(), &ReplayMode::Live).expect("all handlers build");
    assert_eq!(host.selections.load(Ordering::SeqCst), 1);
}

// Exercise the actual bridge-installed tools through a scripted local provider.
// Tool arguments cannot be supplied by the canned mock, which always sends {}.
async fn memory_tool_run(
    mut spec: EngineSpec,
    host: SharedCallbacks,
    tool_calls: Vec<(&str, Value)>,
) -> Vec<RecordedCall> {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").expect("local provider binds");
    let address = listener.local_addr().unwrap();
    let mut replies: Vec<Value> = tool_calls
        .into_iter()
        .enumerate()
        .map(|(i, (name, input))| {
            json!({"role": "assistant", "content": null, "tool_calls": [{
                "id": format!("call-{i}"), "type": "function",
                "function": {"name": name, "arguments": input.to_string()}
            }]})
        })
        .collect();
    replies.push(json!({"role": "assistant", "content": "FINAL: done"}));
    let server = std::thread::spawn(move || {
        for message in replies {
            let (mut stream, _) = listener.accept().expect("provider accepts");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut buffer = [0; 8192];
                let n = stream.read(&mut buffer).expect("provider reads");
                assert!(n > 0, "provider request ended prematurely");
                request.extend_from_slice(&buffer[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(end) = text.find("\r\n\r\n") {
                    let size = text[..end]
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .expect("request content length");
                    if request.len() >= end + 4 + size {
                        break;
                    }
                }
            }
            let body = json!({"model": "test", "choices": [{
                "message": message,
                "finish_reason": if message.get("tool_calls").is_some() { "tool_calls" } else { "stop" }
            }]}).to_string();
            write!(stream, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    for agent in spec.agents.values_mut() {
        agent.provider = "openai".to_owned();
        agent.model = Some("test".to_owned());
        agent.base_url = Some(format!("http://{address}/v1"));
    }
    let calls = recorded_memory_run(&spec, host).await;
    server.join().expect("provider completes");
    calls
}

fn tool_observations(call: &RecordedCall) -> Vec<Value> {
    call.request
        .messages
        .iter()
        .filter(|message| message.role == "tool")
        .map(|message| serde_json::from_str(&message.content).expect("JSON observation"))
        .collect()
}

#[tokio::test]
async fn memory_tools_and_middleware_share_only_the_host_capability() {
    let victim = ScopedHost::new();
    let spec = memory_spec("g", "a", "copied-namespace");
    recorded_memory_run(&spec, victim.clone()).await; // Middleware persists the reasoning trace.

    let calls = memory_tool_run(
        spec.clone(),
        victim.clone(),
        vec![
            ("rememberMemory", json!({"text": "victim-private-fact"})),
            ("recallMemory", json!({"query": "done victim-private-fact"})),
        ],
    )
    .await;
    let observations = tool_observations(calls.last().unwrap());
    assert_eq!(observations[0]["remembered"], json!(true));
    let hits = observations[1]["items"].as_array().unwrap();
    assert!(hits
        .iter()
        .any(|hit| hit["text"] == json!("victim-private-fact")));
    assert!(
        hits.iter().any(|hit| hit["text"]
            .as_str()
            .is_some_and(|text| text.contains("final:done"))),
        "tools must recall middleware writes"
    );

    let next = recorded_memory_run(&spec, victim.clone()).await;
    assert!(
        next[0]
            .request
            .messages
            .iter()
            .any(|message| message.content.contains("victim-private-fact")),
        "middleware must recall tool writes"
    );

    // A namespace collision with different graph/run/node IDs also grants nothing.
    // Attempt to overwrite the same text key and add a malicious fact.
    let attacker = ScopedHost::new();
    let mut attacker_spec = memory_spec("attacker-graph", "attacker-agent", "copied-namespace");
    attacker_spec.run_id = Some("attacker-run".to_owned());
    let calls = memory_tool_run(
        attacker_spec,
        attacker,
        vec![
            ("recallMemory", json!({"query": "victim-private-fact"})),
            ("rememberMemory", json!({"text": "victim-private-fact"})),
            ("rememberMemory", json!({"text": "attacker-poison"})),
        ],
    )
    .await;
    assert!(!recalled(&calls[0]));
    assert_eq!(
        tool_observations(calls.last().unwrap())[0]["items"],
        json!([])
    );
    let items = victim
        .scope
        .store
        .recall_by_vector("copied-namespace", &[1.0], 100)
        .await
        .unwrap();
    assert!(items.iter().any(|item| item.text == "victim-private-fact"));
    assert!(items.iter().all(|item| item.text != "attacker-poison"));
    assert!(items
        .iter()
        .all(|item| item.provenance.principal.as_deref() == Some("agent:a")));
}
