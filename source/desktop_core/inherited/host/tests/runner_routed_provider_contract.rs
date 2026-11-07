use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::inference::provider_session::{
    OpenRouterCheckpoint, ProviderSessionError, RoutedProviderCheckpoint, RoutedToolDefinition,
};
use mahayana_host_runtime::extensions::transcript::runner_registry::{
    RUN_DIRECT_USER_INTERRUPT_REASON, RUN_WATCHDOG_INTERRUPT_REASON, TranscriptRunnerRegistry,
};
use mahayana_host_runtime::runner::production_turn_run_shell_adapter::RoutedProviderCheckpointStore;
use mahayana_host_runtime::runner::routed_provider_runtime::{
    ProductionRoutedProviderCheckpointStore, ROUTED_MCP_PROTOCOL_VERSION,
    RoutedProviderTaskRegistry, RoutedToolBridge, start_routed_mcp_server,
};
use serde_json::{Value, json};

struct FakeBridge;

impl RoutedToolBridge for FakeBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(vec![RoutedToolDefinition {
            name: "github_search".into(),
            provider_identifier: "github".into(),
            tool_name: "search".into(),
            description: Some("Search GitHub".into()),
            input_schema: json!({"type":"object"}),
        }])
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        assert_eq!(tool.provider_identifier, "github");
        assert_eq!(args["q"], "rust");
        assert!(!tool_call_id.is_empty());
        Ok(json!({"content":[{"type":"text","text":"tool-result"}]}))
    }
}

fn post(url: &str, body: Value) -> Value {
    let without_scheme = url.strip_prefix("http://").expect("http");
    let (authority, path) = without_scheme.split_once('/').expect("path");
    let mut stream = TcpStream::connect(authority).expect("connect");
    let payload = serde_json::to_vec(&body).expect("payload");
    write!(
        stream,
        "POST /{path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    )
    .expect("headers");
    stream.write_all(&payload).expect("body");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("response");
    let body = response.split_once("\r\n\r\n").expect("http body").1;
    serde_json::from_str(body).expect("json")
}

#[test]
fn runner_owned_mcp_bridge_lists_and_executes_host_tools() {
    let mut server = start_routed_mcp_server(Arc::new(FakeBridge)).expect("server");
    let initialized = post(
        server.url(),
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
    );
    assert_eq!(
        initialized["result"]["protocolVersion"],
        ROUTED_MCP_PROTOCOL_VERSION
    );

    let tools = post(
        server.url(),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    );
    assert_eq!(tools["result"]["tools"][0]["name"], "github_search");

    let called = post(
        server.url(),
        json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{"name":"github_search","arguments":{"q":"rust"}}
        }),
    );
    assert_eq!(called["result"]["content"][0]["text"], "tool-result");
    server.close();
}

#[test]
fn runner_provider_registry_cancels_by_stream_and_retires_finished_runs() {
    let registry = RoutedProviderTaskRegistry::default();
    let cancellation = registry.register("stream-cancel").expect("register");
    assert_eq!(registry.active_count(), 1);
    assert!(!cancellation.is_cancelled());
    assert!(registry.cancel("stream-cancel", "user cancelled"));
    assert!(cancellation.is_cancelled());
    assert_eq!(cancellation.reason().as_deref(), Some("user cancelled"));
    assert!(!registry.cancel("missing-stream", "ignored"));
    registry.finish("stream-cancel");
    assert_eq!(registry.active_count(), 0);
}

#[test]
fn transcript_runner_registry_cancels_all_streams_for_deleted_agent_with_reason() {
    let tasks = Arc::new(RoutedProviderTaskRegistry::default());
    let registry = TranscriptRunnerRegistry::new(Arc::clone(&tasks));
    let first = registry
        .register_routed_provider("agent-a", "delete-a-1")
        .expect("a1");
    let second = registry
        .register_routed_provider("agent-a", "delete-a-2")
        .expect("a2");
    let other = registry
        .register_routed_provider("agent-b", "delete-b")
        .expect("b");

    assert_eq!(
        registry.agent_id_for_stream("delete-a-1").as_deref(),
        Some("agent-a")
    );
    assert_eq!(registry.agent_id_for_stream("missing"), None);
    assert_eq!(registry.cancel_agent("agent-a", "agent deleted"), 2);
    assert!(first.is_cancelled());
    assert!(second.is_cancelled());
    assert_eq!(first.reason().as_deref(), Some("agent deleted"));
    assert_eq!(second.reason().as_deref(), Some("agent deleted"));
    assert!(!other.is_cancelled());

    registry.finish_routed_provider("delete-a-1");
    assert_eq!(registry.agent_id_for_stream("delete-a-1"), None);
    registry.finish_routed_provider("delete-a-2");
    registry.finish_routed_provider("delete-b");
}

#[test]
fn production_routed_provider_checkpoint_store_persists_durable_json() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "fabushi-runner-checkpoint-{}-{nonce}",
        std::process::id()
    ));
    let store = ProductionRoutedProviderCheckpointStore::new(
        &root,
        "agent-private-id",
        "stream-private-id",
    );
    let directory = store.directory().to_string_lossy();
    assert!(!directory.contains("agent-private-id"));
    assert!(!directory.contains("stream-private-id"));

    let checkpoint = RoutedProviderCheckpoint::OpenRouter(OpenRouterCheckpoint {
        conversation: vec![
            json!({"role":"user","content":"calendar"}),
            json!({"role":"tool","tool_call_id":"call-1","content":"daily"}),
        ],
        text: "Checking ".into(),
        completed_steps: 1,
        tool_calls_completed: 1,
    });
    let cursor = store.persist(&checkpoint).expect("persist checkpoint");
    let cursor_path = Path::new(&cursor);
    assert!(cursor_path.starts_with(store.directory()));
    assert_eq!(
        cursor_path.extension().and_then(|value| value.to_str()),
        Some("json")
    );

    let decoded: RoutedProviderCheckpoint =
        serde_json::from_slice(&fs::read(cursor_path).expect("read persisted checkpoint"))
            .expect("decode persisted checkpoint");
    assert_eq!(decoded, checkpoint);

    fs::remove_dir_all(&root).expect("remove checkpoint fixture");
}

#[test]
fn transcript_runner_registry_watchdog_interrupts_only_current_direct_and_group_runs() {
    let tasks = Arc::new(RoutedProviderTaskRegistry::default());
    let registry = TranscriptRunnerRegistry::new(Arc::clone(&tasks));
    let stale_direct = registry
        .register_routed_provider("agent-a", "stream-a-stale")
        .expect("stale direct stream");
    let current_direct = registry
        .register_routed_provider("agent-a", "stream-a-current")
        .expect("current direct stream");
    let stale_group = registry
        .register_group_member("agent-a", "group-a-stale")
        .expect("stale group stream");
    let current_group = registry
        .register_group_member("agent-a", "group-a-current")
        .expect("current group stream");
    let other = registry
        .register_routed_provider("agent-b", "stream-b")
        .expect("other stream");

    assert_eq!(
        registry
            .current_routed_stream_id_for_agent("agent-a")
            .as_deref(),
        Some("stream-a-current")
    );
    assert_eq!(
        registry
            .current_group_member_stream_id_for_agent("agent-a")
            .as_deref(),
        Some("group-a-current")
    );

    assert!(registry.interrupt_wedged_run_for_watchdog("agent-a"));
    assert!(!stale_direct.is_cancelled());
    assert!(current_direct.is_cancelled());
    assert_eq!(
        current_direct.reason().as_deref(),
        Some(RUN_WATCHDOG_INTERRUPT_REASON)
    );
    assert!(!stale_group.is_cancelled());
    assert!(current_group.is_cancelled());
    assert_eq!(
        current_group.reason().as_deref(),
        Some(RUN_WATCHDOG_INTERRUPT_REASON)
    );
    assert!(!other.is_cancelled());
    assert!(!registry.interrupt_wedged_run_for_watchdog("missing"));

    for stream_id in [
        "stream-a-stale",
        "stream-a-current",
        "group-a-stale",
        "group-a-current",
        "stream-b",
    ] {
        registry.finish_routed_provider(stream_id);
    }
    assert_eq!(registry.active_count(), 0);
}

#[test]
fn transcript_runner_registry_preserves_frozen_pre_dispatch_supersede_rules() {
    let tasks = Arc::new(RoutedProviderTaskRegistry::default());
    let registry = TranscriptRunnerRegistry::new(Arc::clone(&tasks));

    let ordinary = registry
        .register_routed_provider_with_recovery_shape("agent-a", "ordinary", false)
        .expect("ordinary");
    assert_eq!(
        registry.preempt_routed_agent_for_supersede(
            "agent-a",
            RUN_DIRECT_USER_INTERRUPT_REASON,
            Some(false),
        ),
        0,
        "an undispatched ordinary run must survive a non-recovery supersede",
    );
    assert!(!ordinary.is_cancelled());
    assert_eq!(
        registry.preempt_routed_agent_for_supersede(
            "agent-a",
            RUN_DIRECT_USER_INTERRUPT_REASON,
            Some(true),
        ),
        0,
        "an undispatched ordinary run must survive even when the new turn carries recovery",
    );
    assert!(registry.mark_routed_provider_dispatched("ordinary"));
    assert_eq!(
        registry.preempt_routed_agent_for_supersede(
            "agent-a",
            RUN_DIRECT_USER_INTERRUPT_REASON,
            Some(false),
        ),
        1,
        "once dispatched, a direct user turn supersedes the active run",
    );
    assert!(ordinary.is_cancelled());
    registry.finish_routed_provider("ordinary");

    let recovery = registry
        .register_routed_provider_with_recovery_shape("agent-a", "recovery", true)
        .expect("recovery");
    assert_eq!(
        registry.preempt_routed_agent_for_supersede(
            "agent-a",
            RUN_DIRECT_USER_INTERRUPT_REASON,
            Some(true),
        ),
        1,
        "an undispatched recovery-shaped run may be superseded only by a recovery-carrying turn",
    );
    assert!(recovery.is_cancelled());
    registry.finish_routed_provider("recovery");
}

#[test]
fn transcript_runner_registry_preempts_only_current_one_to_one_run_for_direct_user_turn() {
    let tasks = Arc::new(RoutedProviderTaskRegistry::default());
    let registry = TranscriptRunnerRegistry::new(Arc::clone(&tasks));
    let stale = registry
        .register_routed_provider("agent-a", "direct-stale")
        .expect("stale direct stream");
    let current = registry
        .register_routed_provider("agent-a", "direct-current")
        .expect("current direct stream");

    registry.finish_routed_provider("direct-stale");
    assert_eq!(
        registry
            .current_routed_stream_id_for_agent("agent-a")
            .as_deref(),
        Some("direct-current")
    );
    assert_eq!(
        registry.preempt_routed_agent("agent-a", RUN_DIRECT_USER_INTERRUPT_REASON),
        1
    );
    assert!(!stale.is_cancelled());
    assert!(current.is_cancelled());
    assert_eq!(
        current.reason().as_deref(),
        Some(RUN_DIRECT_USER_INTERRUPT_REASON)
    );
    assert_eq!(
        registry.preempt_routed_agent("missing", RUN_DIRECT_USER_INTERRUPT_REASON),
        0
    );

    registry.finish_routed_provider("direct-current");
    assert_eq!(registry.current_routed_stream_id_for_agent("agent-a"), None);
}

#[test]
fn transcript_runner_registry_preempts_only_current_group_member_and_preserves_new_identity() {
    let tasks = Arc::new(RoutedProviderTaskRegistry::default());
    let registry = TranscriptRunnerRegistry::new(tasks);
    let stale = registry
        .register_group_member("agent-a", "group-stale")
        .expect("stale group stream");
    let current = registry
        .register_group_member("agent-a", "group-current")
        .expect("current group stream");

    registry.finish_routed_provider("group-stale");
    assert_eq!(
        registry
            .current_group_member_stream_id_for_agent("agent-a")
            .as_deref(),
        Some("group-current")
    );
    assert_eq!(
        registry.preempt_group_member_agent("agent-a", "direct user message"),
        1
    );
    assert!(!stale.is_cancelled());
    assert!(current.is_cancelled());
    assert!(registry.take_group_member_preempted("agent-a"));
    assert!(!registry.take_group_member_preempted("agent-a"));

    let next = registry
        .register_group_member("agent-a", "group-next")
        .expect("next group stream");
    registry.finish_routed_provider("group-current");
    assert_eq!(
        registry
            .current_group_member_stream_id_for_agent("agent-a")
            .as_deref(),
        Some("group-next")
    );
    assert_eq!(
        registry.preempt_group_member_agent("agent-a", "second direct user message"),
        1
    );
    assert!(next.is_cancelled());

    registry.finish_routed_provider("group-next");
    assert_eq!(
        registry.current_group_member_stream_id_for_agent("agent-a"),
        None
    );
}
