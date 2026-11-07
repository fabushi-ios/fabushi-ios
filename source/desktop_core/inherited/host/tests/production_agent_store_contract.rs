use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::session::production_agent_store::{
    ProductionAgentMetadataKey, ProductionAgentStore,
};
use sha2::{Digest, Sha256};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-production-agent-store-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn encode_varint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value as u8) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn push_bytes(field: u64, value: &[u8], output: &mut Vec<u8>) {
    encode_varint((field << 3) | 2, output);
    encode_varint(value.len() as u64, output);
    output.extend_from_slice(value);
}

fn push_string(field: u64, value: &str, output: &mut Vec<u8>) {
    push_bytes(field, value.as_bytes(), output);
}

fn push_varint(field: u64, value: u64, output: &mut Vec<u8>) {
    encode_varint(field << 3, output);
    encode_varint(value, output);
}

fn subagent_state_bytes(created: u64, last_used: u64) -> Vec<u8> {
    let mut state = Vec::new();
    push_bytes(1, &[], &mut state);
    push_varint(2, created, &mut state);
    push_varint(3, last_used, &mut state);
    push_bytes(4, b"default", &mut state);
    state
}

fn map_entry(key: &str, value: &[u8]) -> Vec<u8> {
    let mut entry = Vec::new();
    push_string(1, key, &mut entry);
    push_bytes(2, value, &mut entry);
    entry
}

#[test]
fn production_agent_store_persists_content_addressed_checkpoint_and_resets_from_db() {
    let root = temp_root("checkpoint");
    let workers = ProductionSessionWorkers::with_agents_root(&root, 500);
    let session = workers
        .materialize_session_with_active(None, "user", None, None)
        .expect("materialized session");

    assert!(session.agent_store.latest_root_blob_id().is_empty());
    assert_eq!(session.agent_store.latest_checkpoint_bytes(), None);

    // Empty bytes are the canonical protobuf encoding of an empty
    // ConversationStateStructure and generated fromBinary accepts them.
    let checkpoint = Vec::<u8>::new();
    let expected_root = Sha256::digest(&checkpoint).to_vec();
    let root_blob_id = session
        .agent_store
        .handle_checkpoint_bytes(&checkpoint)
        .expect("checkpoint");
    assert_eq!(root_blob_id, expected_root);
    assert_eq!(
        session.db.get_latest_root_blob_id().expect("metadata root"),
        expected_root
    );
    assert_eq!(
        session.agent_store.get_blob(&expected_root).expect("root blob"),
        Some(checkpoint.clone())
    );
    assert_eq!(
        session.agent_store.latest_checkpoint_bytes(),
        Some(checkpoint.clone())
    );

    let reopened = ProductionAgentStore::new(
        Arc::clone(&session.db),
        workers
            .create_agent_blob_store(&session.record.id)
            .expect("second blob store"),
    );
    assert!(reopened.try_reset_from_db());
    assert_eq!(reopened.latest_root_blob_id(), expected_root);
    assert_eq!(reopened.latest_checkpoint_bytes(), Some(checkpoint));

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn production_agent_store_rejects_malformed_checkpoint_without_advancing_metadata() {
    let root = temp_root("malformed");
    let workers = ProductionSessionWorkers::with_agents_root(&root, 500);
    let session = workers
        .materialize_session_with_active(None, "user", None, None)
        .expect("materialized session");

    let error = session
        .agent_store
        .handle_checkpoint_bytes(&[0x80])
        .expect_err("malformed protobuf must fail");
    assert!(error.contains("invalid ConversationStateStructure checkpoint"));
    assert!(
        session
            .db
            .get_latest_root_blob_id()
            .expect("metadata root")
            .is_empty()
    );

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn production_agent_store_metadata_bridge_gets_sets_and_subscribes() {
    let root = temp_root("metadata-bridge");
    let workers = ProductionSessionWorkers::with_agents_root(&root, 500);
    let session = workers
        .materialize_session_with_active(None, "user", None, None)
        .expect("materialized session");

    assert_eq!(session.agent_store.get_id(), session.record.id);
    assert_eq!(
        session
            .agent_store
            .get_metadata(ProductionAgentMetadataKey::Mode)
            .expect("read mode"),
        Some(serde_json::Value::String("default".into()))
    );

    let seen = Arc::new(Mutex::new(Vec::new()));
    let capture = Arc::clone(&seen);
    let _subscription = session.agent_store.subscribe_to_metadata(
        ProductionAgentMetadataKey::Name,
        Arc::new(move |value| {
            capture.lock().expect("capture metadata").push(value);
        }),
    );

    assert!(
        session
            .agent_store
            .set_metadata(
                ProductionAgentMetadataKey::Name,
                serde_json::Value::String("Renamed Agent".into()),
            )
            .expect("set name")
    );
    assert_eq!(
        session
            .agent_store
            .get_metadata(ProductionAgentMetadataKey::Name)
            .expect("read name"),
        Some(serde_json::Value::String("Renamed Agent".into()))
    );
    assert_eq!(
        *seen.lock().expect("read captured metadata"),
        vec![Some(serde_json::Value::String("Renamed Agent".into()))]
    );

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn production_agent_store_reads_latest_request_and_full_conversation_from_same_root() {
    let root = temp_root("conversation-read");
    let workers = ProductionSessionWorkers::with_agents_root(&root, 500);
    let session = workers
        .materialize_session_with_active(None, "user", None, None)
        .expect("materialized session");
    let blob_store = workers
        .create_agent_blob_store(&session.record.id)
        .expect("blob store");

    let mut user = Vec::new();
    push_string(1, "hello from durable state", &mut user);
    push_string(2, "message-1", &mut user);
    let user_id = Sha256::digest(&user).to_vec();
    futures::executor::block_on(blob_store.set_blob(&(), &user_id, &user))
        .expect("user blob");

    let mut agent_turn = Vec::new();
    push_bytes(1, &user_id, &mut agent_turn);
    push_string(3, "request-42", &mut agent_turn);
    let mut turn = Vec::new();
    push_bytes(1, &agent_turn, &mut turn);
    let turn_id = Sha256::digest(&turn).to_vec();
    futures::executor::block_on(blob_store.set_blob(&(), &turn_id, &turn))
        .expect("turn blob");

    let mut checkpoint = Vec::new();
    push_bytes(8, &turn_id, &mut checkpoint);
    session
        .agent_store
        .handle_checkpoint_bytes(&checkpoint)
        .expect("checkpoint");

    assert_eq!(
        session
            .agent_store
            .get_last_request_id_from_conversation()
            .expect("last request id"),
        Some("request-42".into())
    );
    let conversation = session
        .agent_store
        .get_full_conversation()
        .expect("full conversation");
    assert_eq!(conversation.turns.len(), 1);
    assert!(conversation.todos.is_empty());
    assert_eq!(conversation.summary, None);

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn production_agent_store_metadata_key_surface_matches_frozen_agent_store() {
    let keys = [
        ProductionAgentMetadataKey::AgentId,
        ProductionAgentMetadataKey::LatestRootBlobId,
        ProductionAgentMetadataKey::Name,
        ProductionAgentMetadataKey::Mode,
        ProductionAgentMetadataKey::IsRunEverything,
        ProductionAgentMetadataKey::ApprovalMode,
        ProductionAgentMetadataKey::CreatedAt,
        ProductionAgentMetadataKey::LastUsedModel,
        ProductionAgentMetadataKey::LastDebugServerPort,
        ProductionAgentMetadataKey::CurrentPlanUri,
        ProductionAgentMetadataKey::SubagentInfo,
        ProductionAgentMetadataKey::BlobEncryptionKey,
    ];
    assert_eq!(
        keys.map(ProductionAgentMetadataKey::as_str),
        [
            "agentId",
            "latestRootBlobId",
            "name",
            "mode",
            "isRunEverything",
            "approvalMode",
            "createdAt",
            "lastUsedModel",
            "lastDebugServerPort",
            "currentPlanUri",
            "subagentInfo",
            "blobEncryptionKey",
        ]
    );
}


#[test]
fn production_agent_store_subagent_refs_override_inline_and_missing_refs_fall_back_inline() {
    let root = temp_root("subagents");
    let workers = ProductionSessionWorkers::with_agents_root(&root, 500);
    let session = workers
        .materialize_session_with_active(None, "user", None, None)
        .expect("materialized session");
    let blob_store = workers
        .create_agent_blob_store(&session.record.id)
        .expect("blob store");

    let referenced_state = subagent_state_bytes(20, 30);
    let referenced_id = Sha256::digest(&referenced_state).to_vec();
    futures::executor::block_on(blob_store.set_blob(
        &(),
        &referenced_id,
        &referenced_state,
    ))
    .expect("referenced subagent state");

    let missing_id = vec![0xAB; 32];
    let mut checkpoint = Vec::new();
    push_bytes(
        16,
        &map_entry("subagent-ref", &subagent_state_bytes(1, 2)),
        &mut checkpoint,
    );
    push_bytes(
        31,
        &map_entry("subagent-ref", &referenced_id),
        &mut checkpoint,
    );
    push_bytes(
        16,
        &map_entry("subagent-fallback", &subagent_state_bytes(7, 8)),
        &mut checkpoint,
    );
    push_bytes(
        31,
        &map_entry("subagent-fallback", &missing_id),
        &mut checkpoint,
    );
    session
        .agent_store
        .handle_checkpoint_bytes(&checkpoint)
        .expect("checkpoint");

    let (_, subagents) = session
        .agent_store
        .get_full_conversation_with_subagents()
        .expect("subagent hydration");
    assert_eq!(subagents.len(), 2);
    assert_eq!(subagents["subagent-ref"].created_timestamp_ms, 20);
    assert_eq!(subagents["subagent-ref"].last_used_timestamp_ms, 30);
    assert_eq!(subagents["subagent-fallback"].created_timestamp_ms, 7);
    assert_eq!(subagents["subagent-fallback"].last_used_timestamp_ms, 8);

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn production_agent_store_missing_subagent_ref_without_inline_fails_closed() {
    let root = temp_root("missing-subagent");
    let workers = ProductionSessionWorkers::with_agents_root(&root, 500);
    let session = workers
        .materialize_session_with_active(None, "user", None, None)
        .expect("materialized session");

    let mut checkpoint = Vec::new();
    push_bytes(
        31,
        &map_entry("subagent-missing", &[0xCD; 32]),
        &mut checkpoint,
    );
    session
        .agent_store
        .handle_checkpoint_bytes(&checkpoint)
        .expect("checkpoint");

    let error = session
        .agent_store
        .get_full_conversation_with_subagents()
        .expect_err("missing ref without inline fallback must fail");
    assert!(error.contains("subagent state ref blob not found"));

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}
