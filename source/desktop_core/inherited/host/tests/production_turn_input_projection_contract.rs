use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderMessage, ProviderSessionError, RoutedProvider, RoutedProviderCheckpoint,
    RoutedToolDefinition,
};
use mahayana_host_runtime::host_request_context::HostRequestContext;
use mahayana_host_runtime::runner::production_agent_checkpoint::AgentStateCheckpointSink;
use mahayana_host_runtime::runner::production_turn_input_projection::{
    ProductionTurnInputProjection, create_production_turn_agent_input_projection,
    create_production_turn_input_projection,
};
use mahayana_host_runtime::runner::production_turn_run_shell_adapter::RoutedProviderCheckpointStore;
use mahayana_host_runtime::runner::routed_provider_runtime::{
    RoutedProviderCancellation, RoutedToolBridge, RunnerRequestContextSnapshot,
};
use mahayana_host_runtime::runner::turn_agent_composition::TurnAgentComposition;
use mahayana_host_runtime::runner::TurnRunOptions;
use serde_json::Value;

#[test]
fn production_turn_projection_binds_stream_identity_and_recovery_shape() {
    let args = serde_json::json!({
        "messageId": "msg-1",
        "recentUserMessages": [
            {"id":"msg-0","text":"older"},
            {"id":"msg-1","text":"hello from durable history"}
        ],
        "isFork": true,
        "attachmentPaths": ["/tmp/a", "/tmp/b"],
        "selectedImages": [{
            "uuid":"image",
            "path":"/tmp/image.png",
            "mimeType":"image/png",
            "data":[1,2,3]
        }],
        "selectedVideos": [{
            "uuid":"video",
            "path":"/tmp/video.mp4",
            "mimeType":"video/mp4",
            "filename":"video.mp4",
            "data":[0,0,0,0,102,116,121,112]
        }],
        "replyContext": {"id":"reply"}
    });
    let messages = vec![ProviderMessage {
        role: "user".into(),
        content: "hello from the real turn".into(),
    }];

    let projected = create_production_turn_input_projection(
        &args,
        "stream-request-123",
        &messages,
    )
    .expect("projection");

    assert_eq!(
        projected.options.inference_request_id.as_deref(),
        Some("stream-request-123")
    );
    assert_eq!(projected.options.message_id.as_deref(), Some("msg-1"));
    assert_eq!(
        projected.options.recent_message_text.as_deref(),
        Some("hello from durable history")
    );
    assert_eq!(
        projected
            .options
            .recent_user_messages
            .iter()
            .map(|message| (message.id.as_str(), message.text.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("msg-0", "older"),
            ("msg-1", "hello from durable history")
        ]
    );
    assert!(projected.options.is_fork);
    assert_eq!(projected.options.attachment_count, 2);
    assert_eq!(projected.options.image_count, 1);
    assert_eq!(projected.options.video_count, 1);
    assert!(projected.options.has_reply_context);
    assert_eq!(projected.prompt_action.message_id.as_deref(), Some("msg-1"));
    assert_eq!(
        projected.prompt_action.selected_context.selected_images[0]
            .data
            .as_deref(),
        Some(&[1, 2, 3][..])
    );
    assert_eq!(
        projected.prompt_action.selected_context.selected_videos[0]
            .filename
            .as_deref(),
        Some("video.mp4")
    );
}

#[test]
fn production_turn_projection_uses_latest_user_fallback_without_durable_history() {
    let messages = vec![ProviderMessage {
        role: "user".into(),
        content: "fallback user text".into(),
    }];
    let projected = create_production_turn_input_projection(
        &serde_json::json!({"messageId":"msg-missing"}),
        "stream-fallback",
        &messages,
    )
    .expect("projection");
    assert!(projected.options.recent_user_messages.is_empty());
    assert_eq!(
        projected.options.recent_message_text.as_deref(),
        Some("fallback user text")
    );
    assert!(!projected.options.is_fork);
}

#[test]
fn production_turn_projection_rejects_missing_request_identity() {
    let messages = vec![ProviderMessage {
        role: "user".into(),
        content: "hello".into(),
    }];
    assert!(
        create_production_turn_input_projection(
            &serde_json::json!({}),
            "   ",
            &messages,
        )
        .is_err()
    );
}


struct CountingBridge {
    list_calls: Arc<AtomicUsize>,
}

impl RoutedToolBridge for CountingBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![RoutedToolDefinition {
            name: "mcp__demo".into(),
            provider_identifier: "demo".into(),
            tool_name: "demo".into(),
            description: Some("demo tool".into()),
            input_schema: serde_json::json!({"type":"object"}),
        }])
    }

    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Ok(Value::Null)
    }
}

struct MemoryProviderCheckpointStore;

impl RoutedProviderCheckpointStore for MemoryProviderCheckpointStore {
    fn persist(
        &self,
        _checkpoint: &RoutedProviderCheckpoint,
    ) -> Result<String, ProviderSessionError> {
        Ok("provider-checkpoint".into())
    }
}

struct SnapshotCheckpointSink {
    base: Mutex<Vec<u8>>,
}

impl AgentStateCheckpointSink for SnapshotCheckpointSink {
    fn base_state_bytes(&self) -> Result<Vec<u8>, ProviderSessionError> {
        Ok(self.base.lock().expect("base state lock").clone())
    }
}

#[test]
fn production_turn_agent_projection_freezes_action_mcp_ack_and_base_state() {
    let list_calls = Arc::new(AtomicUsize::new(0));
    let composition = TurnAgentComposition::new(
        RoutedProvider::OpenRouter,
        Arc::new(CountingBridge {
            list_calls: Arc::clone(&list_calls),
        }),
        RunnerRequestContextSnapshot {
            context: HostRequestContext {
                os_version: "test".into(),
                shell: None,
                time_zone: Some("UTC".into()),
                transcripts_folder: "/tmp/transcripts".into(),
                user_full_name: None,
            },
            rules: None,
        },
        RoutedProviderCancellation::default(),
        Arc::new(MemoryProviderCheckpointStore),
    );
    let lifecycle_messages = vec![ProviderMessage {
        role: "user".into(),
        content: "hello".into(),
    }];
    let provider_messages = vec![
        ProviderMessage {
            role: "system".into(),
            content: "system".into(),
        },
        lifecycle_messages[0].clone(),
    ];
    let checkpoint_sink: Arc<dyn AgentStateCheckpointSink> = Arc::new(
        SnapshotCheckpointSink {
            base: Mutex::new(vec![1, 2, 3, 4]),
        },
    );
    let mut output = Vec::<String>::new();
    let mut emit_update = |delta: &str, _accumulated: &str| output.push(delta.to_string());
    let turn_input = ProductionTurnInputProjection {
        options: TurnRunOptions::default(),
        ack_token: Some("ack-1".into()),
        prompt_action: Default::default(),
        typed_replay: None,
    };

    let projected = create_production_turn_agent_input_projection(
        &composition,
        Some(&checkpoint_sink),
        &lifecycle_messages,
        &provider_messages,
        &turn_input,
        &mut emit_update,
    )
    .expect("production Agent input projection");

    assert_eq!(projected.action.lifecycle_messages, lifecycle_messages);
    assert_eq!(projected.action.provider_messages, provider_messages);
    assert_eq!(projected.action.prompt_action, turn_input.prompt_action);
    assert_eq!(projected.mcp_tools.len(), 1);
    assert_eq!(projected.mcp_tools[0].name, "mcp__demo");
    assert_eq!(projected.base_state_bytes, vec![1, 2, 3, 4]);
    assert_eq!(projected.ack_token.as_deref(), Some("ack-1"));
    assert_eq!(list_calls.load(Ordering::SeqCst), 1);
    assert!(!projected.cancel_this_run.is_cancelled());
    (projected.emit_update)("delta", "delta");
    drop(projected);
    assert_eq!(output, vec!["delta"]);

    let frozen = composition
        .with_projected_mcp_tools(vec![RoutedToolDefinition {
            name: "mcp__frozen".into(),
            provider_identifier: "demo".into(),
            tool_name: "frozen".into(),
            description: None,
            input_schema: serde_json::json!({"type":"object"}),
        }])
        .snapshot_mcp_tools()
        .expect("frozen MCP snapshot");
    assert_eq!(frozen[0].name, "mcp__frozen");
    assert_eq!(list_calls.load(Ordering::SeqCst), 1);
}


struct FailingDiscoveryBridge {
    list_calls: Arc<AtomicUsize>,
}

impl RoutedToolBridge for FailingDiscoveryBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        Err(ProviderSessionError::Tool("discovery unavailable".into()))
    }

    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Ok(Value::Null)
    }
}

#[test]
fn production_turn_agent_projection_degrades_failed_mcp_discovery_to_empty_snapshot() {
    let list_calls = Arc::new(AtomicUsize::new(0));
    let composition = TurnAgentComposition::new(
        RoutedProvider::OpenRouter,
        Arc::new(FailingDiscoveryBridge {
            list_calls: Arc::clone(&list_calls),
        }),
        RunnerRequestContextSnapshot {
            context: HostRequestContext {
                os_version: "test".into(),
                shell: None,
                time_zone: Some("UTC".into()),
                transcripts_folder: "/tmp/transcripts".into(),
                user_full_name: None,
            },
            rules: None,
        },
        RoutedProviderCancellation::default(),
        Arc::new(MemoryProviderCheckpointStore),
    );
    let lifecycle_messages = vec![ProviderMessage {
        role: "user".into(),
        content: "continue without MCP".into(),
    }];
    let provider_messages = lifecycle_messages.clone();
    let checkpoint_sink: Arc<dyn AgentStateCheckpointSink> = Arc::new(
        SnapshotCheckpointSink {
            base: Mutex::new(vec![9, 8, 7]),
        },
    );
    let turn_input = ProductionTurnInputProjection {
        options: TurnRunOptions::default(),
        ack_token: Some("ack-fallback".into()),
        prompt_action: Default::default(),
        typed_replay: None,
    };
    let mut emit_update = |_delta: &str, _accumulated: &str| {};

    let projected = create_production_turn_agent_input_projection(
        &composition,
        Some(&checkpoint_sink),
        &lifecycle_messages,
        &provider_messages,
        &turn_input,
        &mut emit_update,
    )
    .expect("MCP discovery failure must not abort the turn projection");

    assert_eq!(list_calls.load(Ordering::SeqCst), 1);
    assert!(projected.mcp_tools.is_empty());
    assert_eq!(projected.action.lifecycle_messages, lifecycle_messages);
    assert_eq!(projected.action.provider_messages, provider_messages);
    assert_eq!(projected.base_state_bytes, vec![9, 8, 7]);
    assert_eq!(projected.ack_token.as_deref(), Some("ack-fallback"));
    assert!(!projected.cancel_this_run.is_cancelled());
}
