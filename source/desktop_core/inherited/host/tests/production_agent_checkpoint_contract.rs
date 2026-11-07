use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderMessage, ProviderSessionError, RoutedProvider, RoutedProviderCheckpoint,
    RoutedToolDefinition,
};
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::host_request_context::HostRequestContext;
use mahayana_host_runtime::runner::production_agent_checkpoint::{
    AgentStateCheckpointSink, ProductionAgentStateCheckpointSink,
    build_generated_turn_checkpoint_with_rich_text, build_text_turn_checkpoint,
    build_text_turn_checkpoint_with_rich_text, prepare_settled_checkpoint_state,
};
use mahayana_host_runtime::runner::production_turn_agent_owner::ProductionTurnAgentOwner;
use mahayana_host_runtime::runner::production_turn_run_shell_adapter::RoutedProviderCheckpointStore;
use mahayana_host_runtime::runner::routed_provider_runtime::{
    RoutedProviderCancellation, RoutedToolBridge, RunnerRequestContextSnapshot,
};
use mahayana_host_runtime::runner::sand_agent_runner::SandAgentRunner;
use mahayana_host_runtime::runner::turn_agent_composition::TurnAgentComposition;
use mahayana_host_runtime::runner::{TerminalOutcome, TokenDetailsPersistenceTracker, TurnRunOptions};
use mahayana_host_runtime::transcript_mirror::conversation_state_binary::{
    decode_conversation_state_recovery_fields, decode_transcript_mirror_conversation_state,
};
use mahayana_host_runtime::transcript_mirror::generated_occurrence_codec::{
    GeneratedTranscriptOccurrenceCodec, RejectGeneratedToolJsonProjection,
};
use mahayana_host_runtime::transcript_mirror::production_provider::{
    ProductionTranscriptMirrorProvider,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-production-agent-checkpoint-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn user_messages() -> Vec<ProviderMessage> {
    vec![ProviderMessage {
        role: "user".into(),
        content: "hello checkpoint".into(),
    }]
}

#[test]
fn canonical_text_turn_checkpoint_preserves_prior_wire_and_appends_turn_reference() {
    let prior = vec![0x9a, 0x06, 0x03, b'o', b'l', b'd'];
    let checkpoint = build_text_turn_checkpoint(
        &prior,
        "hello",
        "message-1",
        Some("request-1"),
        "world",
    );
    assert_eq!(&checkpoint.state_bytes[..prior.len()], prior.as_slice());
    let decoded = decode_transcript_mirror_conversation_state(&checkpoint.state_bytes)
        .expect("decode state");
    assert_eq!(decoded.turns, vec![checkpoint.turn_id.clone()]);
    assert!(!checkpoint.user_message_id.is_empty());
    assert_eq!(checkpoint.step_ids.len(), 1);

    let rich = build_text_turn_checkpoint_with_rich_text(
        &prior,
        "hello",
        Some("{\"type\":\"doc\",\"content\":[]}"),
        "message-rich",
        Some("request-rich"),
        "world",
    );
    let rich_wire = b"{\"type\":\"doc\",\"content\":[]}";
    assert!(
        rich.user_message_bytes
            .windows(rich_wire.len())
            .any(|window| window == rich_wire)
    );
    assert_ne!(rich.user_message_id, checkpoint.user_message_id);
}

#[test]
fn generated_turn_checkpoint_hashes_and_references_tool_steps_before_assistant_text() {
    let tool_step = vec![0x12, 0x02, 0x0a, 0x00];
    let checkpoint = build_generated_turn_checkpoint_with_rich_text(
        &[],
        "use a tool",
        None,
        "message-tool",
        Some("request-tool"),
        std::slice::from_ref(&tool_step),
        "done",
    );
    assert_eq!(checkpoint.step_bytes.len(), 2);
    assert_eq!(checkpoint.step_bytes[0], tool_step);
    assert_eq!(
        checkpoint.step_ids[0],
        Sha256::digest(&checkpoint.step_bytes[0]).to_vec()
    );
    assert_eq!(
        checkpoint.step_ids[1],
        Sha256::digest(&checkpoint.step_bytes[1]).to_vec()
    );

    // AgentConversationTurnStructure field 2 contains the ordered blob ids.
    let first = checkpoint
        .turn_bytes
        .windows(checkpoint.step_ids[0].len())
        .position(|window| window == checkpoint.step_ids[0].as_slice())
        .expect("tool step id is referenced");
    let second = checkpoint
        .turn_bytes
        .windows(checkpoint.step_ids[1].len())
        .position(|window| window == checkpoint.step_ids[1].as_slice())
        .expect("assistant step id is referenced");
    assert!(first < second, "tool step must precede terminal assistant text");
}

#[test]
fn production_sink_commits_real_agent_wire_through_mirror_and_agent_store() {
    let root = temp_root("production");
    let agents_root = root.join("agents");
    let transcripts_dir = root.join("transcripts");
    let sessions = ProductionSessionWorkers::with_agents_root(&agents_root, 500);
    let session = sessions
        .materialize_session_with_active(None, "user", None, None)
        .expect("session");
    let blob_store = Arc::new(
        sessions
            .create_agent_blob_store(&session.record.id)
            .expect("blob store"),
    );
    session
        .db
        .append_transcript_entry(&serde_json::json!({
            "id": "message-1",
            "kind": "message",
            "role": "user",
            "content": "hello checkpoint",
            "confirmed": false
        }))
        .expect("durable user echo");
    let prior = session.agent_store.latest_checkpoint_bytes().unwrap_or_default();
    let provider = ProductionTranscriptMirrorProvider::new(
        &transcripts_dir,
        GeneratedTranscriptOccurrenceCodec::new(
            RejectGeneratedToolJsonProjection,
        ),
    );
    let mirror = Arc::new(
        provider
            .route_for_session(
                Arc::clone(&blob_store),
                &prior,
                Arc::new(|| Ok(true)),
            )
            .expect("route"),
    );
    let sink = ProductionAgentStateCheckpointSink::new(
        session.record.id.clone(),
        Arc::clone(&session.agent_store),
        Arc::clone(&blob_store),
        mirror,
        prior,
        true,
    )
    .expect("sink");
    let fixture_blob = b"canonical generated Agent state blob";
    let fixture_blob_id = Sha256::digest(fixture_blob).to_vec();
    blob_store
        .set_blob_blocking(&fixture_blob_id, fixture_blob)
        .expect("fixture blob");
    assert_eq!(
        sink.read_state_blob(&fixture_blob_id)
            .expect("canonical blob read")
            .as_deref(),
        Some(fixture_blob.as_slice())
    );
    sink.checkpoint_text_turn(
        &user_messages(),
        &TurnRunOptions {
            inference_request_id: Some("request-1".into()),
            message_id: Some("message-1".into()),
            recent_message_text: Some("hello checkpoint".into()),
            turn_started_at_ms: Some(1),
            ..TurnRunOptions::default()
        },
        "assistant checkpoint",
    )
    .expect("checkpoint");

    let state = session
        .agent_store
        .latest_checkpoint_bytes()
        .expect("latest checkpoint");
    let decoded =
        decode_transcript_mirror_conversation_state(&state).expect("decoded state");
    assert_eq!(decoded.turns.len(), 1);
    let recovery =
        decode_conversation_state_recovery_fields(&state).expect("decoded recovery state");
    assert_eq!(recovery.turn_timings.len(), 1);
    assert!(recovery.turn_timings[0].timestamp_ms > 1);
    assert_eq!(
        recovery.turn_timings[0].duration_ms,
        recovery.turn_timings[0].timestamp_ms.saturating_sub(1),
    );
    assert!(!session.agent_store.latest_root_blob_id().is_empty());
    assert_eq!(
        session
            .db
            .get_transcript_entry_by_id("message-1")
            .expect("read user echo")
            .and_then(|entry| entry.get("confirmed").and_then(serde_json::Value::as_bool)),
        Some(true)
    );

    let jsonl = fs::read_to_string(
        transcripts_dir
            .join(&session.record.id)
            .join(format!("{}.jsonl", session.record.id)),
    )
    .expect("journal transcript");
    assert!(jsonl.contains("hello checkpoint"));
    assert!(jsonl.contains("assistant checkpoint"));

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn missing_confirmed_message_aborts_root_advance() {
    let root = temp_root("missing-confirm");
    let agents_root = root.join("agents");
    let transcripts_dir = root.join("transcripts");
    let sessions = ProductionSessionWorkers::with_agents_root(&agents_root, 500);
    let session = sessions
        .materialize_session_with_active(None, "user", None, None)
        .expect("session");
    let blob_store = Arc::new(
        sessions
            .create_agent_blob_store(&session.record.id)
            .expect("blob store"),
    );
    let prior = session.agent_store.latest_checkpoint_bytes().unwrap_or_default();
    let prior_root = session.agent_store.latest_root_blob_id();
    let provider = ProductionTranscriptMirrorProvider::new(
        &transcripts_dir,
        GeneratedTranscriptOccurrenceCodec::new(
            RejectGeneratedToolJsonProjection,
        ),
    );
    let mirror = Arc::new(
        provider
            .route_for_session(
                Arc::clone(&blob_store),
                &prior,
                Arc::new(|| Ok(true)),
            )
            .expect("route"),
    );
    let sink = ProductionAgentStateCheckpointSink::new(
        session.record.id.clone(),
        Arc::clone(&session.agent_store),
        blob_store,
        mirror,
        prior,
        true,
    )
    .expect("sink");
    let error = sink
        .checkpoint_text_turn(
            &user_messages(),
            &TurnRunOptions {
                inference_request_id: Some("request-missing".into()),
                message_id: Some("missing-message".into()),
                recent_message_text: Some("hello checkpoint".into()),
                ..TurnRunOptions::default()
            },
            "assistant checkpoint",
        )
        .expect_err("missing user echo must reject checkpoint");
    assert!(error.to_string().contains("were not committed atomically"));
    assert_eq!(session.agent_store.latest_root_blob_id(), prior_root);

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}

struct EmptyBridge;

impl RoutedToolBridge for EmptyBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(Vec::new())
    }

    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Err(ProviderSessionError::Tool("no tools".into()))
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

struct FailingAgentCheckpointSink {
    calls: Mutex<usize>,
}

impl AgentStateCheckpointSink for FailingAgentCheckpointSink {
    fn checkpoint_text_turn(
        &self,
        _messages: &[ProviderMessage],
        _options: &TurnRunOptions,
        _assistant_content: &str,
    ) -> Result<(), ProviderSessionError> {
        *self.calls.lock().expect("calls") += 1;
        Err(ProviderSessionError::Protocol(
            "durable Agent checkpoint failed".into(),
        ))
    }
}

#[test]
fn owner_does_not_settle_completed_before_agent_checkpoint_succeeds() {
    let composition = TurnAgentComposition::new(
        RoutedProvider::OpenRouter,
        Arc::new(EmptyBridge),
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
    let sink = Arc::new(FailingAgentCheckpointSink {
        calls: Mutex::new(0),
    });
    let owner = ProductionTurnAgentOwner::new(composition)
        .with_agent_state_checkpoint_sink(sink.clone());
    let mut runner = SandAgentRunner::new(owner);
    let error = runner
        .run_with_options(
            &user_messages(),
            TurnRunOptions {
                inference_request_id: Some("request-1".into()),
                message_id: Some("message-1".into()),
                ..TurnRunOptions::default()
            },
            || Ok("provider completed".into()),
        )
        .expect_err("checkpoint failure must fail the turn");
    assert!(error.to_string().contains("durable Agent checkpoint failed"));
    assert_eq!(*sink.calls.lock().expect("calls"), 1);
    assert!(matches!(
        runner.last_finished().map(|finished| &finished.outcome),
        Some(TerminalOutcome::Failed {
            retryable: false,
            message,
        }) if message.contains("durable Agent checkpoint failed")
    ));
}


#[test]
fn production_sink_can_stage_generated_state_before_outer_stream_final_persist() {
    let root = temp_root("staged-generated-state");
    let agents_root = root.join("agents");
    let transcripts_dir = root.join("transcripts");
    let sessions = ProductionSessionWorkers::with_agents_root(&agents_root, 500);
    let session = sessions
        .materialize_session_with_active(None, "user", None, None)
        .expect("session");
    session
        .db
        .append_transcript_entry(&serde_json::json!({
            "id": "message-staged",
            "kind": "message",
            "role": "user",
            "content": "stage me",
            "confirmed": false
        }))
        .expect("user echo");
    let blob_store = Arc::new(
        sessions
            .create_agent_blob_store(&session.record.id)
            .expect("blob store"),
    );
    let prior = session.agent_store.latest_checkpoint_bytes().unwrap_or_default();
    let provider = ProductionTranscriptMirrorProvider::new(
        &transcripts_dir,
        GeneratedTranscriptOccurrenceCodec::new(
            RejectGeneratedToolJsonProjection,
        ),
    );
    let mirror = Arc::new(
        provider
            .route_for_session(
                Arc::clone(&blob_store),
                &prior,
                Arc::new(|| Ok(true)),
            )
            .expect("route"),
    );
    let sink = ProductionAgentStateCheckpointSink::new(
        session.record.id.clone(),
        Arc::clone(&session.agent_store),
        blob_store,
        mirror,
        prior.clone(),
        true,
    )
    .expect("sink");
    let options = TurnRunOptions {
        inference_request_id: Some("request-staged".into()),
        message_id: Some("message-staged".into()),
        recent_message_text: Some("stage me".into()),
        ..TurnRunOptions::default()
    };
    let staged = sink
        .stage_text_turn(&[ProviderMessage {
            role: "user".into(),
            content: "stage me".into(),
        }], &options, "assistant staged")
        .expect("stage generated state");
    assert_eq!(session.agent_store.latest_checkpoint_bytes().unwrap_or_default(), prior);
    assert_ne!(staged.state_bytes, prior);

    sink.persist_staged_text_turn(&options, &staged)
        .expect("persist staged generated state");
    assert_eq!(
        session.agent_store.latest_checkpoint_bytes().expect("final state"),
        staged.state_bytes
    );

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn production_sink_can_checkpoint_inside_existing_localpool_executor() {
    let root = temp_root("nested-localpool");
    let agents_root = root.join("agents");
    let transcripts_dir = root.join("transcripts");
    let sessions = ProductionSessionWorkers::with_agents_root(&agents_root, 500);
    let session = sessions
        .materialize_session_with_active(None, "user", None, None)
        .expect("session");
    session
        .db
        .append_transcript_entry(&serde_json::json!({
            "id": "message-nested",
            "kind": "message",
            "role": "user",
            "content": "nested checkpoint",
            "confirmed": false
        }))
        .expect("user echo");
    let blob_store = Arc::new(
        sessions
            .create_agent_blob_store(&session.record.id)
            .expect("blob store"),
    );
    let prior = session.agent_store.latest_checkpoint_bytes().unwrap_or_default();
    let provider = ProductionTranscriptMirrorProvider::new(
        &transcripts_dir,
        GeneratedTranscriptOccurrenceCodec::new(
            RejectGeneratedToolJsonProjection,
        ),
    );
    let mirror = Arc::new(
        provider
            .route_for_session(
                Arc::clone(&blob_store),
                &prior,
                Arc::new(|| Ok(true)),
            )
            .expect("route"),
    );
    let sink = ProductionAgentStateCheckpointSink::new(
        session.record.id.clone(),
        Arc::clone(&session.agent_store),
        blob_store,
        mirror,
        prior,
        true,
    )
    .expect("sink");

    futures::executor::block_on(async {
        sink.checkpoint_text_turn(
            &[ProviderMessage {
                role: "user".into(),
                content: "nested checkpoint".into(),
            }],
            &TurnRunOptions {
                inference_request_id: Some("request-nested".into()),
                message_id: Some("message-nested".into()),
                recent_message_text: Some("nested checkpoint".into()),
                ..TurnRunOptions::default()
            },
            "assistant nested",
        )
        .expect("checkpoint inside existing LocalPool");
    });

    assert!(!session.agent_store.latest_root_blob_id().is_empty());
    assert_eq!(
        session
            .db
            .get_transcript_entry_by_id("message-nested")
            .expect("read user echo")
            .and_then(|entry| entry.get("confirmed").and_then(serde_json::Value::as_bool)),
        Some(true)
    );

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn settled_checkpoint_suppresses_compaction_stale_tokens_and_appends_timing() {
    // ConversationStateStructure:
    //   summary_archives (13) = [0xaa]
    //   token_details (5) = { used_tokens: 120, max_tokens: 1000 }
    let state = [
        0x6a, 0x01, 0xaa,
        0x2a, 0x05, 0x08, 0x78, 0x10, 0xe8, 0x07,
    ];
    let mut tracker = TokenDetailsPersistenceTracker::new(0);
    let settled = prepare_settled_checkpoint_state(
        &state,
        &mut tracker,
        Some(100),
        175,
    )
    .expect("settled state");
    let decoded =
        decode_conversation_state_recovery_fields(&settled).expect("decode settled state");
    assert!(decoded.token_details.is_none());
    assert_eq!(decoded.summary_archives, vec![vec![0xaa]]);
    assert_eq!(decoded.turn_timings.len(), 1);
    assert_eq!(decoded.turn_timings[0].duration_ms, 75);
    assert_eq!(decoded.turn_timings[0].timestamp_ms, 175);
}
