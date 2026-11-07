use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::agents::agent_profile::SandAgentProfile;
use mahayana_host_runtime::extensions::session::gateway::{
    SessionGatewayError, dispatch_production_session_gateway_call,
    persist_accepted_send_prompt,
};
use mahayana_host_runtime::extensions::session::agent_db_transcript_pages::HumanTranscriptPageQuery;
use mahayana_host_runtime::extensions::session::native_messaging::FabushiNativeMessagingClient;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::extension::{
    TranscriptExtension, start_transcript_extension,
};
use mahayana_host_runtime::host_event_bus::SandHostEventBus;
use mahayana_host_runtime::transcript_mutation_events::{
    TRANSCRIPT_MUTATION_ORIGIN_KEY, publish_transcript_mutation,
    subscribe_transcript_mutations, transcript_mutation_origin,
};
use serde_json::json;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-session-gateway-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn dispatch(
    runtime: &Arc<ProductionSessionWorkers>,
    method: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    dispatch_production_session_gateway_call(runtime, method, &args)
        .expect("handled method")
        .expect("successful method")
}


fn spawn_remote_history_server() -> (String, thread::JoinHandle<Vec<String>>) {
    // Three ascending-ID coverage pages, then the existing latest snapshot.
    // Use the bounded fixture so a missing or unexpected request fails causally.
    let replies = [(1_u64, 200_u64), (201, 400), (401, 401), (202, 401)]
        .into_iter()
        .map(|(low, high)| {
            let messages = (low..=high)
                .map(|id| {
                    let minute = id / 60;
                    let second = id % 60;
                    json!({
                        "id": id,
                        "senderUserId": 2,
                        "senderUsername": "peer",
                        "recipientUserId": 1,
                        "recipientUsername": "local",
                        "text": format!("remote-{id}"),
                        "clientRequestId": serde_json::Value::Null,
                        "createdAt": format!("1970-01-01T00:{minute:02}:{second:02}Z"),
                        "readAt": serde_json::Value::Null,
                        "isOutgoing": false
                    })
                })
                .collect::<Vec<_>>();
            HumanMessageFixtureReply::Json(200, json!({
                "success": true,
                "data": { "messages": messages }
            }))
        })
        .collect();
    spawn_human_message_fixture(replies)
}

#[test]
fn production_gateway_reads_transcript_pages_and_counts_agents_without_compat_host() {
    let root = temp_root("transcript");
    let agents = root.join("agents");
    let runtime = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let record = runtime
        .materialize_new_session(None, "user", None)
        .expect("agent");
    runtime
        .append_agent_transcript_entries(
            &record.id,
            &[
                json!({"id":"m1","kind":"message","role":"user","content":"hello","timestampMs":10}),
                json!({"id":"m2","kind":"send-message","message":{"type":"text","content":"reply"},"timestampMs":20}),
            ],
        )
        .expect("entries");

    assert_eq!(dispatch(&runtime, "countAgents", json!({})), json!(1));
    assert_eq!(
        dispatch(&runtime, "getAgentTranscript", json!({"id":record.id}))["0"],
        serde_json::Value::Null
    );
    let transcript = dispatch(&runtime, "getAgentTranscript", json!({"id":record.id}));
    assert_eq!(transcript.as_array().map(Vec::len), Some(2));

    let page = dispatch(
        &runtime,
        "getAgentTranscriptPage",
        json!({"id":record.id,"untilMs":100,"limit":10}),
    );
    assert_eq!(page["entries"].as_array().map(Vec::len), Some(2));
    assert_eq!(page["nextBeforeSeq"], serde_json::Value::Null);

    let window = dispatch(
        &runtime,
        "getAgentTranscriptWindow",
        json!({"id":record.id,"limit":10}),
    );
    assert_eq!(window["entries"].as_array().map(Vec::len), Some(2));
    assert!(window["threadCounts"].is_object());

    let tail = dispatch(
        &runtime,
        "getAgentTranscriptTail",
        json!({"id":record.id,"limit":1}),
    );
    assert_eq!(tail["entries"].as_array().map(Vec::len), Some(1));

    // The shipping Renderer reloads a selected Agent through openAgentTail.
    // Rust-created Agents must stay on the same Session owner instead of
    // falling through to the compatibility Host roster.
    let opened_tail = dispatch(
        &runtime,
        "openAgentTail",
        json!({"id":record.id,"limit":1}),
    );
    assert_eq!(opened_tail, tail);

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn production_gateway_composes_channel_metadata_and_secrets() {
    let root = temp_root("channels");
    let agents = root.join("agents");
    let runtime = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let record = runtime
        .materialize_new_session(None, "user", None)
        .expect("agent");

    let connected = dispatch(
        &runtime,
        "connectChannel",
        json!({"id":record.id,"platform":"slack","token":"secret"}),
    );
    assert_eq!(connected.as_array().map(Vec::len), Some(1));
    assert_eq!(connected[0]["platform"], "slack");
    assert_eq!(
        dispatch(&runtime, "getAgentChannels", json!({"id":record.id})),
        connected
    );
    assert_eq!(
        dispatch(&runtime, "refreshChannel", json!({"id":record.id})),
        connected
    );
    let disconnected = dispatch(
        &runtime,
        "disconnectChannel",
        json!({"id":record.id,"platform":"slack"}),
    );
    assert_eq!(disconnected, json!([]));

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn production_gateway_creates_a_real_session_record_for_the_shipping_new_chat_path() {
    let root = temp_root("create-agent");
    let agents = root.join("agents");
    let runtime = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));

    let created = dispatch(
        &runtime,
        "createAgent",
        json!({
            "name": "New chat",
            "description": "",
            "origin": "user",
            "isIntroductionSuppressed": true,
            "isKickstartRequested": false,
            "clientNonce": "e2e-create-agent"
        }),
    );

    let agent_id = created["agent"]["id"]
        .as_str()
        .expect("created agent id")
        .to_string();
    assert_eq!(created["agent"]["name"], "New chat");
    assert_eq!(created["agent"]["isActive"], true);
    assert_eq!(created["transcript"], json!([]));
    assert!(runtime.session_db_path(&agent_id).expect("db path").is_file());

    let listed = dispatch(&runtime, "listAgents", json!({}));
    assert_eq!(listed.as_array().map(Vec::len), Some(1));
    assert_eq!(listed[0]["id"], agent_id);
    assert_eq!(listed[0]["name"], "New chat");
    assert!(!runtime
        .get_agent_introduction_pending(&agent_id)
        .expect("introduction state"));

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn production_gateway_cuts_over_safe_agent_lifecycle_mutations_to_rust_session_owner() {
    let root = temp_root("agent-lifecycle");
    let agents = root.join("agents");
    let runtime = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let record = runtime
        .materialize_new_session(
            Some(&SandAgentProfile {
                name: "Fresh Agent".into(),
                description: String::new(),
                title: String::new(),
                avatar_shape: String::new(),
                avatar_color: String::new(),
            }),
            "user",
            None,
        )
        .expect("agent");

    // Frozen Grok roster semantics intentionally hide a completely blank
    // default "Grok" agent. Give this lifecycle fixture a real identity so
    // listAgents tests the shipping cutover without weakening that filter.
    let listed = dispatch(&runtime, "listAgents", json!({}));
    assert_eq!(listed.as_array().map(Vec::len), Some(1));
    assert_eq!(listed[0]["id"], record.id);

    let updated = dispatch(
        &runtime,
        "updateAgent",
        json!({
            "id": record.id,
            "profile": {
                "name": "  Lifecycle Agent  ",
                "description": "  production gateway  ",
                "title": "  Operator  ",
                "avatarShape": " rounded ",
                "avatarColor": " violet "
            }
        }),
    );
    assert_eq!(updated["name"], "Lifecycle Agent");
    assert_eq!(updated["description"], "production gateway");
    assert_eq!(updated["title"], "Operator");

    assert_eq!(
        dispatch(
            &runtime,
            "setAgentUnread",
            json!({"id": record.id, "isUnread": true, "atMs": 1234}),
        ),
        serde_json::Value::Null
    );
    assert_eq!(
        dispatch(
            &runtime,
            "setAgentNotifyOnUpdates",
            json!({"id": record.id, "isEnabled": false}),
        ),
        serde_json::Value::Null
    );
    assert_eq!(
        dispatch(
            &runtime,
            "setAgentHiddenFromSidebar",
            json!({"id": record.id, "isHidden": true}),
        ),
        serde_json::Value::Null
    );
    let mutated = dispatch(&runtime, "listAgents", json!({}));
    assert_eq!(mutated[0]["hasUnread"], true);
    assert_eq!(mutated[0]["notifyOnUpdatesEnabled"], false);
    assert_eq!(mutated[0]["isHiddenFromSidebar"], true);

    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9Z2S8AAAAASUVORK5CYII=";
    let _summary = dispatch(
        &runtime,
        "setAgentAvatarBytes",
        json!({"id": record.id, "pngBase64": png}),
    );
    let avatar = dispatch(&runtime, "getAgentAvatar", json!({"id": record.id}));
    assert!(avatar["version"].as_str().is_some_and(|value| !value.is_empty()));
    assert!(avatar["dataUrl"]
        .as_str()
        .is_some_and(|value| value.starts_with("data:image/png;base64,")));

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn accepted_send_prompt_is_persisted_into_the_rust_authoritative_transcript() {
    let root = temp_root("send-prompt-persistence");
    let agents = root.join("agents");
    let runtime = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let record = runtime
        .materialize_new_session(None, "user", None)
        .expect("agent");
    let session_key = runtime
        .read_session_work_admission(&record.id)
        .expect("admission")
        .expect("admission record")
        .session_key;

    persist_accepted_send_prompt(
        &runtime,
        &json!({
            "agentId": record.id,
            "sessionKey": session_key,
            "prompt": "",
            "attachmentPaths": ["/tmp/attachment-only.txt"],
            "attachmentNames": ["attachment-only.txt"],
            "clientNonce": "nonce-attachment-only",
            "composedAtMs": 1234
        }),
        &json!({
            "accepted": true,
            "operationId": "operation-attachment-only"
        }),
    )
    .expect("persist accepted sendPrompt");

    let transcript = dispatch(
        &runtime,
        "getAgentTranscript",
        json!({"id":record.id}),
    );
    let entries = transcript.as_array().expect("transcript array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["id"], "t0ua0");
    assert_eq!(entries[0]["kind"], "user-attachment");
    assert_eq!(entries[0]["file_path"], "/tmp/attachment-only.txt");
    assert_eq!(entries[0]["file_name"], "attachment-only.txt");
    assert_eq!(entries[0]["clientNonce"], "nonce-attachment-only");
    assert_eq!(
        entries[0].get("sessionKey").and_then(serde_json::Value::as_str),
        runtime
            .read_session_work_admission(&record.id)
            .expect("admission")
            .as_ref()
            .map(|record| record.session_key.as_str())
    );
    assert!(entries[0]["batchId"].as_str().is_some_and(|value| !value.is_empty()));

    // Production nonce admission normally coalesces this before persistence.
    // The persistence boundary itself also remains idempotent by clientNonce.
    persist_accepted_send_prompt(
        &runtime,
        &json!({
            "agentId": record.id,
            "prompt": "",
            "attachmentPaths": ["/tmp/attachment-only.txt"],
            "attachmentNames": ["attachment-only.txt"],
            "clientNonce": "nonce-attachment-only",
            "composedAtMs": 1234
        }),
        &json!({
            "accepted": true,
            "operationId": "operation-attachment-only"
        }),
    )
    .expect("idempotent persistence");
    assert_eq!(
        dispatch(&runtime, "getAgentTranscript", json!({"id":record.id}))
            .as_array()
            .map(Vec::len),
        Some(1)
    );


    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn session_key_send_prompt_rechecks_admission_at_transcript_write() {
    let root = temp_root("send-prompt-admission");
    let agents = root.join("agents");
    let runtime = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let record = runtime
        .materialize_new_session(None, "user", None)
        .expect("agent");
    let session_key = format!("agent:{}:main", record.id);
    let admission = runtime
        .read_session_work_admission(&record.id)
        .expect("read admission")
        .expect("admission");
    runtime
        .transition_session_work_archive(
            &record.id,
            &session_key,
            &record.id,
            Some(&admission.lifecycle_revision),
            true,
            Some("contract"),
            Some("write-fence"),
            1000.0,
        )
        .expect("archive session");

    let error = persist_accepted_send_prompt(
        &runtime,
        &json!({
            "agentId": record.id,
            "sessionKey": session_key,
            "prompt": "must be rejected",
            "clientNonce": "nonce-archived",
        }),
        &json!({"accepted": true}),
    )
    .expect_err("archived session write must be rejected");
    assert!(error.to_string().contains("not admitted"));
    assert!(runtime
        .read_agent_transcript_entries(&record.id)
        .expect("read transcript")
        .is_empty());

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn accepted_send_prompt_uses_frozen_entry_ids_and_reply_fork_stamping() {
    let root = temp_root("send-prompt-threading");
    let agents = root.join("agents");
    let runtime = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let record = runtime.materialize_new_session(None, "user", None).expect("agent");

    let first = persist_accepted_send_prompt(
        &runtime,
        &json!({"agentId":record.id,"prompt":"first","clientNonce":"nonce-first","composedAtMs":1000}),
        &json!({"accepted":true,"operationId":"operation-first"}),
    ).expect("first send");
    assert_eq!(first.as_deref(), Some("t0u"));

    let second = persist_accepted_send_prompt(
        &runtime,
        &json!({
            "agentId":record.id,
            "prompt":" reply ",
            "richText":"**reply**",
            "replyToId":"t0u",
            "isFork":true,
            "clientNonce":"nonce-reply",
            "composedAtMs":2000
        }),
        &json!({"accepted":true,"operationId":"operation-reply"}),
    ).expect("reply send");
    assert_eq!(second.as_deref(), Some("t1u"));

    let transcript = dispatch(&runtime, "getAgentTranscript", json!({"id":record.id}));
    let entries = transcript.as_array().expect("transcript array");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["id"], "t0u");
    assert_eq!(entries[1]["id"], "t1u");
    assert_eq!(entries[1]["content"], "reply");
    assert_eq!(entries[1]["richText"], "**reply**");
    assert_eq!(entries[1]["replyTo"], "t0u");
    assert_eq!(entries[1]["branched"], true);
    assert_eq!(entries[1]["timestampMs"], 2000.0);
    assert_eq!(entries[1]["sentWhileOfflineAtMs"], 2000.0);


    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn malformed_session_gateway_requests_fail_closed_and_unknown_methods_fall_through() {
    let root = temp_root("bad-request");
    let runtime = Arc::new(ProductionSessionWorkers::with_agents_root(root.join("agents"), 500));
    assert_eq!(
        dispatch_production_session_gateway_call(
            &runtime,
            "getAgentTranscript",
            &json!({"id":""}),
        ),
        Some(Err(SessionGatewayError::BadRequest(
            "missing or invalid id".to_string()
        )))
    );
    assert!(dispatch_production_session_gateway_call(
        &runtime,
        "not-a-session-method",
        &json!({}),
    )
    .is_none());
    let _ = fs::remove_dir_all(root);
}


#[test]
fn production_gateway_owns_conversation_outline_for_fresh_agents() {
    let root = temp_root("conversation-outline");
    let agents = root.join("agents");
    let runtime = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let record = runtime.materialize_new_session(None, "user", None).expect("agent");

    assert_eq!(
        dispatch(
            &runtime,
            "getConversationOutline",
            json!({"id":record.id}),
        ),
        json!([])
    );

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn production_gateway_owns_group_creation_membership_and_nested_group_rejection() {
    let root = temp_root("groups");
    let agents = root.join("agents");
    let runtime = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));

    let first = dispatch(
        &runtime,
        "createAgent",
        json!({"name":"Alice","description":"","origin":"user","isIntroductionSuppressed":true}),
    );
    let second = dispatch(
        &runtime,
        "createAgent",
        json!({"name":"Bob","description":"","origin":"user","isIntroductionSuppressed":true}),
    );
    let first_id = first["agent"]["id"].as_str().expect("alice id").to_string();
    let second_id = second["agent"]["id"].as_str().expect("bob id").to_string();

    let created = dispatch(
        &runtime,
        "createGroup",
        json!({
            "name":"Team",
            "description":"Production group",
            "memberIds":[first_id.clone(), second_id.clone(), first_id.clone()]
        }),
    );
    let group_id = created["agent"]["id"].as_str().expect("group id").to_string();
    assert_eq!(created["agent"]["isGroup"], true);
    assert_eq!(
        created["agent"]["memberIds"],
        json!([first_id.clone(), second_id.clone()])
    );
    assert_eq!(created["transcript"], json!([]));

    let duplicate = dispatch(
        &runtime,
        "createGroup",
        json!({
            "name":"Ignored duplicate name",
            "memberIds":[second_id.clone(), first_id.clone()]
        }),
    );
    assert_eq!(duplicate["agent"]["id"], group_id);

    let updated = dispatch(
        &runtime,
        "setGroupMembers",
        json!({"id":group_id.clone(),"memberIds":[second_id.clone()]}),
    );
    assert_eq!(updated["isGroup"], true);
    assert_eq!(updated["memberIds"], json!([second_id.clone()]));

    let nested = dispatch_production_session_gateway_call(
        &runtime,
        "createGroup",
        &json!({"name":"Nested","memberIds":[group_id.clone()]}),
    )
    .expect("createGroup handled")
    .expect_err("nested group rejected");
    assert!(matches!(nested, SessionGatewayError::BadRequest(message) if message.contains("individual agents")));

    let listed = dispatch(&runtime, "listAgents", json!({}));
    assert!(listed
        .as_array()
        .expect("agents")
        .iter()
        .any(|agent| agent["id"] == group_id && agent["isGroup"] == true));

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn human_conversation_uses_session_transcript_owner_without_becoming_an_agent() {
    let root = temp_root("human-conversation");
    let agents = root.join("agents");
    let memory = Arc::new(mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(agents.clone()));
    let runtime = Arc::new(ProductionSessionWorkers::with_agents_root_identity_and_dependencies(
        &agents,
        500,
        Some("human-local".into()),
        Arc::new(|| None),
        memory,
    ));
    let agent = runtime
        .materialize_new_session(
            Some(&SandAgentProfile {
                name: "Agent peer".into(),
                description: String::new(),
                title: String::new(),
                avatar_shape: String::new(),
                avatar_color: String::new(),
            }),
            "user",
            None,
        )
        .expect("agent");

    let created = dispatch(
        &runtime,
        "createHumanConversation",
        json!({
            "localHumanId": "human-local",
            "peerHumanId": "human-peer",
            "title": "Human peer"
        }),
    );
    let conversation_id = created["id"]
        .as_str()
        .expect("conversation id")
        .to_string();
    assert_eq!(created["kind"], "human");
    let human_db = runtime
        .open_human_conversation_db_owner(&conversation_id)
        .expect("Human conversation database");
    assert_eq!(
        human_db.get_metadata("conversationKind").expect("kind"),
        Some(json!("human"))
    );
    assert_eq!(human_db.get_metadata("agentId").expect("agent id"), None);
    assert_eq!(human_db.get_metadata("blobEncryptionKey").expect("blob key"), None);
    assert_eq!(human_db.get_metadata("mode").expect("agent mode"), None);
    assert_eq!(dispatch(&runtime, "countAgents", json!({})), json!(1));
    let agents_list = dispatch(&runtime, "listAgents", json!({}));
    assert_eq!(agents_list.as_array().map(Vec::len), Some(1));
    assert_eq!(agents_list[0]["id"], agent.id);

    let conversations = dispatch(&runtime, "listHumanConversations", json!({}));
    assert_eq!(conversations.as_array().map(Vec::len), Some(1));
    assert_eq!(conversations[0]["id"], conversation_id);
    assert_eq!(conversations[0]["updatedAt"], 0.0);

    let spoofed_sender = dispatch_production_session_gateway_call(
        &runtime,
        "sendHumanMessage",
        &json!({
            "conversationId": conversation_id,
            "senderId": "human-peer",
            "text": "spoofed",
            "clientNonce": "human-spoof"
        }),
    )
    .expect("handled spoofed Human send")
    .expect_err("renderer-provided peer identity must not become the local sender");
    assert!(matches!(spoofed_sender, SessionGatewayError::BadRequest(_)));

    let first = dispatch(
        &runtime,
        "sendHumanMessage",
        json!({
            "conversationId": conversation_id,
            "senderId": "human-local",
            "text": "hello human",
            "clientNonce": "human-nonce-1",
            "composedAtMs": 1234
        }),
    );
    assert_eq!(first["authorKind"], "human");
    assert_eq!(first["authorId"], "human-local");
    assert_eq!(first["clientNonce"], "human-nonce-1");
    assert_eq!(
        human_db
            .get_unread_state()
            .expect("Human conversation unread state")
            .unread_count,
        0.0,
        "outgoing Human messages must not reuse Agent unread activity semantics"
    );
    assert_eq!(
        human_db
            .get_metadata("lastActivityAt")
            .expect("Human conversation activity metadata"),
        Some(json!(1234.0))
    );

    let replay = dispatch(
        &runtime,
        "sendHumanMessage",
        json!({
            "conversationId": conversation_id,
            "senderId": "human-local",
            "text": "hello human",
            "clientNonce": "human-nonce-1",
            "composedAtMs": 1234
        }),
    );
    assert_eq!(replay, first);

    let reply = dispatch(
        &runtime,
        "sendHumanMessage",
        json!({
            "conversationId": conversation_id,
            "senderId": "human-local",
            "text": "reply with file",
            "clientNonce": "human-nonce-2",
            "composedAtMs": 1500,
            "replyToId": "human-message:human-nonce-1",
            "attachments": [{
                "path": "committed-resource-report",
                "name": "report.pdf"
            }]
        }),
    );
    assert_eq!(reply["replyToId"], "human-message:human-nonce-1");
    assert_eq!(
        reply["attachments"],
        json!([{
            "path": "committed-resource-report",
            "name": "report.pdf"
        }])
    );

    let attachment_only = dispatch(
        &runtime,
        "sendHumanMessage",
        json!({
            "conversationId": conversation_id,
            "senderId": "human-local",
            "text": "",
            "clientNonce": "human-nonce-3",
            "composedAtMs": 1600,
            "attachments": [{
                "path": "committed-resource-image",
                "name": "image.png"
            }]
        }),
    );
    assert_eq!(attachment_only["content"], "");
    assert_eq!(attachment_only["attachments"][0]["name"], "image.png");

    let invalid_reply = dispatch_production_session_gateway_call(
        &runtime,
        "sendHumanMessage",
        &json!({
            "conversationId": conversation_id,
            "senderId": "human-local",
            "text": "bad reply",
            "clientNonce": "human-nonce-invalid-reply",
            "replyToId": "foreign-message"
        }),
    )
    .expect("handled Human reply")
    .expect_err("reply target outside the Human transcript must fail closed");
    assert!(matches!(invalid_reply, SessionGatewayError::Internal(message) if message.contains("reply target")));

    let agent_result = runtime
        .append_human_agent_message(
            &conversation_id,
            &agent.id,
            "runner-stream-1",
            None,
            "trusted Agent result",
            2345.0,
        )
        .expect("Host-internal Agent result projection");
    assert_eq!(agent_result["role"], "assistant");
    assert_eq!(agent_result["authorKind"], "agent");
    assert_eq!(agent_result["authorId"], agent.id);
    assert_eq!(agent_result["sourceStreamId"], "runner-stream-1");
    assert_eq!(
        runtime
            .append_human_agent_message(
                &conversation_id,
                &agent.id,
                "runner-stream-1",
                None,
                "trusted Agent result",
                2345.0,
            )
            .expect("idempotent Agent result replay"),
        agent_result
    );
    assert!(
        runtime
            .append_human_agent_message(
                &conversation_id,
                &agent.id,
                "runner-stream-1",
                None,
                "different Agent result",
                2345.0,
            )
            .is_err(),
        "one Runner stream must not identify different Human-transcript content"
    );

    let session_key = format!("agent:{}:main", agent.id);
    let bound_agent_result = runtime
        .append_human_agent_message(
            &conversation_id,
            &agent.id,
            "runner-stream-bound",
            Some(&session_key),
            "session-bound Agent result",
            2346.0,
        )
        .expect("session-bound Human handoff Agent result");
    assert_eq!(bound_agent_result["sessionKey"], session_key);
    assert_eq!(
        runtime
            .append_human_agent_message(
                &conversation_id,
                &agent.id,
                "runner-stream-bound",
                Some(&session_key),
                "session-bound Agent result",
                2346.0,
            )
            .expect("idempotent session-bound Agent result replay"),
        bound_agent_result
    );
    assert!(
        runtime
            .append_human_agent_message(
                &conversation_id,
                &agent.id,
                "runner-stream-bound",
                Some("agent:other:main"),
                "session-bound Agent result",
                2346.0,
            )
            .is_err(),
        "one Human handoff stream must not be rebound to another session key"
    );
    assert!(
        dispatch_production_session_gateway_call(
            &runtime,
            "appendHumanAgentMessage",
            &json!({
                "conversationId": conversation_id,
                "agentId": agent.id,
                "streamId": "renderer-forgery",
                "content": "forged"
            }),
        )
        .is_none(),
        "trusted Agent result projection must not be renderer-callable"
    );

    let transcript = dispatch(
        &runtime,
        "getHumanConversationTranscript",
        json!({"conversationId": conversation_id}),
    );
    assert_eq!(transcript.as_array().map(Vec::len), Some(5));
    assert_eq!(transcript[0]["id"], "human-message:human-nonce-1");
    assert_eq!(transcript[1], reply);
    assert_eq!(transcript[2], attachment_only);
    assert_eq!(transcript[3], agent_result);
    assert_eq!(transcript[4], bound_agent_result);

    let human_tail = dispatch(
        &runtime,
        "getHumanConversationTranscriptTail",
        json!({
            "conversationId": conversation_id,
            "limit": 3
        }),
    );
    assert_eq!(human_tail["entries"], json!([attachment_only.clone(), agent_result.clone(), bound_agent_result.clone()]));
    let older_cursor = human_tail["nextBeforeSeq"]
        .as_i64()
        .expect("Human transcript tail should expose an older-page cursor");
    let human_older = dispatch(
        &runtime,
        "getHumanConversationTranscriptTail",
        json!({
            "conversationId": conversation_id,
            "limit": 2,
            "beforeSeq": older_cursor
        }),
    );
    assert_eq!(human_older["entries"], json!([first.clone(), reply.clone()]));
    assert!(human_older["nextBeforeSeq"].is_null());

    let attachment_search = dispatch(
        &runtime,
        "searchHumanMessages",
        json!({
            "conversationId": conversation_id,
            "query": "REPORT",
            "limit": 10
        }),
    );
    assert_eq!(attachment_search.as_array().map(Vec::len), Some(1));
    assert_eq!(attachment_search[0]["id"], reply["id"]);
    let agent_search = dispatch(
        &runtime,
        "searchHumanMessages",
        json!({
            "conversationId": conversation_id,
            "query": "trusted agent",
            "limit": 1
        }),
    );
    assert_eq!(agent_search, json!([agent_result.clone()]));
    let empty_search = dispatch_production_session_gateway_call(
        &runtime,
        "searchHumanMessages",
        &json!({
            "conversationId": conversation_id,
            "query": "   "
        }),
    )
    .expect("handled Human search")
    .expect_err("empty Human search query must fail closed");
    assert!(matches!(empty_search, SessionGatewayError::BadRequest(_)));

    let conversations_after_send = dispatch(&runtime, "listHumanConversations", json!({}));
    assert_eq!(conversations_after_send[0]["updatedAt"], 2346.0);

    let conflicting = dispatch_production_session_gateway_call(
        &runtime,
        "sendHumanMessage",
        &json!({
            "conversationId": conversation_id,
            "senderId": "human-local",
            "text": "different content",
            "clientNonce": "human-nonce-1",
            "composedAtMs": 1234
        }),
    )
    .expect("handled Human send")
    .expect_err("nonce collision must reject different content");
    match conflicting {
        SessionGatewayError::Internal(message) => {
            assert!(message.contains("different content"));
        }
        other => panic!("unexpected nonce collision error: {other:?}"),
    }

    runtime.shutdown();

    let restarted_memory = Arc::new(mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(agents.clone()));
    let restarted = Arc::new(ProductionSessionWorkers::with_agents_root_identity_and_dependencies(
        &agents,
        500,
        Some("human-local".into()),
        Arc::new(|| None),
        restarted_memory,
    ));
    let restored = dispatch(
        &restarted,
        "getHumanConversationTranscript",
        json!({"conversationId": conversation_id}),
    );
    assert_eq!(restored, transcript);
    assert_eq!(dispatch(&restarted, "countAgents", json!({})), json!(1));

    let foreign_memory = Arc::new(mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(agents.clone()));
    let foreign = Arc::new(ProductionSessionWorkers::with_agents_root_identity_and_dependencies(
        &agents,
        500,
        Some("human-other".into()),
        Arc::new(|| None),
        foreign_memory,
    ));
    assert_eq!(dispatch(&foreign, "listHumanConversations", json!({})), json!([]));
    let denied = dispatch_production_session_gateway_call(
        &foreign,
        "getHumanConversationTranscript",
        &json!({"conversationId": conversation_id}),
    )
    .expect("handled foreign Human transcript")
    .expect_err("foreign Human identity must not read the conversation");
    assert!(matches!(denied, SessionGatewayError::Internal(_)));
    let foreign_search = dispatch_production_session_gateway_call(
        &foreign,
        "searchHumanMessages",
        &json!({
            "conversationId": conversation_id,
            "query": "hello"
        }),
    )
    .expect("handled foreign Human search")
    .expect_err("foreign Human identity must not search the conversation");
    assert!(matches!(foreign_search, SessionGatewayError::Internal(_)));
    foreign.shutdown();
    restarted.shutdown();
    let _ = fs::remove_dir_all(root);
}


fn read_fixture_http_request(stream: &mut std::net::TcpStream) -> String {
    read_fixture_http_request_before(stream, None)
}

fn read_fixture_http_request_before(
    stream: &mut std::net::TcpStream,
    deadline: Option<std::time::Instant>,
) -> String {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 4096];
    let mut header_end = None;
    let mut content_length = 0usize;
    loop {
        if let Some(deadline) = deadline {
            let remaining = deadline.checked_duration_since(std::time::Instant::now())
                .filter(|remaining| !remaining.is_zero())
                .expect("fixture request exceeded its total read deadline");
            stream.set_read_timeout(Some(remaining)).expect("bound fixture request read");
        }
        let read = stream.read(&mut chunk).expect("read fixture request");
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
        if header_end.is_none() {
            if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                header_end = Some(index + 4);
                let headers = String::from_utf8_lossy(&request[..index + 4]);
                content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':').and_then(|(name, value)| {
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                    })
                    .unwrap_or(0);
            }
        }
        if let Some(header_end) = header_end {
            if request.len() >= header_end + content_length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&request).into_owned()
}

fn spawn_remote_reaction_server() -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind reaction fixture");
    let address = listener.local_addr().expect("fixture address");
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        for (active, count, reacted_by_me) in [(true, 2_u64, true), (false, 1_u64, false)] {
            let (mut stream, _) = listener.accept().expect("accept reaction mutation");
            requests.push(read_fixture_http_request(&mut stream));
            let body = json!({
                "success": true,
                "messageId": 11,
                "reactions": [{
                    "emoji": "👍",
                    "count": count,
                    "reactedByMe": reacted_by_me
                }]
            }).to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            ).expect("write reaction response");
            stream.flush().expect("flush reaction response");
            let _ = active;
        }
        requests
    });
    (format!("http://{address}"), handle)
}

fn spawn_remote_reply_attachment_server() -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind reply attachment fixture");
    let address = listener.local_addr().expect("fixture address");
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();

        let (mut upload_stream, _) = listener.accept().expect("accept resource upload");
        requests.push(read_fixture_http_request(&mut upload_stream));
        let upload_body = json!({
            "success": true,
            "resource": {
                "resourceId": "resource-1",
                "name": "report.pdf",
                "contentType": "application/octet-stream",
                "size": 4,
                "createdAt": "2026-10-05T01:00:00Z"
            }
        }).to_string();
        write!(
            upload_stream,
            "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            upload_body.len(),
            upload_body
        ).expect("write resource response");
        upload_stream.flush().expect("flush resource response");

        let (mut send_stream, _) = listener.accept().expect("accept message send");
        requests.push(read_fixture_http_request(&mut send_stream));
        let send_body = json!({
            "success": true,
            "deduplicated": false,
            "message": {
                "id": 11,
                "senderUserId": 1,
                "senderUsername": "local",
                "recipientUserId": 2,
                "recipientUsername": "peer",
                "text": "reply with file",
                "clientRequestId": "shipping-reply-1",
                "createdAt": "2026-10-05T01:00:01Z",
                "readAt": serde_json::Value::Null,
                "isOutgoing": true,
                "replyToMessageId": 10,
                "attachments": [{
                    "resourceId": "resource-1",
                    "name": "report.pdf",
                    "contentType": "application/octet-stream",
                    "size": 4,
                    "createdAt": "2026-10-05T01:00:00Z"
                }],
                "reactions": []
            }
        }).to_string();
        write!(
            send_stream,
            "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            send_body.len(),
            send_body
        ).expect("write send response");
        send_stream.flush().expect("flush send response");

        let (mut resource_stream, _) = listener.accept().expect("accept resource download");
        requests.push(read_fixture_http_request(&mut resource_stream));
        let resource = [1_u8, 2, 3, 4];
        write!(
            resource_stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            resource.len()
        ).expect("write resource download headers");
        resource_stream.write_all(&resource).expect("write resource download body");
        resource_stream.flush().expect("flush resource download");

        requests
    });
    (format!("http://{address}"), handle)
}

#[test]
fn shipping_human_send_uploads_attachment_and_binds_reply_to_server_identity() {
    let root = temp_root("human-shipping-reply-attachment");
    let agents = root.join("agents");
    fs::create_dir_all(&root).expect("fixture root");
    let credential_path = root.join("host-credential.json");
    fs::write(
        &credential_path,
        r#"{"accessToken":"fixture-token","deviceId":"device-a","userId":1}"#,
    )
    .expect("credential");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&credential_path)
            .expect("credential metadata")
            .permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&credential_path, permissions).expect("private credential");
    }
    let attachment_path = root.join("report.pdf");
    fs::write(&attachment_path, [1_u8, 2, 3, 4]).expect("attachment fixture");

    let (base_url, server) = spawn_remote_reply_attachment_server();
    fs::write(&credential_path, serde_json::json!({
        "accessToken": "fixture-token", "deviceId": "device-a", "userId": 1, "issuer": &base_url
    }).to_string()).expect("issuer-bound fixture credential");
    let client = Arc::new(
        FabushiNativeMessagingClient::new(&base_url, &credential_path)
            .expect("shipping messaging client"),
    );
    let memory = Arc::new(
        mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(
            agents.clone(),
        ),
    );
    let runtime = Arc::new(
        ProductionSessionWorkers::with_agents_root_identity_messaging_and_dependencies(
            &agents,
            500,
            Some("1".into()),
            Some(client),
            true,
            None,
            Arc::new(|| None),
            memory,
        ),
    );
    let created = runtime
        .create_human_conversation("2", "Peer")
        .expect("Human conversation");
    let conversation_id = created["id"]
        .as_str()
        .expect("conversation id")
        .to_string();
    let owner = runtime
        .open_human_conversation_db_owner(&conversation_id)
        .expect("conversation owner");
    assert!(owner
        .append_transcript_entry(&json!({
            "id": "human-server-message:10",
            "kind": "message",
            "role": "user",
            "authorKind": "human",
            "authorId": "2",
            "content": "remote anchor",
            "timestampMs": 1000.0,
            "delivery": "sent",
            "remoteMessageId": "10",
            "remoteCreatedAt": "2026-10-05T01:03:21Z"
        }))
        .expect("remote anchor"));

    let sent = dispatch(
        &runtime,
        "sendHumanMessage",
        json!({
            "conversationId": conversation_id,
            "senderId": "1",
            "text": "reply with file",
            "clientNonce": "shipping-reply-1",
            "replyToId": "human-server-message:10",
            "attachments": [{
                "path": attachment_path.to_string_lossy(),
                "name": "report.pdf"
            }]
        }),
    );
    assert_eq!(sent["id"], "human-server-message:11");
    assert_eq!(sent["remoteMessageId"], "11");
    assert_eq!(sent["replyToId"], "human-server-message:10");
    assert_eq!(sent["remoteReplyToMessageId"], "10");
    assert_eq!(sent["attachments"][0]["resourceId"], "resource-1");
    let materialized_path = sent["attachments"][0]["path"]
        .as_str()
        .expect("materialized attachment path");
    assert!(materialized_path.ends_with("human-message-resources/resource-1/report.pdf"));
    assert_eq!(fs::read(materialized_path).expect("materialized attachment bytes"), [1_u8, 2, 3, 4]);
    assert_eq!(sent["attachments"][0]["name"], "report.pdf");

    let requests = server.join().expect("shipping send fixture");
    assert_eq!(requests.len(), 3);
    assert!(requests[0].starts_with("POST /api/social/message-resources "));
    assert!(requests[0].contains("filename=\"report.pdf\""));
    assert!(requests[1].starts_with("POST /api/social/messages "));
    assert!(requests[1].contains(r#""replyToMessageId":"10""#));
    assert!(requests[1].contains(r#""resourceId":"resource-1""#));
    assert!(requests[1].contains(r#""clientRequestId":"shipping-reply-1""#));
    assert!(requests[2].starts_with("GET /api/social/message-resources/resource-1 "));

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_oversized_upload_stays_pending_across_restart_and_recovers_same_nonce() {
    const UPLOAD_LIMIT: u64 = 25 * 1024 * 1024;
    const EXPECTED_ERROR: &str =
        "Fabushi Human attachment exceeds the 25 MiB message upload limit";
    const NONCE: &str = "bounded-video-upload";
    const VIDEO_BYTES: &[u8] = b"\x00\x00\x00\x18ftypmp42\x00\x00\x00\x00mp42isom";
    let root = temp_root("human-upload-bound-recovery");
    fs::create_dir_all(&root).expect("upload fixture root");
    let attachment_path = root.join("bounded-video.mp4");
    let oversized = fs::File::create(&attachment_path).expect("sparse video fixture");
    oversized.set_len(UPLOAD_LIMIT + 1).expect("oversized sparse video length");
    drop(oversized);
    assert_eq!(fs::metadata(&attachment_path).unwrap().len(), UPLOAD_LIMIT + 1);

    let resource = json!({
        "resourceId":"bounded-video-resource", "name":"bounded-video.mp4",
        "contentType":"video/mp4", "size":VIDEO_BYTES.len(),
        "createdAt":"2026-10-05T01:00:00Z"
    });
    let remote = fixture_remote_human_message(
        92, 1, 2, "bounded video upload", Some(NONCE), json!([resource.clone()]),
    );
    let (seen, accepted) = std::sync::mpsc::channel();
    let (release, resume) = std::sync::mpsc::channel();
    let (url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Deferred(
            Box::new(HumanMessageFixtureReply::Json(
                201, json!({"success":true,"resource":resource}),
            )),
            seen,
            resume,
        ),
        HumanMessageFixtureReply::Json(201, json!({"success":true,"message":remote})),
        HumanMessageFixtureReply::Bytes(200, VIDEO_BYTES.to_vec()),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("conversation id").to_string();
    let args = json!({
        "conversationId":id, "senderId":"1", "text":"bounded video upload",
        "clientNonce":NONCE, "composedAtMs":1000.0,
        "attachments":[{"path":attachment_path.to_string_lossy(),"name":"bounded-video.mp4"}]
    });
    let assert_rejected_send = |runtime: &Arc<ProductionSessionWorkers>| {
        let error = dispatch_production_session_gateway_call(runtime, "sendHumanMessage", &args)
            .expect("handled Human send")
            .expect_err("oversized video must fail at the Native upload boundary");
        match error {
            SessionGatewayError::Internal(message) => {
                assert!(message.contains(EXPECTED_ERROR), "{message}");
            }
            other => panic!("unexpected oversized Human send error: {other:?}"),
        }
        // The ordinary synchronous send has returned its preflight error.
        // No first HTTP request may have reached the still-live fixture.
        assert!(matches!(
            accepted.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ), "resource and message dispatch must remain at zero");
    };
    let read_pending = |runtime: &Arc<ProductionSessionWorkers>| {
        let owner = runtime.open_human_conversation_db_owner(&id).expect("SQLite owner");
        let rows = owner.get_transcript_entries().expect("durable rejected send");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], format!("human-message:{NONCE}"));
        assert_eq!(rows[0]["authorId"], "1");
        assert_eq!(rows[0]["clientNonce"], NONCE);
        assert_eq!(rows[0]["content"], "bounded video upload");
        assert_eq!(rows[0]["delivery"], "pending");
        assert_eq!(rows[0]["lastDispatchError"], EXPECTED_ERROR);
        assert!(rows[0]["lastDispatchFailedAtMs"].is_number());
        assert_eq!(rows[0]["attachments"], args["attachments"]);
        assert!(rows[0].get("remoteMessageId").is_none());
        assert!(rows[0].get("remoteDispatchAttachments").is_none());
        rows
    };

    assert_rejected_send(&runtime);
    let pending = read_pending(&runtime);
    runtime.shutdown();
    drop(runtime);

    let restarted = human_message_fixture_runtime(&root, &url);
    assert_eq!(
        restarted.read_human_conversation_transcript(&id).expect("pure pending reopen"),
        pending
    );
    assert_rejected_send(&restarted);
    let retried_pending = read_pending(&restarted);
    let mut expected_pending = pending;
    expected_pending[0]["lastDispatchFailedAtMs"] =
        retried_pending[0]["lastDispatchFailedAtMs"].clone();
    assert_eq!(retried_pending, expected_pending,
        "retry may update only the failure clock, never the durable nonce or intent");

    // Retry the same ordinary gateway command after correcting the same input.
    // Do not invoke sync: an unrelated history refresh must not change this
    // upload/message/download request-count oracle.
    fs::write(&attachment_path, VIDEO_BYTES).expect("replace with bounded video bytes");
    let sending = {
        let runtime = Arc::clone(&restarted);
        let args = args.clone();
        thread::spawn(move || dispatch(&runtime, "sendHumanMessage", args))
    };
    accepted.recv_timeout(std::time::Duration::from_secs(15))
        .expect("corrected input reaches the first upload");
    release.send(()).expect("release the bounded upload response");
    let settled = sending.join().expect("same-nonce gateway recovery");
    assert_eq!(settled["id"], "human-server-message:92");
    assert_eq!(settled["remoteMessageId"], "92");
    assert_eq!(settled["clientNonce"], NONCE);
    assert_eq!(settled["delivery"], "sent");
    assert_eq!(settled["attachments"][0]["resourceId"], "bounded-video-resource");
    assert_eq!(settled["attachments"][0]["name"], "bounded-video.mp4");
    assert_eq!(settled["attachments"][0]["size"], VIDEO_BYTES.len());
    assert!(settled.get("lastDispatchError").is_none());
    assert!(settled.get("lastDispatchFailedAtMs").is_none());
    assert!(settled.get("remoteDispatchAttachments").is_none());
    let materialized_path = settled["attachments"][0]["path"].as_str()
        .expect("canonical resource path").to_string();
    assert!(materialized_path.ends_with(
        "human-message-resources/bounded-video-resource/bounded-video.mp4"
    ));
    assert_eq!(fs::read(&materialized_path).expect("materialized video bytes"), VIDEO_BYTES);
    assert_eq!(
        restarted.read_human_conversation_transcript(&id).expect("unique canonical row"),
        vec![settled.clone()]
    );
    assert_eq!(
        dispatch(&restarted, "sendHumanMessage", args.clone()),
        settled,
        "repeating the settled nonce must not upload or send again"
    );
    restarted.shutdown();
    drop(restarted);

    let reopened = human_message_fixture_runtime(&root, &url);
    assert_eq!(
        reopened.read_human_conversation_transcript(&id).expect("pure canonical reopen"),
        vec![settled]
    );
    assert_eq!(fs::read(&materialized_path).expect("reopened video bytes"), VIDEO_BYTES);
    reopened.shutdown();
    drop(reopened);

    let requests = server.join().expect("bounded upload recovery fixture");
    assert_eq!(requests.len(), 3,
        "both oversized attempts and both pure reopens must add no HTTP requests");
    assert!(requests[0].starts_with("POST /api/social/message-resources "));
    assert!(requests[1].starts_with("POST /api/social/messages "));
    assert!(requests[2].starts_with("GET /api/social/message-resources/bounded-video-resource "));
    assert!(requests.iter().all(|request| request.to_ascii_lowercase()
        .contains("authorization: bearer fixture-token")));
    assert!(requests[0].contains("filename=\"bounded-video.mp4\""));
    let (upload_headers, multipart) = requests[0].split_once("\r\n\r\n")
        .expect("multipart request");
    let boundary = upload_headers.lines()
        .find(|line| line.to_ascii_lowercase().starts_with("content-type:"))
        .and_then(|line| line.split_once("boundary=").map(|(_, value)| value.trim()))
        .map(|value| value.trim_matches('"'))
        .expect("multipart boundary");
    let (_, file_and_end) = multipart.split_once("\r\n\r\n").expect("file part headers");
    let closing_boundary = format!("\r\n--{boundary}--");
    let (uploaded_bytes, end) = file_and_end.split_once(closing_boundary.as_str())
        .expect("complete file part");
    assert_eq!(uploaded_bytes.as_bytes(), VIDEO_BYTES,
        "only the corrected bytes may cross the HTTP boundary");
    assert!(end.is_empty() || end == "\r\n");
    let submitted = fixture_request_json(&requests[1]);
    assert_eq!(submitted["targetUserId"], "2");
    assert_eq!(submitted["text"], "bounded video upload");
    assert_eq!(submitted["clientRequestId"], NONCE);
    assert_eq!(submitted["attachments"], json!([{"resourceId":"bounded-video-resource"}]));
    let _ = fs::remove_dir_all(root);
}

fn spawn_ambiguous_human_send_server() -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ambiguous send fixture");
    let address = listener.local_addr().expect("fixture address");
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();

        let (mut upload_stream, _) = listener.accept().expect("accept durable resource upload");
        requests.push(read_fixture_http_request(&mut upload_stream));
        let upload_body = json!({
            "success": true,
            "resource": {
                "resourceId": "durable-resource-1",
                "name": "resume.pdf",
                "contentType": "application/octet-stream",
                "size": 4,
                "createdAt": "2026-10-05T02:00:00Z"
            }
        }).to_string();
        write!(
            upload_stream,
            "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            upload_body.len(),
            upload_body
        ).expect("write durable resource response");
        upload_stream.flush().expect("flush durable resource response");

        let (mut first_send, _) = listener.accept().expect("accept ambiguous message send");
        requests.push(read_fixture_http_request(&mut first_send));
        drop(first_send);

        let (mut replay_send, _) = listener.accept().expect("accept replayed message send");
        requests.push(read_fixture_http_request(&mut replay_send));
        let send_body = json!({
            "success": true,
            "deduplicated": true,
            "message": {
                "id": 21,
                "senderUserId": 1,
                "senderUsername": "local",
                "recipientUserId": 2,
                "recipientUsername": "peer",
                "text": "durable restart",
                "clientRequestId": "durable-send-1",
                "createdAt": "2026-10-05T02:00:01Z",
                "readAt": serde_json::Value::Null,
                "isOutgoing": true,
                "replyToMessageId": serde_json::Value::Null,
                "attachments": [{
                    "resourceId": "durable-resource-1",
                    "name": "resume.pdf",
                    "contentType": "application/octet-stream",
                    "size": 4,
                    "createdAt": "2026-10-05T02:00:00Z"
                }],
                "reactions": []
            }
        }).to_string();
        write!(
            replay_send,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            send_body.len(),
            send_body
        ).expect("write replay response");
        replay_send.flush().expect("flush replay response");

        let (mut resource_stream, _) = listener.accept().expect("accept durable resource download");
        requests.push(read_fixture_http_request(&mut resource_stream));
        let resource = [5_u8, 6, 7, 8];
        write!(
            resource_stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            resource.len()
        ).expect("write durable resource download headers");
        resource_stream.write_all(&resource).expect("write durable resource download body");
        resource_stream.flush().expect("flush durable resource download");

        let (mut sync_stream, _) = listener.accept().expect("accept post-settlement sync");
        requests.push(read_fixture_http_request(&mut sync_stream));
        assert!(requests.last().expect("history request").contains("&afterId=0"));
        let sync_body = json!({"success":true,"data":{
            "messages":[],"historyCursor":{"mode":"after-id","afterId":0},"nextAfterId":0
        }}).to_string();
        write!(
            sync_stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            sync_body.len(),
            sync_body
        ).expect("write sync response");
        sync_stream.flush().expect("flush sync response");
        requests
    });
    (format!("http://{address}"), handle)
}

#[test]
fn shipping_human_send_recovers_ambiguous_dispatch_after_restart_without_reupload() {
    let root = temp_root("human-shipping-durable-send");
    let agents = root.join("agents");
    fs::create_dir_all(&root).expect("fixture root");
    let credential_path = root.join("host-credential.json");
    fs::write(
        &credential_path,
        r#"{"accessToken":"fixture-token","deviceId":"device-a","userId":1}"#,
    ).expect("credential");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&credential_path)
            .expect("credential metadata")
            .permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&credential_path, permissions).expect("private credential");
    }
    let attachment_path = root.join("resume.pdf");
    fs::write(&attachment_path, [5_u8, 6, 7, 8]).expect("attachment fixture");
    let (base_url, server) = spawn_ambiguous_human_send_server();
    fs::write(&credential_path, serde_json::json!({
        "accessToken": "fixture-token", "deviceId": "device-a", "userId": 1, "issuer": &base_url
    }).to_string()).expect("issuer-bound fixture credential");

    let make_runtime = || {
        let client = Arc::new(
            FabushiNativeMessagingClient::new(&base_url, &credential_path)
                .expect("shipping messaging client"),
        );
        let memory = Arc::new(
            mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(
                agents.clone(),
            ),
        );
        ProductionSessionWorkers::with_agents_root_identity_messaging_and_dependencies(
            &agents,
            500,
            Some("1".into()),
            Some(client),
            true,
            None,
            Arc::new(|| None),
            memory,
        )
    };

    let runtime = Arc::new(make_runtime());
    let created = dispatch(
        &runtime,
        "createHumanConversation",
        json!({"localHumanId":"1", "peerHumanId":"2", "title":"Peer"}),
    );
    assert_eq!(created["kind"], "human");
    let conversation_id = created["id"]
        .as_str()
        .expect("conversation id")
        .to_string();
    let events = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let captured = Arc::clone(&events);
    let event_id = conversation_id.clone();
    let subscription = mahayana_host_runtime::transcript_mutation_events::subscribe_transcript_mutations(move |mutation| {
        if mutation.get("agentId").and_then(serde_json::Value::as_str) == Some(event_id.as_str())
            && mutation.get("entries").and_then(serde_json::Value::as_array).is_some_and(|entries| {
                entries.iter().any(|entry| entry["clientNonce"].as_str() == Some("durable-send-1"))
            })
        {
            captured.lock().expect("creation recovery event capture")
                .push(serde_json::Value::Object(mutation.clone()));
        }
    });
    let first_error = runtime
        .append_human_message(
            &conversation_id,
            "durable restart",
            "durable-send-1",
            Some(2000.0),
            None,
            &[json!({
                "path": attachment_path.to_string_lossy(),
                "name": "resume.pdf"
            })],
        )
        .expect_err("lost response must remain unsettled");
    assert!(!first_error.is_empty());

    let pending_owner = runtime
        .open_human_conversation_db_owner(&conversation_id)
        .expect("pending owner");
    let pending = pending_owner
        .get_transcript_entries()
        .expect("pending transcript");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["id"], "human-message:durable-send-1");
    assert_eq!(pending[0]["delivery"], "pending");
    assert_eq!(
        pending[0]["remoteDispatchAttachments"][0]["resourceId"],
        "durable-resource-1"
    );
    assert!(events.lock().unwrap().iter().any(|event| {
        event["entries"].as_array().is_some_and(|entries| entries.iter().any(|entry| {
            entry["clientNonce"] == "durable-send-1" && entry["delivery"] == "pending"
        }))
    }), "gateway-created conversation publishes its durable pending message");
    runtime.shutdown();
    drop(pending_owner);
    drop(runtime);

    let restarted = Arc::new(make_runtime());
    assert_eq!(dispatch(&restarted, "createHumanConversation",
        json!({"localHumanId":"1", "peerHumanId":"2", "title":"Must keep persisted title"})), created);
    let settled = restarted
        .sync_human_conversation(&conversation_id)
        .expect("restart sync settles pending send");
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0]["id"], "human-server-message:21");
    assert_eq!(settled[0]["remoteMessageId"], "21");
    assert_eq!(settled[0]["delivery"], "sent");
    assert_eq!(
        settled[0]["attachments"][0]["resourceId"],
        "durable-resource-1"
    );
    assert_eq!(settled[0].get("remoteDispatchAttachments"), None);
    assert!(events.lock().unwrap().iter().any(|event| {
        event["entries"].as_array().is_some_and(|entries| entries.iter().any(|entry| {
            entry["clientNonce"] == "durable-send-1" && entry["remoteMessageId"] == "21"
                && entry["delivery"] == "sent"
        }))
    }), "canonical settlement publishes the recovered message from the same owner");
    subscription.unsubscribe();
    restarted.shutdown();
    drop(restarted);

    let requests = server.join().expect("durable send fixture");
    assert_eq!(requests.len(), 5);
    assert!(requests[0].starts_with("POST /api/social/message-resources "));
    assert!(requests[1].starts_with("POST /api/social/messages "));
    assert!(requests[2].starts_with("POST /api/social/messages "));
    assert!(requests[2].contains(r#""clientRequestId":"durable-send-1""#));
    assert!(requests[2].contains(r#""resourceId":"durable-resource-1""#));
    assert!(requests[3].starts_with("GET /api/social/message-resources/durable-resource-1 "));
    assert!(requests[4].starts_with("GET /api/social/messages?"));
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("POST /api/social/message-resources "))
            .count(),
        1
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_reaction_toggles_backend_truth_and_updates_canonical_transcript() {
    let root = temp_root("human-shipping-reaction");
    let agents = root.join("agents");
    fs::create_dir_all(&root).expect("fixture root");
    let credential_path = root.join("host-credential.json");
    fs::write(
        &credential_path,
        r#"{"accessToken":"fixture-token","deviceId":"device-a","userId":1}"#,
    )
    .expect("credential");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&credential_path)
            .expect("credential metadata")
            .permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&credential_path, permissions).expect("private credential");
    }

    let (base_url, server) = spawn_remote_reaction_server();
    fs::write(&credential_path, serde_json::json!({
        "accessToken": "fixture-token", "deviceId": "device-a", "userId": 1, "issuer": &base_url
    }).to_string()).expect("issuer-bound fixture credential");
    let client = Arc::new(
        FabushiNativeMessagingClient::new(&base_url, &credential_path)
            .expect("shipping messaging client"),
    );
    let memory = Arc::new(
        mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(
            agents.clone(),
        ),
    );
    let runtime = Arc::new(
        ProductionSessionWorkers::with_agents_root_identity_messaging_and_dependencies(
            &agents,
            500,
            Some("1".into()),
            Some(client),
            true,
            None,
            Arc::new(|| None),
            memory,
        ),
    );
    let created = runtime
        .create_human_conversation("2", "Peer")
        .expect("Human conversation");
    let conversation_id = created["id"]
        .as_str()
        .expect("conversation id")
        .to_string();
    let owner = runtime
        .open_human_conversation_db_owner(&conversation_id)
        .expect("conversation owner");
    assert!(owner
        .append_transcript_entry(&json!({
            "id": "human-server-message:11",
            "kind": "message",
            "role": "user",
            "authorKind": "human",
            "authorId": "2",
            "content": "react to me",
            "timestampMs": 1000.0,
            "delivery": "sent",
            "remoteMessageId": "11",
            "remoteCreatedAt": "2026-10-05T01:00:01Z",
            "remoteReactions": [],
            "reactions": []
        }))
        .expect("remote message"));

    let reacted = dispatch(
        &runtime,
        "reactHumanMessage",
        json!({
            "conversationId": conversation_id,
            "entryId": "human-server-message:11",
            "emoji": "👍"
        }),
    );
    assert_eq!(reacted["remoteReactions"][0]["reactedByMe"], true);
    assert_eq!(reacted["remoteReactions"][0]["count"], 2);
    assert_eq!(
        reacted["reactions"],
        json!([
            {"emoji": "👍", "by": "me"},
            {"emoji": "👍", "by": "2"}
        ])
    );

    let unreacted = dispatch(
        &runtime,
        "reactHumanMessage",
        json!({
            "conversationId": conversation_id,
            "entryId": "human-server-message:11",
            "emoji": "👍"
        }),
    );
    assert_eq!(unreacted["remoteReactions"][0]["reactedByMe"], false);
    assert_eq!(unreacted["reactions"], json!([{"emoji": "👍", "by": "2"}]));
    let transcript = dispatch(
        &runtime,
        "getHumanConversationTranscript",
        json!({"conversationId": conversation_id}),
    );
    assert_eq!(transcript[0], unreacted);

    let requests = server.join().expect("reaction fixture");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("POST /api/social/messages/11/reactions "));
    assert!(requests[0].contains(r#""active":true"#));
    assert!(requests[1].contains(r#""active":false"#));
    assert!(requests[0].contains(r#""emoji":"👍""#));

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_sync_recovers_multi_device_gap_until_known_remote_overlap() {
    let root = temp_root("human-multi-device-gap");
    let agents = root.join("agents");
    fs::create_dir_all(&root).expect("fixture root");
    let credential_path = root.join("host-credential.json");
    fs::write(
        &credential_path,
        r#"{"accessToken":"fixture-token","deviceId":"device-a","userId":1}"#,
    )
    .expect("credential");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&credential_path)
            .expect("credential metadata")
            .permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&credential_path, permissions).expect("private credential");
    }

    let (base_url, server) = spawn_remote_history_server();
    fs::write(&credential_path, serde_json::json!({
        "accessToken": "fixture-token", "deviceId": "device-a", "userId": 1, "issuer": &base_url
    }).to_string()).expect("issuer-bound fixture credential");
    let client = Arc::new(
        FabushiNativeMessagingClient::new(&base_url, &credential_path)
            .expect("shipping messaging client"),
    );
    let memory = Arc::new(
        mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(
            agents.clone(),
        ),
    );
    let runtime = Arc::new(
        ProductionSessionWorkers::with_agents_root_identity_messaging_and_dependencies(
            &agents,
            500,
            Some("1".into()),
            Some(client),
            true,
            None,
            Arc::new(|| None),
            memory,
        ),
    );
    let created = runtime
        .create_human_conversation("2", "Peer")
        .expect("Human conversation");
    let conversation_id = created["id"]
        .as_str()
        .expect("conversation id")
        .to_string();
    let owner = runtime
        .open_human_conversation_db_owner(&conversation_id)
        .expect("conversation owner");
    let anchor = json!({
        "id": "human-server-message:1",
        "kind": "message",
        "role": "user",
        "authorKind": "human",
        "authorId": "2",
        "content": "remote-1",
        "timestampMs": 1000.0,
        "delivery": "sent",
        "remoteMessageId": "1",
        "remoteCreatedAt": "1970-01-01T00:00:01Z"
    });
    assert!(owner.append_transcript_entry(&anchor).expect("anchor"));

    let transcript = runtime
        .sync_human_conversation(&conversation_id)
        .expect("bounded reconnect recovery");
    assert_eq!(transcript.len(), 401);
    assert_eq!(transcript[0]["id"], "human-server-message:1");
    assert_eq!(transcript[1]["id"], "human-server-message:2");
    assert_eq!(transcript[400]["id"], "human-server-message:401");
    assert_eq!(
        owner
            .get_metadata("lastActivityAt")
            .expect("last activity"),
        Some(json!(401000.0))
    );

    let requests = server.join().expect("messaging fixture");
    assert_eq!(requests.len(), 4);
    assert!(requests[0].starts_with("GET /api/social/messages?contactId=2&limit=200&afterId=0 "));
    assert!(requests[0].contains("&afterId=0"));
    assert!(requests[1].contains("&afterId=200"));
    assert!(requests[2].contains("&afterId=400"));
    assert!(requests[3].starts_with("GET /api/social/messages?contactId=2&limit=200 "));

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}





// NMR-001: real Session workers + the existing shipping HTTP boundary.
// Each request is scripted explicitly; unexpected resource fetches fail rather
// than silently passing a string/regex-only source inspection.
enum HumanMessageFixtureReply {
    Json(u16, serde_json::Value),
    Bytes(u16, Vec<u8>),
    Disconnect,
    Truncated(u16),
    Deferred(
        Box<HumanMessageFixtureReply>,
        std::sync::mpsc::Sender<()>,
        std::sync::mpsc::Receiver<()>,
    ),
    // NMR-REACTION-RECOVERY-01: hold a real response after headers and one
    // body byte, and keep the listener observable after the scripted prefix.
    StreamingJson(
        u16,
        serde_json::Value,
        std::sync::mpsc::Sender<()>,
        std::sync::mpsc::Receiver<()>,
    ),
    ObserveUntilStopped(std::sync::mpsc::Receiver<()>),
}

fn spawn_human_message_fixture(
    responses: Vec<HumanMessageFixtureReply>,
) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind Human message fixture");
    listener.set_nonblocking(true).expect("bound Human fixture accept");
    let address = listener.local_addr().expect("Human fixture address");
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        let mut deferred = Vec::new();
        for (response_index, response) in responses.into_iter().enumerate() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            if let HumanMessageFixtureReply::ObserveUntilStopped(stopped) = &response {
                loop {
                    assert!(std::time::Instant::now() < deadline, "live Human fixture observer was not stopped");
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            stream.set_nonblocking(false).expect("blocking unexpected Human request");
                            stream.set_write_timeout(Some(std::time::Duration::from_secs(15)))
                                .expect("bound unexpected Human response");
                            requests.push(read_fixture_http_request_before(&mut stream, Some(deadline)));
                            let body = json!({"success":false,"error":"unexpected downstream Human recovery request"}).to_string();
                            write!(stream,
                                "HTTP/1.1 503 Unexpected\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                body.len(), body,
                            ).expect("answer unexpected Human recovery request");
                            stream.flush().expect("flush unexpected Human recovery response");
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            match stopped.recv_timeout(std::time::Duration::from_millis(5)) {
                                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                            }
                        }
                        Err(error) => panic!("live Human recovery observer accept: {error}"),
                    }
                }
                continue;
            }
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(std::time::Instant::now() < deadline,
                            "Human fixture timed out waiting for response {response_index}; received {} requests",
                            requests.len());
                        thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(error) => panic!("Human fixture accept failed for response {response_index}: {error}"),
                }
            };
            stream.set_nonblocking(false).expect("restore blocking Human fixture stream");
            stream.set_write_timeout(Some(std::time::Duration::from_secs(15)))
                .expect("bound Human fixture response write");
            requests.push(read_fixture_http_request_before(&mut stream, Some(deadline)));
            let after_id = requests.last().and_then(|request| request.split("afterId=").nth(1))
                .and_then(|tail| tail.split(|character: char| !character.is_ascii_digit()).next())
                .and_then(|value| value.parse::<u64>().ok());
            let (response, release, streaming_body) = match response {
                HumanMessageFixtureReply::Deferred(response, accepted, release) => {
                    accepted.send(()).expect("observe accepted deferred request");
                    (*response, Some(release), None)
                }
                HumanMessageFixtureReply::StreamingJson(status, body, started, release) => {
                    (HumanMessageFixtureReply::Json(status, body), None, Some((started, release)))
                }
                response => (response, None, None),
            };
            let is_deferred = release.is_some() || streaming_body.is_some();
            let respond = move || {
                if let Some(release) = release {
                    release.recv_timeout(std::time::Duration::from_secs(15))
                        .expect("deferred Human response must be explicitly released");
                }
                let (status, content_type, body) = match response {
                    HumanMessageFixtureReply::Json(status, mut body) => {
                        // Existing pre-cursor cases model a compatible backend response.
                        // Tests that explicitly supply null or a conflicting cursor
                        // bypass this fixture default and assert fail-closed behavior.
                        if status == 200 {
                            if let (Some(after), Some(data)) = (
                                after_id, body.get_mut("data").and_then(serde_json::Value::as_object_mut)
                            ) {
                                if let Some(messages) = data.get("messages").and_then(serde_json::Value::as_array) {
                                    let next = messages.last().and_then(|message| message.get("id"))
                                        .cloned().unwrap_or(json!(after));
                                    data.entry("historyCursor").or_insert_with(|| json!({"mode":"after-id","afterId":after}));
                                    data.entry("nextAfterId").or_insert(next);
                                }
                            }
                        }
                        (status, "application/json", body.to_string().into_bytes())
                    },
                    HumanMessageFixtureReply::Bytes(status, bytes) => (
                        status, "application/octet-stream", bytes,
                    ),
                    HumanMessageFixtureReply::Disconnect => return,
                    HumanMessageFixtureReply::Truncated(status) => {
                        write!(stream, "HTTP/1.1 {status} Incomplete\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{{}}")
                            .expect("write deliberately truncated response");
                        stream.flush().expect("flush truncated response");
                        return;
                    }
                    HumanMessageFixtureReply::Deferred(..)
                    | HumanMessageFixtureReply::StreamingJson(..)
                    | HumanMessageFixtureReply::ObserveUntilStopped(..) =>
                        panic!("nested or misplaced Human fixture control"),
                };
                let reason = match status {
                    200 => "OK",
                    201 => "Created",
                    503 => "Service Unavailable",
                    _ => "Error",
                };
                write!(
                    stream,
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                ).expect("write Human response headers");
                if let Some((started, release)) = streaming_body {
                    assert!(!body.is_empty(), "streamed JSON has a first byte");
                    stream.write_all(&body[..1]).expect("first Human response body byte");
                    stream.flush().expect("flush incomplete Human response body");
                    started.send(()).expect("observe incomplete Human response");
                    release.recv_timeout(std::time::Duration::from_secs(15))
                        .expect("release remaining Human response body");
                    if let Err(error) = stream.write_all(&body[1..]).and_then(|_| stream.flush()) {
                        assert!(matches!(error.kind(),
                            std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                        ), "unexpected streamed Human response failure: {error}");
                    }
                } else {
                    stream.write_all(&body).expect("write Human response body");
                    stream.flush().expect("flush Human fixture response");
                }
            };
            if is_deferred {
                deferred.push(thread::spawn(respond));
            } else {
                respond();
            }
        }
        for writer in deferred {
            writer.join().expect("deferred Human response writer");
        }
        requests
    });
    (format!("http://{address}"), handle)
}

fn human_message_fixture_runtime(
    root: &std::path::Path,
    base_url: &str,
) -> Arc<ProductionSessionWorkers> {
    let agents = root.join("agents");
    fs::create_dir_all(&agents).expect("Human fixture agent root");
    let credential_path = root.join("host-credential.json");
    fs::write(&credential_path, json!({
        "accessToken": "fixture-token",
        "deviceId": "device-a",
        "userId": 1,
        "issuer": base_url,
    }).to_string()).expect("write Human fixture credential");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&credential_path).expect("credential metadata").permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&credential_path, perms).expect("private Human credential");
    }
    let client = Arc::new(
        FabushiNativeMessagingClient::new(base_url, &credential_path)
            .expect("shipping Human messaging client")
    );
    let memory = Arc::new(
        mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(
            agents.clone()
        )
    );
    Arc::new(ProductionSessionWorkers::with_agents_root_identity_messaging_and_dependencies(
        &agents,
        500,
        Some("1".into()),
        Some(client),
        true,
        None,
        Arc::new(|| None),
        memory,
    ))
}

fn fixture_remote_human_message(
    id: u64,
    sender: u64,
    recipient: u64,
    text: &str,
    nonce: Option<&str>,
    attachments: serde_json::Value,
) -> serde_json::Value {
    json!({
        "id": id,
        "senderUserId": sender,
        "senderUsername": if sender == 1 { "local" } else { "peer" },
        "recipientUserId": recipient,
        "recipientUsername": if recipient == 1 { "local" } else { "peer" },
        "text": text,
        "clientRequestId": nonce,
        "createdAt": format!("2026-10-05T01:00:{:02}Z", id % 60),
        "readAt": null,
        "isOutgoing": sender == 1,
        "replyToMessageId": null,
        "attachments": attachments,
        "reactions": []
    })
}

#[test]
fn shipping_human_known_refresh_preserves_omitted_reactions_and_clears_explicit_empty() {
    let root = temp_root("human-known-refresh");
    let mut initial = fixture_remote_human_message(51, 2, 1, "canonical", None, json!([]));
    initial["reactions"] = json!([{"emoji":"👍","count":1,"reactedByMe":false}]);
    let mut partial = initial.clone();
    partial.as_object_mut().expect("message object").remove("reactions");
    partial["readAt"] = json!("2026-10-05T02:00:00Z");
    let mut clear = partial.clone();
    clear["reactions"] = json!([]);
    clear["readAt"] = json!("2026-10-05T01:30:00Z");
    let pages = [initial, partial, clear].into_iter().map(|message|
        HumanMessageFixtureReply::Json(200, json!({
            "success":true, "data":{"messages":[message]}
        }))
    ).collect::<Vec<_>>();
    let (url, server) = spawn_human_message_fixture(pages);
    let runtime = human_message_fixture_runtime(&root, &url);
    let conversation_id = runtime.create_human_conversation("2","Peer").expect("conversation")["id"]
        .as_str().expect("id").to_string();
    let first = runtime.sync_human_conversation(&conversation_id).expect("initial");
    assert_eq!(first.len(),1);
    assert_eq!(first[0]["reactions"],json!([{"emoji":"👍","by":"2"}]));
    let second = runtime.sync_human_conversation(&conversation_id).expect("partial refresh");
    assert_eq!(second.len(),1,"known row has stable identity");
    assert_eq!(second[0]["reactions"],first[0]["reactions"]);
    assert_eq!(second[0]["readAt"],json!("2026-10-05T02:00:00Z"));
    let third = runtime.sync_human_conversation(&conversation_id).expect("explicit removal");
    assert_eq!(third.len(),1);
    assert_eq!(third[0]["reactions"],json!([]));
    assert_eq!(third[0]["readAt"],second[0]["readAt"],"read state is monotonic");
    runtime.shutdown();
    let reopened = human_message_fixture_runtime(&root,&url);
    let owner = reopened.open_human_conversation_db_owner(&conversation_id).expect("reopen owner");
    let stored = owner.get_transcript_entries().expect("durable entries");
    assert_eq!(stored.len(),1);
    assert_eq!(stored[0]["reactions"],json!([]));
    assert_eq!(stored[0]["readAt"],second[0]["readAt"]);
    let requests = server.join().expect("fixture requests");
    assert_eq!(requests.len(),3);
    reopened.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_sync_rejects_foreign_participants_before_resources_or_overlap() {
    let root = temp_root("nmr-foreign");
    let forbidden_resource = json!([{
        "resourceId": "foreign-resource",
        "name": "private.txt",
        "contentType": "text/plain",
        "size": 4,
        "createdAt": "2026-10-05T01:00:10Z"
    }]);
    let rows = json!([
        fixture_remote_human_message(10, 1, 3, "wrong peer", Some("foreign"), forbidden_resource),
        fixture_remote_human_message(12, 2, 1, "valid independent row", Some("peer-request"), json!([])),
    ]);
    let (base_url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":rows}}))
    ]);
    let runtime = human_message_fixture_runtime(&root, &base_url);
    let id = runtime.create_human_conversation("2", "Peer")
        .expect("Human conversation")["id"].as_str().expect("id").to_owned();
    let owner = runtime.open_human_conversation_db_owner(&id).expect("conversation owner");
    let anchor = json!({
        "id":"human-server-message:10", "kind":"message","role":"user",
        "authorKind":"human","authorId":"2","content":"old anchor",
        "timestampMs": 1000.0,"delivery":"sent","remoteMessageId":"10",
        "remoteCreatedAt":"2026-10-05T01:00:10Z"
    });
    assert!(owner.append_transcript_entry(&anchor).expect("anchor"));
    let error = runtime.sync_human_conversation(&id)
        .expect_err("foreign row must not be accepted");
    assert!(error.contains("selected Human conversation"), "{error}");
    let state = owner.get_transcript_entries().expect("durable transcript");
    assert_eq!(state.len(), 2, "valid row must progress despite independent rejection");
    assert_eq!(state[0], anchor);
    assert_eq!(state[1]["id"], "human-server-message:12");
    assert_eq!(state[1]["remoteClientRequestId"], "peer-request");
    assert!(state[1].get("clientNonce").is_none());
    let requests = server.join().expect("Human fixture");
    assert_eq!(requests.len(), 1, "foreign attachment must never be fetched");
    assert!(requests[0].starts_with("GET /api/social/messages?"));
    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_sync_keeps_peer_nonce_separate_through_local_settlement_and_restart() {
    let root = temp_root("nmr-peer-nonce");
    let incoming = fixture_remote_human_message(13, 2, 1, "peer text", Some("shared-nonce"), json!([]));
    let outgoing = fixture_remote_human_message(14, 1, 2, "local text", Some("shared-nonce"), json!([]));
    let failed_dispatch = json!({"success":false,"error":"temporary dispatch failure"});
    let (base_url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(503, failed_dispatch.clone()),
        HumanMessageFixtureReply::Json(503, failed_dispatch),
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[incoming]}})),
        HumanMessageFixtureReply::Json(201, json!({"success":true,"deduplicated":false,"message":outgoing})),
    ]);
    let runtime = human_message_fixture_runtime(&root, &base_url);
    let id = runtime.create_human_conversation("2", "Peer")
        .expect("Human conversation")["id"].as_str().expect("id").to_owned();

    runtime.append_human_message(
        &id, "local text", "shared-nonce", Some(1800.0), None, &[],
    ).expect_err("failed dispatch must leave a durable local intent before peer sync");
    let pending_rows = runtime.read_human_conversation_transcript(&id).expect("pending transcript");
    assert_eq!(pending_rows.len(), 1);
    assert_eq!(pending_rows[0]["id"], "human-message:shared-nonce");
    assert_eq!(pending_rows[0]["authorId"], "1");
    assert_eq!(pending_rows[0]["clientNonce"], "shared-nonce");
    assert_eq!(pending_rows[0]["content"], "local text");
    assert_eq!(pending_rows[0]["delivery"], "pending");
    assert!(pending_rows[0].get("remoteMessageId").is_none());

    let peer_rows = runtime.sync_human_conversation(&id)
        .expect("peer sync progresses despite a separate failed local retry");
    assert_eq!(peer_rows.len(), 2, "peer provenance must not settle or replace the pending local row");
    // The retry may update its failure clock; every other pending field remains identical.
    assert!(peer_rows[0]["lastDispatchFailedAtMs"].as_f64()
        .is_some_and(|timestamp| timestamp.is_finite() && timestamp >= 0.0));
    let mut expected_pending = pending_rows[0].clone();
    expected_pending["lastDispatchFailedAtMs"] = peer_rows[0]["lastDispatchFailedAtMs"].clone();
    assert_eq!(peer_rows[0], expected_pending);
    assert_eq!(peer_rows[1]["id"], "human-server-message:13");
    assert_eq!(peer_rows[1]["authorId"], "2");
    assert_eq!(peer_rows[1]["content"], "peer text");
    assert_eq!(peer_rows[1]["remoteClientRequestId"], "shared-nonce");
    assert!(peer_rows[1].get("clientNonce").is_none());
    runtime.shutdown();
    drop(runtime);

    let restarted = human_message_fixture_runtime(&root, &base_url);
    assert_eq!(
        restarted.read_human_conversation_transcript(&id).expect("restart pending and peer"),
        peer_rows,
    );
    let sent = restarted.append_human_message(
        &id, "local text", "shared-nonce", Some(1800.0), None, &[],
    ).expect("only the matching local acknowledgement may settle the pending intent");
    assert_eq!(sent["id"], "human-server-message:14");
    assert_eq!(sent["authorId"], "1");
    assert_eq!(sent["clientNonce"], "shared-nonce");
    assert_eq!(sent["content"], "local text");
    assert_eq!(sent["delivery"], "sent");
    assert!(sent.get("lastDispatchError").is_none());
    let entries = restarted.read_human_conversation_transcript(&id).expect("settled transcript");
    assert_eq!(entries, vec![peer_rows[1].clone(), sent.clone()],
        "canonical Human order follows authoritative timestamps without merging the peer nonce");
    assert_eq!(restarted.open_human_conversation_db_owner(&id).expect("physical owner")
        .get_transcript_entries().expect("stable physical identities"), vec![sent, peer_rows[1].clone()],
        "settlement retains the original physical slot and the complete peer row");
    restarted.shutdown();
    drop(restarted);

    let reopened = human_message_fixture_runtime(&root, &base_url);
    assert_eq!(reopened.read_human_conversation_transcript(&id).expect("restart settled rows"), entries);
    reopened.shutdown();
    let requests = server.join().expect("Human fixture");
    assert_eq!(requests.len(), 4);
    assert!(requests[0].starts_with("POST /api/social/messages "));
    assert!(requests[1].starts_with("POST /api/social/messages "));
    assert!(requests[2].starts_with("GET /api/social/messages?"));
    assert!(requests[3].starts_with("POST /api/social/messages "));
    assert_eq!(fixture_request_json(&requests[0]), fixture_request_json(&requests[1]));
    assert_eq!(fixture_request_json(&requests[0]), fixture_request_json(&requests[3]));
    assert_eq!(fixture_request_json(&requests[3])["clientRequestId"], "shared-nonce");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_sync_rejects_conflicting_local_acknowledgements_before_resource_access() {
    for (difference, expected_error) in [
        ("text", "remote Human acknowledgement conflicts with the local send identity or text"),
        ("reply", "remote Human acknowledgement changed the dispatched reply"),
        ("resource-id", "remote Human acknowledgement changed dispatched resources"),
        ("resource-order", "remote Human acknowledgement changed dispatched resources"),
        ("missing-resource", "remote Human acknowledgement changed dispatched resources"),
    ] {
        let root = temp_root(&format!("nmr-conflict-{difference}"));
        let dispatched = json!([
            {
                "resourceId":"expected-resource-a","name":"first.txt","contentType":"text/plain",
                "size":4,"createdAt":"2026-10-05T01:00:19Z"
            },
            {
                "resourceId":"expected-resource-b","name":"second.txt","contentType":"text/plain",
                "size":4,"createdAt":"2026-10-05T01:00:20Z"
            },
        ]);
        let mut matching_ack = fixture_remote_human_message(
            21, 1, 2, "expected text", Some("pending-nonce"), dispatched.clone(),
        );
        matching_ack["replyToMessageId"] = json!(10);
        let mut conflict = matching_ack.clone();
        // Each rejected response differs in exactly one immutable acknowledgement field.
        match difference {
            "text" => conflict["text"] = json!("wrong text"),
            "reply" => conflict["replyToMessageId"] = json!(11),
            "resource-id" => conflict["attachments"][0]["resourceId"] = json!("untrusted-conflict"),
            "resource-order" => conflict["attachments"].as_array_mut().expect("resources").reverse(),
            "missing-resource" => {
                conflict["attachments"].as_array_mut().expect("resources").pop();
            }
            _ => unreachable!(),
        }
        let (base_url, server) = spawn_human_message_fixture(vec![
            HumanMessageFixtureReply::Json(503, json!({"success":false,"error":"temporary dispatch failure"})),
            HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[conflict]}})),
            HumanMessageFixtureReply::Json(201, json!({"success":true,"deduplicated":true,"message":matching_ack})),
            HumanMessageFixtureReply::Bytes(200, vec![1, 2, 3, 4]),
            HumanMessageFixtureReply::Bytes(200, vec![5, 6, 7, 8]),
        ]);
        let runtime = human_message_fixture_runtime(&root, &base_url);
        let id = runtime.create_human_conversation("2","Peer")
            .expect("Human conversation")["id"].as_str().expect("id").to_owned();
        let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
        let anchor = json!({
            "id":"human-server-message:10","kind":"message","role":"user",
            "authorKind":"human","authorId":"2","content":"reply anchor",
            "timestampMs":500.0,"delivery":"sent","remoteMessageId":"10",
            "remoteCreatedAt":"2026-10-05T01:00:10Z"
        });
        let attachments = vec![
            json!({"path":root.join("first.txt").to_string_lossy(),"name":"first.txt"}),
            json!({"path":root.join("second.txt").to_string_lossy(),"name":"second.txt"}),
        ];
        fs::write(root.join("first.txt"), [1_u8, 2, 3, 4]).expect("first local attachment");
        fs::write(root.join("second.txt"), [5_u8, 6, 7, 8]).expect("second local attachment");
        let pending = json!({
            "id":"human-message:pending-nonce","kind":"message","role":"user",
            "authorKind":"human","authorId":"1","clientNonce":"pending-nonce",
            "content":"expected text","composedAtMs":1000.0,"timestampMs":1000.0,
            "delivery":"pending","replyToId":"human-server-message:10",
            "attachments":attachments,"remoteDispatchAttachments":dispatched
        });
        assert!(owner.append_transcript_entry(&anchor).expect("reply anchor"));
        assert!(owner.append_transcript_entry(&pending).expect("durable pending dispatch"));

        let error = runtime.sync_human_conversation(&id).expect_err("conflicting local ack");
        assert_eq!(error, expected_error, "{difference} must reach its own admission guard");
        let rows = owner.get_transcript_entries().expect("durable rows after rejection");
        assert_eq!(rows.len(), 2, "{difference} must not append a conflicting canonical row");
        assert_eq!(rows[0], anchor);
        assert!(rows[1]["lastDispatchError"].as_str().is_some());
        assert!(rows[1]["lastDispatchFailedAtMs"].as_f64()
            .is_some_and(|timestamp| timestamp.is_finite() && timestamp >= 0.0));
        let mut expected_pending = pending.clone();
        expected_pending["lastDispatchError"] = rows[1]["lastDispatchError"].clone();
        expected_pending["lastDispatchFailedAtMs"] = rows[1]["lastDispatchFailedAtMs"].clone();
        assert_eq!(rows[1], expected_pending, "{difference} must preserve the complete pending intent");
        assert!(rows[1].get("remoteMessageId").is_none());
        runtime.shutdown();
        drop(owner);
        drop(runtime);

        let restarted = human_message_fixture_runtime(&root, &base_url);
        assert_eq!(
            restarted.read_human_conversation_transcript(&id).expect("restart rejected pending"),
            rows,
            "{difference} must remain rejected durably",
        );
        let sent = restarted.append_human_message(
            &id, "expected text", "pending-nonce", Some(1000.0),
            Some("human-server-message:10"), &attachments,
        ).expect("a fully matching acknowledgement must recover the same pending intent");
        assert_eq!(sent["id"], "human-server-message:21");
        assert_eq!(sent["clientNonce"], "pending-nonce");
        assert_eq!(sent["content"], "expected text");
        assert_eq!(sent["replyToId"], "human-server-message:10");
        assert_eq!(sent["remoteReplyToMessageId"], "10");
        assert_eq!(sent["delivery"], "sent");
        assert_eq!(sent["attachments"][0]["resourceId"], "expected-resource-a");
        assert_eq!(sent["attachments"][1]["resourceId"], "expected-resource-b");
        assert_eq!(sent["attachments"].as_array().expect("settled resources").len(), 2);
        assert!(sent.get("remoteDispatchAttachments").is_none());
        assert!(sent.get("lastDispatchError").is_none());
        assert_eq!(
            fs::read(sent["attachments"][0]["path"].as_str().expect("first resource path"))
                .expect("first materialized bytes"),
            [1_u8, 2, 3, 4],
        );
        assert_eq!(
            fs::read(sent["attachments"][1]["path"].as_str().expect("second resource path"))
                .expect("second materialized bytes"),
            [5_u8, 6, 7, 8],
        );
        let settled_rows = restarted.read_human_conversation_transcript(&id).expect("recovered transcript");
        assert_eq!(settled_rows, vec![anchor, sent]);
        restarted.shutdown();
        drop(restarted);

        let reopened = human_message_fixture_runtime(&root, &base_url);
        assert_eq!(reopened.read_human_conversation_transcript(&id).expect("restart recovered rows"), settled_rows);
        reopened.shutdown();
        let requests = server.join().expect("Human fixture");
        assert_eq!(requests.len(), 5, "{difference} must be rejected before any resource download");
        assert!(requests[0].starts_with("POST /api/social/messages "));
        assert!(requests[1].starts_with("GET /api/social/messages?"));
        assert!(requests[2].starts_with("POST /api/social/messages "));
        assert_eq!(fixture_request_json(&requests[0]), fixture_request_json(&requests[2]));
        assert!(requests[3].starts_with("GET /api/social/message-resources/expected-resource-a "));
        assert!(requests[4].starts_with("GET /api/social/message-resources/expected-resource-b "));
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn shipping_human_sync_rejects_canonical_ack_duplicates_with_missing_or_changed_nonce() {
    for nonce in [None, Some("changed")] {
        let root = temp_root("nmr-duplicate-canonical-id");
        let matching_ack = fixture_remote_human_message(
            21, 1, 2, "already sent", Some("original"), json!([]),
        );
        let mut conflict = matching_ack.clone();
        conflict["clientRequestId"] = json!(nonce);
        let (base_url, server) = spawn_human_message_fixture(vec![
            HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[conflict]}})),
            HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[matching_ack]}})),
        ]);
        let runtime = human_message_fixture_runtime(&root, &base_url);
        let id = runtime.create_human_conversation("2","Peer")
            .expect("Human conversation")["id"].as_str().expect("id").to_owned();
        let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
        let settled = json!({
            "id":"human-server-message:21","kind":"message","role":"user",
            "authorKind":"human","authorId":"1","clientNonce":"original",
            "content":"already sent","timestampMs":1000.0,"delivery":"sent",
            "remoteMessageId":"21","remoteCreatedAt":"2026-10-05T01:00:21Z"
        });
        assert!(owner.append_transcript_entry(&settled).expect("settled row"));
        let error = runtime.sync_human_conversation(&id)
            .expect_err("canonical id does not excuse a nonce mismatch on its own");
        assert_eq!(error, "remote Human acknowledgement conflicts with the local send identity or text");
        assert_eq!(owner.get_transcript_entries().expect("rows"), vec![settled.clone()]);
        runtime.shutdown();
        drop(owner);
        drop(runtime);

        let restarted = human_message_fixture_runtime(&root, &base_url);
        assert_eq!(
            restarted.read_human_conversation_transcript(&id).expect("restart canonical row"),
            vec![settled.clone()],
        );
        let mut refreshed = settled;
        refreshed["authorName"] = json!("local");
        refreshed["timestampMs"] = json!(1791162021000.0);
        refreshed["reactions"] = json!([]);
        refreshed["remoteReactions"] = json!([]);
        assert_eq!(
            restarted.sync_human_conversation(&id).expect("unchanged nonce admits an authoritative refresh"),
            vec![refreshed.clone()],
        );
        restarted.shutdown();
        drop(restarted);

        let reopened = human_message_fixture_runtime(&root, &base_url);
        assert_eq!(
            reopened.read_human_conversation_transcript(&id).expect("restart refreshed canonical row"),
            vec![refreshed],
        );
        reopened.shutdown();
        let requests = server.join().expect("Human fixture");
        assert_eq!(requests.len(), 2, "nonce admission must not dispatch or fetch resources");
        assert!(requests.iter().all(|request| request.starts_with("GET /api/social/messages?")));
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn shipping_human_legacy_peer_nonce_is_safe_in_reactions_and_local_send() {
    let root = temp_root("nmr-legacy-peer");
    let outgoing = fixture_remote_human_message(24, 1, 2, "new local text", Some("old-peer-nonce"), json!([]));
    let (base_url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(200, json!({
            "success":true,"messageId":11,"reactions":[{"emoji":"👍","count":1,"reactedByMe":true}]
        })),
        HumanMessageFixtureReply::Json(201, json!({"success":true,"deduplicated":false,"message":outgoing})),
    ]);
    let runtime = human_message_fixture_runtime(&root, &base_url);
    let id = runtime.create_human_conversation("2","Peer")
        .expect("Human conversation")["id"].as_str().expect("id").to_owned();
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    assert!(owner.append_transcript_entry(&json!({
        "id":"human-server-message:11","kind":"message","role":"user",
        "authorKind":"human","authorId":"2","clientNonce":"old-peer-nonce",
        "content":"historical peer text","timestampMs":1000.0,"delivery":"sent",
        "remoteMessageId":"11","remoteCreatedAt":"2026-10-05T01:00:11Z",
        "remoteReactions":[],"reactions":[]
    })).expect("legacy peer"));
    let changed = runtime.toggle_human_message_reaction(&id,"human-server-message:11","👍")
        .expect("authorized reaction");
    assert_eq!(changed["content"], "historical peer text");
    assert_eq!(changed["remoteClientRequestId"], "old-peer-nonce");
    assert!(changed.get("clientNonce").is_none());
    let sent = runtime.append_human_message(&id,"new local text","old-peer-nonce",None,None,&[])
        .expect("old peer nonce cannot shadow a new local message");
    assert_eq!(sent["id"], "human-server-message:24");
    let entries = owner.get_transcript_entries().expect("rows");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0], changed);
    assert_eq!(entries[1]["clientNonce"], "old-peer-nonce");
    runtime.shutdown();
    let restarted = human_message_fixture_runtime(&root, &base_url);
    assert_eq!(restarted.read_human_conversation_transcript(&id).expect("restarted rows"), entries);
    restarted.shutdown();
    let requests = server.join().expect("Human fixture");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("POST /api/social/messages/11/reactions "));
    assert!(requests[1].starts_with("POST /api/social/messages "));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_sync_retries_resource_gap_after_restart_before_later_overlap() {
    let root = temp_root("nmr-resource-gap");
    let attachment = json!([{
        "resourceId":"retry-resource","name":"retry.txt","contentType":"text/plain",
        "size":4,"createdAt":"2026-10-05T01:00:31Z"
    }]);
    let rows = json!([
        fixture_remote_human_message(31, 2, 1, "older with resource", None, attachment),
        fixture_remote_human_message(32, 2, 1, "newer", None, json!([])),
    ]);
    let (base_url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":rows}})),
        HumanMessageFixtureReply::Json(503, json!({"success":false,"error":"retryable resource failure"})),
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":rows}})),
        HumanMessageFixtureReply::Bytes(200, vec![5, 6, 7, 8]),
    ]);
    let runtime = human_message_fixture_runtime(&root,&base_url);
    let id = runtime.create_human_conversation("2","Peer")
        .expect("Human conversation")["id"].as_str().expect("id").to_owned();
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    assert!(runtime.sync_human_conversation(&id).is_err(), "first resource GET fails");
    assert!(owner.get_transcript_entries().expect("no later overlap").is_empty());
    runtime.shutdown();
    let restarted = human_message_fixture_runtime(&root,&base_url);
    let recovered = restarted.sync_human_conversation(&id).expect("restart retries gap");
    assert_eq!(recovered.len(), 2);
    assert_eq!(recovered[0]["remoteMessageId"], "31");
    assert_eq!(recovered[1]["remoteMessageId"], "32");
    let downloaded = recovered[0]["attachments"][0]["path"].as_str().expect("resource path");
    assert_eq!(fs::read(downloaded).expect("resource bytes"), [5, 6, 7, 8]);
    restarted.shutdown();
    let requests = server.join().expect("Human fixture");
    assert_eq!(requests.len(), 4);
    assert!(requests[0].starts_with("GET /api/social/messages?"));
    assert!(requests[1].starts_with("GET /api/social/message-resources/retry-resource "));
    assert!(requests[2].starts_with("GET /api/social/messages?"));
    assert!(requests[3].starts_with("GET /api/social/message-resources/retry-resource "));
    let _ = fs::remove_dir_all(root);
}

fn invalid_human_reaction_summaries() -> Vec<(&'static str, serde_json::Value, &'static str)> {
    let inconsistent = "remote Human reaction summary is inconsistent with direct participants";
    vec![
        ("zero-with-local", json!([{"emoji":"👍","count":0,"reactedByMe":true}]), inconsistent),
        ("two-without-local", json!([{"emoji":"👍","count":2,"reactedByMe":false}]), inconsistent),
        ("third-participant", json!([{"emoji":"👍","count":3,"reactedByMe":true}]), inconsistent),
        ("u64-max", json!([{"emoji":"👍","count":u64::MAX,"reactedByMe":true}]), inconsistent),
        ("duplicate-emoji", json!([
            {"emoji":"👍","count":1,"reactedByMe":false},
            {"emoji":"👍","count":1,"reactedByMe":true}
        ]), "remote Human reaction summary contains duplicate emoji"),
    ]
}

#[test]
fn shipping_human_reaction_summaries_preserve_allowed_states_and_exact_unicode_after_restart() {
    let root = temp_root("nmr-reaction-allowed");
    let summary = json!([
        {"emoji":"🫥","count":0,"reactedByMe":false},
        {"emoji":"❤","count":1,"reactedByMe":false},
        {"emoji":"❤️","count":1,"reactedByMe":true},
        {"emoji":"👩🏽‍💻","count":2,"reactedByMe":true},
        {"emoji":" 👍","count":1,"reactedByMe":true},
        {"emoji":"👍 ","count":1,"reactedByMe":false},
        {"emoji":"é","count":1,"reactedByMe":true},
        {"emoji":"e\u{301}","count":1,"reactedByMe":false}
    ]);
    let mut remote = fixture_remote_human_message(
        890101, 2, 1, "all direct-pair states", None, json!([]),
    );
    remote["reactions"] = summary.clone();
    let (base_url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[remote.clone()]}})),
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[remote]}})),
    ]);
    let runtime = human_message_fixture_runtime(&root, &base_url);
    let id = runtime.create_human_conversation("2", "Peer")
        .expect("Human conversation")["id"].as_str().expect("id").to_owned();
    let rows = runtime.sync_human_conversation(&id).expect("all four allowed states");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "human-server-message:890101");
    assert_eq!(rows[0]["remoteReactions"], summary);
    assert_eq!(rows[0]["reactions"], json!([
        {"emoji":"❤","by":"2"},
        {"emoji":"❤️","by":"me"},
        {"emoji":"👩🏽‍💻","by":"me"},
        {"emoji":"👩🏽‍💻","by":"2"},
        {"emoji":" 👍","by":"me"},
        {"emoji":"👍 ","by":"2"},
        {"emoji":"é","by":"me"},
        {"emoji":"e\u{301}","by":"2"}
    ]));
    assert_eq!(runtime.read_human_conversation_transcript(&id).expect("durable rows"), rows);
    runtime.shutdown();
    drop(runtime);

    let restarted = human_message_fixture_runtime(&root, &base_url);
    assert_eq!(restarted.read_human_conversation_transcript(&id)
        .expect("pure reopen preserves zero state and exact Unicode"), rows);
    restarted.shutdown();
    drop(restarted);
    let requests = server.join().expect("bounded Human fixture");
    assert_eq!(requests.len(), 2, "only coverage and refresh; pure reopen must not request HTTP");
    assert!(requests[0].starts_with("GET /api/social/messages?contactId=2&limit=200&afterId=0 "));
    assert!(requests[1].starts_with("GET /api/social/messages?contactId=2&limit=200 "));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_sync_rejects_invalid_reactions_before_resources_and_known_overlap() {
    for (case_index, (label, summary, expected_error)) in
        invalid_human_reaction_summaries().into_iter().enumerate()
    {
        let root = temp_root(&format!("nmr-reaction-ingress-{label}"));
        let base = 900000_u64 + case_index as u64 * 1000;
        let known_id = base + 200;
        let known_entry_id = format!("human-server-message:{known_id}");
        let forbidden_id = format!("rejected-reaction-resource-{label}");
        let forbidden_attachment = json!([{
            "resourceId": forbidden_id, "name":"must-not-download.txt",
            "contentType":"text/plain", "size":4, "createdAt":"2026-10-05T01:00:00Z"
        }]);
        let mut bad_new = fixture_remote_human_message(
            base, 2, 1, "invalid new row", None, forbidden_attachment.clone(),
        );
        bad_new["createdAt"] = json!("2026-10-05T01:00:00Z");
        bad_new["reactions"] = summary.clone();
        let mut page = vec![bad_new];
        // The endpoint's newest window is oldest-first. Keep its raw length
        // at 200 even though two rows will be rejected before overlap lookup.
        for offset in 1_u64..=198 {
            let mut valid = fixture_remote_human_message(
                base + offset, 2, 1, &format!("valid-{offset}"), None, json!([]),
            );
            valid["createdAt"] = json!(format!(
                "2026-10-05T01:{:02}:{:02}Z", offset / 60, offset % 60,
            ));
            valid["reactions"] = json!([{"emoji":"👍","count":1,"reactedByMe":false}]);
            page.push(valid);
        }
        let mut bad_known = fixture_remote_human_message(
            known_id, 2, 1, "must not overwrite anchor", None, forbidden_attachment,
        );
        bad_known["createdAt"] = json!("2026-10-05T01:03:20Z");
        bad_known["reactions"] = summary;
        page.push(bad_known);
        assert_eq!(page.len(), 200);
        let mut older = fixture_remote_human_message(
            base + 201, 2, 1, "next afterId page still required", None, json!([]),
        );
        older["createdAt"] = json!("2026-10-05T00:59:59Z");
        let (base_url, server) = spawn_human_message_fixture(vec![
            HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":page}})),
            HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[older]}})),
        ]);
        let runtime = human_message_fixture_runtime(&root, &base_url);
        let id = runtime.create_human_conversation("2", "Peer")
            .expect("Human conversation")["id"].as_str().expect("id").to_owned();
        let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
        let anchor = json!({
            "id":known_entry_id, "kind":"message", "role":"user", "authorKind":"human",
            "authorId":"2", "content":"persisted anchor", "timestampMs":1000.0,
            "delivery":"sent", "remoteMessageId":known_id.to_string(),
            "remoteCreatedAt":"2026-10-05T01:03:20Z", "retainedLocalMetadata":{"keep":label},
            "remoteReactions":[{"emoji":"❤","count":1,"reactedByMe":false}],
            "reactions":[{"emoji":"❤","by":"2"}]
        });
        assert!(owner.append_transcript_entry(&anchor).expect("durable known row"));
        assert_eq!(runtime.sync_human_conversation(&id)
            .expect_err("malformed summary must reject before overlap or resources"), expected_error, "{label}");
        let rows = owner.get_transcript_entries().expect("independent valid progress");
        assert_eq!(rows.len(), 200, "anchor, next page, and 198 valid first-page rows: {label}");
        assert_eq!(rows[0], anchor, "known invalid row must not replace any persisted field: {label}");
        assert_eq!(rows[199]["remoteMessageId"], (base + 201).to_string(), "next afterId page was not skipped: {label}");
        for (index, row) in rows[1..199].iter().enumerate() {
            let offset = index as u64 + 1;
            assert_eq!(row["remoteMessageId"], (base + offset).to_string(), "ordered valid progress: {label}");
            assert_eq!(row["content"], format!("valid-{offset}"));
            assert_eq!(row["reactions"], json!([{"emoji":"👍","by":"2"}]));
        }
        let resource_root = root.join("agents").join("human-message-resources");
        assert!(!resource_root.exists(), "rejection precedes even resource-directory creation: {label}");
        drop(owner);
        runtime.shutdown();
        drop(runtime);

        let restarted = human_message_fixture_runtime(&root, &base_url);
        let mut chronological = vec![rows[0].clone(), rows[199].clone()];
        chronological.extend_from_slice(&rows[1..199]);
        assert_eq!(restarted.read_human_conversation_transcript(&id)
            .expect("pure reopen after independent progress"), chronological, "{label}");
        assert_eq!(restarted.open_human_conversation_db_owner(&id).expect("reopened physical owner")
            .get_transcript_entries().expect("every original row remains byte-equivalent"), rows, "{label}");
        assert!(!resource_root.join(&forbidden_id).exists());
        restarted.shutdown();
        drop(restarted);
        let requests = server.join().expect("bounded two-page fixture");
        assert_eq!(requests.len(), 2, "only the two history GETs, no resource GET: {label}");
        assert!(requests.iter().all(|request| request.starts_with("GET /api/social/messages?")), "{label}");
        assert!(requests[0].contains("&afterId=0"), "{label}");
        assert!(requests[1].contains("&afterId="), "malformed known ID must not terminate traversal: {label}");
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn shipping_human_reaction_toggle_rejects_without_mutation_and_recovers_after_restart() {
    let root = temp_root("nmr-reaction-toggle-recovery");
    let target_id = "human-server-message:890303";
    let invalid = invalid_human_reaction_summaries();
    let mut responses = invalid.iter().map(|(_, summary, _)| {
        HumanMessageFixtureReply::Json(200, json!({
            "success":true, "messageId":890303, "reactions":summary
        }))
    }).collect::<Vec<_>>();
    let valid_summary = json!([{"emoji":"👍","count":2,"reactedByMe":true}]);
    responses.push(HumanMessageFixtureReply::Json(200, json!({
        "success":true, "messageId":890303, "reactions":valid_summary
    })));
    let (base_url, server) = spawn_human_message_fixture(responses);
    let runtime = human_message_fixture_runtime(&root, &base_url);
    let id = runtime.create_human_conversation("2", "Peer")
        .expect("Human conversation")["id"].as_str().expect("id").to_owned();
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    let original = json!({
        "id":target_id, "kind":"message", "role":"user", "authorKind":"human", "authorId":"2",
        "content":"retain every field until a valid response", "timestampMs":1000.0, "delivery":"sent",
        "remoteMessageId":"890303", "remoteCreatedAt":"2026-10-05T01:00:03Z",
        "clientNonce":"legacy-peer-reaction-intent", "retainedLocalMetadata":{"keep":true},
        "remoteReactions":[{"emoji":"👍","count":1,"reactedByMe":false}],
        "reactions":[{"emoji":"👍","by":"2"}]
    });
    assert!(owner.append_transcript_entry(&original).expect("durable reaction target"));
    let events = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let captured = events.clone();
    let event_conversation = id.clone();
    let subscription = mahayana_host_runtime::transcript_mutation_events::subscribe_transcript_mutations(
        move |mutation| {
            // Human conversation IDs are shared across isolated test roots;
            // the unique target ID also fences unrelated parallel test events.
            if mutation.get("agentId").and_then(serde_json::Value::as_str) != Some(event_conversation.as_str()) {
                return;
            }
            let targets_entry = mutation.get("entryId").and_then(serde_json::Value::as_str) == Some(target_id)
                || mutation.get("entries").and_then(serde_json::Value::as_array)
                    .is_some_and(|entries| entries.iter().any(|entry| entry["id"].as_str() == Some(target_id)));
            if targets_entry {
                captured.lock().expect("event recording lock").push(serde_json::Value::Object(mutation.clone()));
            }
        },
    );
    for (label, _, expected_error) in &invalid {
        assert_eq!(runtime.toggle_human_message_reaction(&id, target_id, "👍")
            .expect_err("invalid POST summary must not settle"), *expected_error, "{label}");
        assert_eq!(owner.get_transcript_entries().expect("unchanged durable target"), vec![original.clone()], "{label}");
        assert!(events.lock().expect("recorded events").is_empty(), "no target mutation on rejected POST: {label}");
    }
    drop(owner);
    runtime.shutdown();
    drop(runtime);

    let restarted = human_message_fixture_runtime(&root, &base_url);
    assert_eq!(restarted.read_human_conversation_transcript(&id)
        .expect("pure reopen before valid recovery"), vec![original.clone()]);
    assert!(events.lock().expect("recorded events").is_empty());
    let recovered = restarted.toggle_human_message_reaction(&id, target_id, "👍")
        .expect("matching valid POST recovers after rejected summaries");
    let mut expected = original;
    let expected_object = expected.as_object_mut().expect("expected message");
    let legacy_nonce = expected_object.remove("clientNonce").expect("legacy peer provenance");
    expected_object.insert("remoteClientRequestId".into(), legacy_nonce);
    expected_object.insert("remoteReactions".into(), valid_summary);
    expected_object.insert("reactions".into(), json!([{"emoji":"👍","by":"me"},{"emoji":"👍","by":"2"}]));
    assert_eq!(recovered, expected);
    assert_eq!(restarted.read_human_conversation_transcript(&id).expect("durable recovery"), vec![expected.clone()]);
    let published = events.lock().expect("recorded events").clone();
    assert_eq!(published.len(), 1, "only the successful durable update publishes for the target");
    assert_eq!(published[0]["kind"], "entries-upserted");
    assert_eq!(published[0]["agentId"], id);
    assert_eq!(published[0]["entries"], json!([expected.clone()]));
    restarted.shutdown();
    drop(restarted);
    let reopened = human_message_fixture_runtime(&root, &base_url);
    assert_eq!(reopened.read_human_conversation_transcript(&id).expect("reopen accepted recovery"), vec![expected]);
    assert_eq!(events.lock().expect("recorded events").len(), 1, "pure reads do not republish");
    reopened.shutdown();
    drop(reopened);
    subscription.unsubscribe();

    let requests = server.join().expect("bounded reaction fixture");
    assert_eq!(requests.len(), invalid.len() + 1);
    for request in requests {
        assert!(request.starts_with("POST /api/social/messages/890303/reactions "));
        assert_eq!(fixture_request_json(&request), json!({"emoji":"👍","active":true}));
    }
    let _ = fs::remove_dir_all(root);
}


fn fixture_request_json(request: &str) -> serde_json::Value {
    request
        .split_once("\r\n\r\n")
        .and_then(|(_, body)| serde_json::from_str(body).ok())
        .unwrap_or(serde_json::Value::Null)
}

fn spawn_call_ice_server() -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ICE fixture");
    let address = listener.local_addr().expect("ICE fixture address");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept ICE request");
        let request = read_fixture_http_request(&mut stream);
        let body = json!({
            "success": true,
            "iceServers": [
                {"urls": ["stun:stun.cloudflare.com:3478"]},
                {
                    "urls": [
                        "turn:turn.cloudflare.com:3478?transport=udp",
                        "turns:turn.cloudflare.com:443?transport=tcp"
                    ],
                    "username": "temporary-user",
                    "credential": "temporary-credential"
                }
            ],
            "ttlSeconds": 3600
        }).to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        ).expect("write ICE response");
        stream.flush().expect("flush ICE response");
        request
    });
    (format!("http://{address}"), handle)
}

#[test]
fn shipping_call_ice_credentials_remain_host_owned_and_device_scoped() {
    let root = temp_root("shipping-call-ice");
    let agents = root.join("agents");
    fs::create_dir_all(&root).expect("fixture root");
    let credential_path = root.join("host-credential.json");
    fs::write(
        &credential_path,
        r#"{"accessToken":"fixture-token","deviceId":"device-a","userId":1}"#,
    ).expect("credential");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&credential_path)
            .expect("credential metadata")
            .permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&credential_path, permissions).expect("private credential");
    }

    let (base_url, server) = spawn_call_ice_server();
    fs::write(&credential_path, serde_json::json!({
        "accessToken": "fixture-token", "deviceId": "device-a", "userId": 1, "issuer": &base_url
    }).to_string()).expect("issuer-bound fixture credential");
    let client = Arc::new(
        FabushiNativeMessagingClient::new(&base_url, &credential_path)
            .expect("shipping call ICE client"),
    );
    let memory = Arc::new(
        mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(
            agents.clone(),
        ),
    );
    let runtime = Arc::new(
        ProductionSessionWorkers::with_agents_root_identity_messaging_and_dependencies(
            &agents,
            500,
            Some("1".into()),
            Some(client),
            true,
            None,
            Arc::new(|| None),
            memory,
        ),
    );

    let ice = dispatch(&runtime, "getCallIceServers", json!({}));
    assert_eq!(ice["ttlSeconds"], 3600);
    assert_eq!(ice["iceServers"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        ice["iceServers"][1]["urls"][1],
        "turns:turn.cloudflare.com:443?transport=tcp"
    );

    let request = server.join().expect("ICE fixture");
    let request_lower = request.to_ascii_lowercase();
    assert!(request.starts_with("GET /api/social/calls/ice "));
    assert!(request_lower.contains("authorization: bearer fixture-token"));
    assert!(request_lower.contains("x-fabushi-device-id: device-a"));

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}

fn spawn_remote_call_server(expected_requests: usize) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind call fixture");
    let address = listener.local_addr().expect("call fixture address");
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        let mut call_id = String::new();
        let mut state = "invited".to_string();
        let mut generation = 0_u64;
        let mut events: Vec<serde_json::Value> = Vec::new();

        for _ in 0..expected_requests {
            let (mut stream, _) = listener.accept().expect("accept call request");
            let request = read_fixture_http_request(&mut stream);
            let request_line = request.lines().next().unwrap_or_default().to_string();
            let request_json = fixture_request_json(&request);
            requests.push(request.clone());

            let body = if request_line.starts_with("POST /api/social/calls ") {
                call_id = request_json["callId"].as_str().expect("call id body").to_string();
                json!({
                    "success": true,
                    "deduplicated": false,
                    "call": {
                        "callId": call_id,
                        "creatorUserId": 1,
                        "peerUserId": 2,
                        "creatorDeviceId": "device-a",
                        "peerDeviceId": serde_json::Value::Null,
                        "state": state,
                        "generation": generation,
                        "eventSeq": events.len(),
                        "terminalState": serde_json::Value::Null,
                        "createdAt": "2026-10-05T04:00:00Z",
                        "updatedAt": "2026-10-05T04:00:00Z"
                    }
                })
            } else if request_line.starts_with("POST /api/social/calls/") {
                let kind = request_json["kind"].as_str().expect("event kind").to_string();
                let payload = request_json["payload"].clone();
                generation = request_json["generation"].as_u64().expect("event generation");
                if kind == "transition" {
                    state = payload["state"].as_str().expect("transition state").to_string();
                }
                let seq = events.len() as u64 + 1;
                let event = json!({
                    "callId": call_id,
                    "seq": seq,
                    "generation": generation,
                    "userId": 1,
                    "deviceId": "device-a",
                    "clientEventId": request_json["clientEventId"],
                    "kind": kind,
                    "payload": payload,
                    "createdAt": format!("2026-10-05T04:00:{seq:02}Z")
                });
                events.push(event.clone());
                json!({
                    "success": true,
                    "deduplicated": false,
                    "event": event,
                    "call": {
                        "callId": call_id,
                        "creatorUserId": 1,
                        "peerUserId": 2,
                        "creatorDeviceId": "device-a",
                        "peerDeviceId": serde_json::Value::Null,
                        "state": state,
                        "generation": generation,
                        "eventSeq": seq,
                        "terminalState": serde_json::Value::Null,
                        "createdAt": "2026-10-05T04:00:00Z",
                        "updatedAt": format!("2026-10-05T04:00:{seq:02}Z")
                    }
                })
            } else if request_line.starts_with("GET /api/social/calls/") {
                let after_seq = request_line
                    .split("afterSeq=")
                    .nth(1)
                    .and_then(|tail| tail.split('&').next())
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(0);
                let visible = events
                    .iter()
                    .filter(|event| event["seq"].as_u64().unwrap_or(0) > after_seq)
                    .cloned()
                    .collect::<Vec<_>>();
                json!({
                    "success": true,
                    "call": {
                        "callId": call_id,
                        "creatorUserId": 1,
                        "peerUserId": 2,
                        "creatorDeviceId": "device-a",
                        "peerDeviceId": serde_json::Value::Null,
                        "state": state,
                        "generation": generation,
                        "eventSeq": events.len(),
                        "terminalState": serde_json::Value::Null,
                        "createdAt": "2026-10-05T04:00:00Z",
                        "updatedAt": "2026-10-05T04:00:09Z"
                    },
                    "events": visible,
                    "nextAfterSeq": events.last().and_then(|event| event["seq"].as_u64()).unwrap_or(after_seq)
                })
            } else {
                panic!("unexpected call fixture request: {request_line}");
            };
            let body = body.to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            ).expect("write call response");
            stream.flush().expect("flush call response");
        }
        requests
    });
    (format!("http://{address}"), handle)
}

#[test]
fn shipping_call_session_uses_backend_ordering_then_materializes_canonical_owner() {
    let root = temp_root("shipping-call-session");
    let agents = root.join("agents");
    fs::create_dir_all(&root).expect("fixture root");
    let credential_path = root.join("host-credential.json");
    fs::write(
        &credential_path,
        r#"{"accessToken":"fixture-token","deviceId":"device-a","userId":1}"#,
    ).expect("credential");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&credential_path)
            .expect("credential metadata")
            .permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&credential_path, permissions).expect("private credential");
    }

    let (base_url, server) = spawn_remote_call_server(11);
    fs::write(&credential_path, serde_json::json!({
        "accessToken": "fixture-token", "deviceId": "device-a", "userId": 1, "issuer": &base_url
    }).to_string()).expect("issuer-bound fixture credential");
    let client = Arc::new(
        FabushiNativeMessagingClient::new(&base_url, &credential_path)
            .expect("shipping call client"),
    );
    let memory = Arc::new(
        mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(
            agents.clone(),
        ),
    );
    let runtime = Arc::new(
        ProductionSessionWorkers::with_agents_root_identity_messaging_and_dependencies(
            &agents,
            500,
            Some("1".into()),
            Some(client),
            true,
            None,
            Arc::new(|| None),
            memory,
        ),
    );
    let transport_identity = dispatch(&runtime, "getCallTransportIdentity", json!({}));
    assert_eq!(transport_identity["userId"], "1");
    assert_eq!(transport_identity["deviceId"], "device-a");

    let conversation = runtime
        .create_human_conversation("2", "Peer")
        .expect("Human conversation");
    let scope_id = conversation["id"].as_str().expect("scope id").to_string();

    let created = dispatch(
        &runtime,
        "createCallSession",
        json!({"scopeId": scope_id, "participantIds": ["1", "2"]}),
    );
    let call_id = created["id"].as_str().expect("call id").to_string();
    assert_eq!(created["state"], "invited");

    let accepted = dispatch(
        &runtime,
        "transitionCallSession",
        json!({"callId": call_id, "generation": 0, "action": "accept"}),
    );
    assert_eq!(accepted["state"], "negotiating");

    let signal = dispatch(
        &runtime,
        "sendCallSignal",
        json!({
            "callId": call_id,
            "generation": 0,
            "seq": 1,
            "senderDeviceId": "device-a",
            "kind": "offer",
            "payload": {"sdp": "remote-first"}
        }),
    );
    assert_eq!(signal["seq"], 2);
    assert_eq!(signal["payload"]["sdp"], "remote-first");

    let media = dispatch(
        &runtime,
        "updateCallMedia",
        json!({
            "callId": call_id,
            "generation": 0,
            "mediaCapabilities": {"audio": true, "video": true},
            "deviceSelection": {"microphoneId": "mic-a", "cameraId": "cam-a"}
        }),
    );
    assert_eq!(media["mediaCapabilities"]["video"], true);
    assert_eq!(media["deviceSelection"]["cameraId"], "cam-a");

    let lease = dispatch(
        &runtime,
        "getCallTransportLease",
        json!({"callId": call_id}),
    );
    assert_eq!(lease["role"], "creator");
    assert_eq!(lease["claimedDeviceId"], "device-a");
    assert_eq!(lease["isOwner"], true);
    assert_eq!(lease["claimAvailable"], false);

    let requests = server.join().expect("call fixture");
    assert_eq!(requests.len(), 11);
    assert!(requests[0].starts_with("POST /api/social/calls "));
    assert!(requests[0].contains(r#""targetUserId":"2""#));
    assert!(requests[1].starts_with("GET /api/social/calls/"));
    assert!(requests[2].starts_with("POST /api/social/calls/"));
    assert!(requests[2].contains(r#""kind":"transition""#));
    assert!(requests[4].starts_with("GET /api/social/calls/"));
    assert!(requests[5].contains(r#""kind":"signal""#));
    assert!(requests[5].contains(r#""signalKind":"offer""#));
    assert!(!requests[5].contains(r#""signalSeq""#));
    assert!(requests[8].contains(r#""kind":"media""#));
    assert!(requests[8].contains(r#""cameraId":"cam-a""#));

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}

fn spawn_incoming_call_discovery_server() -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind incoming call fixture");
    let address = listener.local_addr().expect("incoming fixture address");
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        let call_id = "remote-incoming-call-1";

        let (mut list_stream, _) = listener.accept().expect("accept call discovery");
        requests.push(read_fixture_http_request(&mut list_stream));
        let list_body = json!({
            "success": true,
            "calls": [{
                "callId": call_id,
                "creatorUserId": 2,
                "peerUserId": 1,
                "state": "negotiating",
                "generation": 0,
                "eventSeq": 1,
                "terminalState": serde_json::Value::Null,
                "createdAt": "2026-10-05T04:10:00Z",
                "updatedAt": "2026-10-05T04:10:01Z"
            }]
        }).to_string();
        write!(
            list_stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            list_body.len(),
            list_body
        ).expect("write call discovery");
        list_stream.flush().expect("flush call discovery");

        let (mut replay_stream, _) = listener.accept().expect("accept incoming call replay");
        requests.push(read_fixture_http_request(&mut replay_stream));
        let replay_body = json!({
            "success": true,
            "call": {
                "callId": call_id,
                "creatorUserId": 2,
                "peerUserId": 1,
                "state": "negotiating",
                "generation": 0,
                "eventSeq": 1,
                "terminalState": serde_json::Value::Null,
                "createdAt": "2026-10-05T04:10:00Z",
                "updatedAt": "2026-10-05T04:10:01Z"
            },
            "events": [{
                "callId": call_id,
                "seq": 1,
                "generation": 0,
                "userId": 1,
                "deviceId": "device-a",
                "clientEventId": "transition:0:accept",
                "kind": "transition",
                "payload": {"action": "accept", "state": "negotiating"},
                "createdAt": "2026-10-05T04:10:01Z"
            }],
            "nextAfterSeq": 1
        }).to_string();
        write!(
            replay_stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            replay_body.len(),
            replay_body
        ).expect("write incoming call replay");
        replay_stream.flush().expect("flush incoming call replay");
        requests
    });
    (format!("http://{address}"), handle)
}

#[test]
fn shipping_call_discovery_materializes_incoming_call_into_canonical_human_scope() {
    let root = temp_root("incoming-call-discovery");
    let agents = root.join("agents");
    fs::create_dir_all(&root).expect("fixture root");
    let credential_path = root.join("host-credential.json");
    fs::write(
        &credential_path,
        r#"{"accessToken":"fixture-token","deviceId":"device-a","userId":1}"#,
    ).expect("credential");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&credential_path)
            .expect("credential metadata")
            .permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&credential_path, permissions).expect("private credential");
    }
    let (base_url, server) = spawn_incoming_call_discovery_server();
    fs::write(&credential_path, serde_json::json!({
        "accessToken": "fixture-token", "deviceId": "device-a", "userId": 1, "issuer": &base_url
    }).to_string()).expect("issuer-bound fixture credential");
    let client = Arc::new(
        FabushiNativeMessagingClient::new(&base_url, &credential_path)
            .expect("incoming call client"),
    );
    let memory = Arc::new(
        mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(
            agents.clone(),
        ),
    );
    let runtime = Arc::new(
        ProductionSessionWorkers::with_agents_root_identity_messaging_and_dependencies(
            &agents,
            500,
            Some("1".into()),
            Some(client),
            true,
            None,
            Arc::new(|| None),
            memory,
        ),
    );

    let calls = dispatch(&runtime, "syncHumanCalls", json!({}));
    assert_eq!(calls.as_array().map(Vec::len), Some(1));
    assert_eq!(calls[0]["id"], "remote-incoming-call-1");
    assert_eq!(calls[0]["creatorId"], "2");
    assert_eq!(calls[0]["state"], "negotiating");
    assert_eq!(calls[0]["participantIds"], json!(["1", "2"]));

    let conversations = dispatch(&runtime, "listHumanConversations", json!({}));
    assert_eq!(conversations.as_array().map(Vec::len), Some(1));
    assert_eq!(calls[0]["scopeId"], conversations[0]["id"]);

    let requests = server.join().expect("incoming call fixture");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /api/social/calls?limit=200 "));
    assert!(requests[1].starts_with("GET /api/social/calls/remote-incoming-call-1?afterSeq=0&limit=200 "));

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn call_session_owner_fences_lifecycle_signaling_reconnect_and_restart() {
    let root = temp_root("call-session-owner");
    let agents = root.join("agents");
    let memory = Arc::new(
        mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(
            agents.clone(),
        ),
    );
    let runtime = Arc::new(
        ProductionSessionWorkers::with_agents_root_identity_and_dependencies(
            &agents,
            500,
            Some("human-local".into()),
            Arc::new(|| None),
            memory,
        ),
    );
    let conversation = dispatch(
        &runtime,
        "createHumanConversation",
        json!({
            "localHumanId": "human-local",
            "peerHumanId": "human-peer",
            "title": "Call peer"
        }),
    );
    let scope_id = conversation["id"].as_str().expect("conversation id");

    let invalid_scope = dispatch_production_session_gateway_call(
        &runtime,
        "createCallSession",
        &json!({
            "scopeId": "not-a-canonical-human-conversation",
            "participantIds": ["human-local", "human-peer"]
        }),
    )
    .expect("handled invalid call scope")
    .expect_err("unknown scope must fail closed");
    assert!(matches!(invalid_scope, SessionGatewayError::Internal(_)));

    let created = dispatch(
        &runtime,
        "createCallSession",
        json!({
            "scopeId": scope_id,
            "participantIds": ["human-local", "human-peer"]
        }),
    );
    let call_id = created["id"].as_str().expect("call id").to_string();
    assert_eq!(created["scopeId"], scope_id);
    assert_eq!(created["state"], "invited");
    assert_eq!(created["generation"], 0);
    assert_eq!(created["signalSeq"], 0);
    assert!(runtime.call_session_store_path().expect("call db").ends_with(".call-sessions.db"));

    let ringing = dispatch(
        &runtime,
        "transitionCallSession",
        json!({"callId": call_id, "generation": 0, "action": "ring"}),
    );
    assert_eq!(ringing["state"], "ringing");

    let negotiating = dispatch(
        &runtime,
        "transitionCallSession",
        json!({"callId": call_id, "generation": 0, "action": "accept"}),
    );
    assert_eq!(negotiating["state"], "negotiating");

    let offer = dispatch(
        &runtime,
        "sendCallSignal",
        json!({
            "callId": call_id,
            "generation": 0,
            "seq": 1,
            "senderDeviceId": "device-a",
            "kind": "offer",
            "payload": {"sdp": "offer-v1"}
        }),
    );
    assert_eq!(offer["seq"], 1);
    assert_eq!(offer["kind"], "offer");

    let replay = dispatch(
        &runtime,
        "sendCallSignal",
        json!({
            "callId": call_id,
            "generation": 0,
            "seq": 1,
            "senderDeviceId": "device-a",
            "kind": "offer",
            "payload": {"sdp": "offer-v1"}
        }),
    );
    assert_eq!(replay, offer);

    let conflicting_replay = dispatch_production_session_gateway_call(
        &runtime,
        "sendCallSignal",
        &json!({
            "callId": call_id,
            "generation": 0,
            "seq": 1,
            "senderDeviceId": "device-a",
            "kind": "offer",
            "payload": {"sdp": "different"}
        }),
    )
    .expect("handled conflicting replay")
    .expect_err("conflicting replay must fail");
    assert!(matches!(
        conflicting_replay,
        SessionGatewayError::Internal(message)
            if message.contains("conflicts with previously accepted payload")
    ));

    let gap = dispatch_production_session_gateway_call(
        &runtime,
        "sendCallSignal",
        &json!({
            "callId": call_id,
            "generation": 0,
            "seq": 3,
            "senderDeviceId": "device-b",
            "kind": "answer",
            "payload": {"sdp": "answer-gap"}
        }),
    )
    .expect("handled signal gap")
    .expect_err("signal gap must fail");
    assert!(matches!(
        gap,
        SessionGatewayError::Internal(message)
            if message.contains("sequence gap")
    ));

    let answer = dispatch(
        &runtime,
        "sendCallSignal",
        json!({
            "callId": call_id,
            "generation": 0,
            "seq": 2,
            "senderDeviceId": "device-b",
            "kind": "answer",
            "payload": {"sdp": "answer-v1"}
        }),
    );
    assert_eq!(answer["seq"], 2);

    let connected = dispatch(
        &runtime,
        "transitionCallSession",
        json!({"callId": call_id, "generation": 0, "action": "connected"}),
    );
    assert_eq!(connected["state"], "connected");

    let media = dispatch(
        &runtime,
        "updateCallMedia",
        json!({
            "callId": call_id,
            "generation": 0,
            "mediaCapabilities": {"audio": true, "video": true, "screenShare": true},
            "deviceSelection": {"microphoneId": "mic-1", "cameraId": "cam-1"}
        }),
    );
    assert_eq!(media["mediaCapabilities"]["audio"], true);
    assert_eq!(media["deviceSelection"]["microphoneId"], "mic-1");

    let reconnecting = dispatch(
        &runtime,
        "transitionCallSession",
        json!({"callId": call_id, "generation": 0, "action": "reconnect"}),
    );
    assert_eq!(reconnecting["state"], "reconnecting");
    assert_eq!(reconnecting["generation"], 1);
    assert_eq!(reconnecting["signalSeq"], 0);

    let stale = dispatch_production_session_gateway_call(
        &runtime,
        "sendCallSignal",
        &json!({
            "callId": call_id,
            "generation": 0,
            "seq": 3,
            "senderDeviceId": "device-a",
            "kind": "candidate",
            "payload": {"candidate": "stale"}
        }),
    )
    .expect("handled stale generation")
    .expect_err("stale generation must fail");
    assert!(matches!(
        stale,
        SessionGatewayError::Internal(message)
            if message.contains("stale call generation")
    ));

    let resumed = dispatch(
        &runtime,
        "transitionCallSession",
        json!({"callId": call_id, "generation": 1, "action": "resume"}),
    );
    assert_eq!(resumed["state"], "negotiating");
    let candidate = dispatch(
        &runtime,
        "sendCallSignal",
        json!({
            "callId": call_id,
            "generation": 1,
            "seq": 1,
            "senderDeviceId": "device-a",
            "kind": "candidate",
            "payload": {"candidate": "fresh"}
        }),
    );
    assert_eq!(candidate["generation"], 1);
    assert_eq!(candidate["seq"], 1);

    let signals = dispatch(
        &runtime,
        "listCallSignals",
        json!({"callId": call_id, "generation": 1, "afterSeq": 0, "limit": 20}),
    );
    assert_eq!(signals.as_array().map(Vec::len), Some(1));
    assert_eq!(signals[0]["payload"]["candidate"], "fresh");

    runtime.shutdown();
    drop(runtime);

    let restarted_memory = Arc::new(
        mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(
            agents.clone(),
        ),
    );
    let restarted = Arc::new(
        ProductionSessionWorkers::with_agents_root_identity_and_dependencies(
            &agents,
            500,
            Some("human-local".into()),
            Arc::new(|| None),
            restarted_memory,
        ),
    );
    let restored = dispatch(
        &restarted,
        "getCallSession",
        json!({"callId": call_id}),
    );
    assert_eq!(restored["state"], "negotiating");
    assert_eq!(restored["generation"], 1);
    assert_eq!(restored["signalSeq"], 1);

    let ended = dispatch(
        &restarted,
        "transitionCallSession",
        json!({
            "callId": call_id,
            "generation": 1,
            "action": "hangup",
            "terminalReason": "user-ended"
        }),
    );
    assert_eq!(ended["state"], "ended");
    assert_eq!(ended["terminalReason"], "user-ended");

    let after_terminal = dispatch_production_session_gateway_call(
        &restarted,
        "sendCallSignal",
        &json!({
            "callId": call_id,
            "generation": 1,
            "seq": 2,
            "senderDeviceId": "device-a",
            "kind": "candidate",
            "payload": {"candidate": "late"}
        }),
    )
    .expect("handled terminal signal")
    .expect_err("terminal signal must fail");
    assert!(matches!(
        after_terminal,
        SessionGatewayError::Internal(message)
            if message.contains("terminal call session")
    ));

    restarted.shutdown();
    drop(restarted);

    let foreign_memory = Arc::new(
        mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(
            agents.clone(),
        ),
    );
    let foreign = Arc::new(
        ProductionSessionWorkers::with_agents_root_identity_and_dependencies(
            &agents,
            500,
            Some("human-foreign".into()),
            Arc::new(|| None),
            foreign_memory,
        ),
    );
    let unauthorized = dispatch_production_session_gateway_call(
        &foreign,
        "getCallSession",
        &json!({"callId": call_id}),
    )
    .expect("handled foreign call lookup")
    .expect_err("foreign identity must not read call truth");
    assert!(matches!(
        unauthorized,
        SessionGatewayError::Internal(message)
            if message.contains("authenticated Human identity")
    ));
    foreign.shutdown();
    drop(foreign);

    let _ = fs::remove_dir_all(root);
}



#[test]
fn shipping_human_after_id_checkpoint_resumes_after_restart_without_replaying_committed_page() {
    let root = temp_root("human-coverage-restart");
    let first_page = (1..=200).map(|n| {
        let mut row = fixture_remote_human_message(n, 2, 1, &format!("row-{n}"), None, json!([]));
        row["createdAt"] = json!("2026-10-05T01:00:00Z");
        row
    }).collect::<Vec<_>>();
    let (base_url, first_server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":first_page}})),
        HumanMessageFixtureReply::Json(503, json!({"success":false,"error":"retryable disconnect"})),
    ]);
    let first = human_message_fixture_runtime(&root, &base_url);
    let id = first.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("id").to_string();
    assert!(first.sync_human_conversation(&id).is_err());
    let owner = first.open_human_conversation_db_owner(&id).expect("owner");
    assert_eq!(owner.get_transcript_entries().expect("page persisted").len(), 200);
    assert_eq!(owner.get_metadata("humanHistoryCoverage").expect("watermark"),
        Some(json!({"version":1,"afterId":200,"complete":false})));
    first.shutdown();
    drop(owner);
    drop(first);
    let requests = first_server.join().expect("fixture");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("&afterId=0"));
    assert!(requests[1].contains("&afterId=200"));

    // Same address and private credential, but new Host instance.
    // The second server gets its own address; replacing the issuer-bound
    // credential is intentional because only the active service origin changed.
    let mut resumed_row = fixture_remote_human_message(201, 2, 1, "row-201", None, json!([]));
    resumed_row["createdAt"] = json!("2026-10-05T01:00:00Z");
    let (resume_url, second_server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[
            resumed_row.clone()
        ]}})),
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[
            resumed_row.clone()
        ]}})),
    ]);
    let resumed = human_message_fixture_runtime(&root, &resume_url);
    let rows = resumed.sync_human_conversation(&id).expect("resume after durable page");
    assert_eq!(rows.len(), 201);
    assert_eq!(rows[0]["id"], "human-server-message:1");
    assert_eq!(rows[200]["id"], "human-server-message:201");
    let owner = resumed.open_human_conversation_db_owner(&id).expect("owner");
    assert_eq!(owner.get_metadata("humanHistoryCoverage").expect("finished"),
        Some(json!({"version":1,"afterId":201,"complete":true})));
    let requests = second_server.join().expect("resume HTTP");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("&afterId=200"));
    assert!(!requests[1].contains("&afterId="), "latest snapshot refresh");
    resumed.shutdown();
    drop(owner);
    drop(resumed);
    let _ = fs::remove_dir_all(root);
}


#[test]
fn shipping_human_reaction_intent_survives_failed_http_and_replays_once_after_restart() {
    let root = temp_root("human-reaction-intent-restart");
    let (url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(503, json!({"success":false,"error":"unavailable"})),
        HumanMessageFixtureReply::Json(200, json!({
            "success":true,"messageId":777,
            "reactions":[{"emoji":"👍","count":1,"reactedByMe":true}]
        })),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2","Peer").expect("conversation")["id"]
        .as_str().expect("id").to_string();
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    let entry_id = "human-server-message:777";
    let original = json!({
        "id":entry_id,"kind":"message","role":"user","authorKind":"human",
        "authorId":"2","content":"persist target","timestampMs":1000.0,
        "delivery":"sent","remoteMessageId":"777","remoteCreatedAt":"2026-10-05T01:00:17Z",
        "remoteReactions":[],"reactions":[]
    });
    assert!(owner.append_transcript_entry(&original).expect("seed"));
    assert!(runtime.toggle_human_message_reaction(&id,entry_id,"👍").is_err());
    assert_eq!(owner.get_metadata("humanReactionIntent").expect("intent"),Some(json!({
        "entryId":entry_id,"remoteMessageId":"777","emoji":"👍","active":true,"localHumanId":"1"
    })));
    assert_eq!(owner.get_transcript_entries().expect("unchanged"),vec![original]);
    runtime.shutdown(); drop(owner); drop(runtime);
    let restarted = human_message_fixture_runtime(&root, &url);
    let settled = restarted.toggle_human_message_reaction(&id,entry_id,"👍")
        .expect("retry staged desired reaction");
    assert_eq!(settled["remoteReactions"],json!([{"emoji":"👍","count":1,"reactedByMe":true}]));
    let owner = restarted.open_human_conversation_db_owner(&id).expect("reopen");
    assert_eq!(owner.get_metadata("humanReactionIntent").expect("ack"),Some(serde_json::Value::Null));
    assert_eq!(owner.get_transcript_entries().expect("recovered").len(),1);
    let requests = server.join().expect("fixture");
    assert_eq!(requests.len(),2);
    for request in requests {
        assert!(request.starts_with("POST /api/social/messages/777/reactions "));
        assert_eq!(fixture_request_json(&request),json!({"emoji":"👍","active":true}));
    }
    restarted.shutdown(); drop(owner); drop(restarted);
    let _ = fs::remove_dir_all(root);
}


#[test]
fn shipping_human_history_rejects_oversized_page_before_persisting_coverage() {
    let root = temp_root("human-oversized-history-page");
    let oversized = (1..=201_u64)
        .map(|id| fixture_remote_human_message(id, 2, 1, &format!("row-{id}"), None, json!([])))
        .collect::<Vec<_>>();
    let (url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":oversized}})),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("conversation id").to_string();
    let error = runtime.sync_human_conversation(&id).expect_err("oversized page must fail");
    assert_eq!(error, "Fabushi Human history page exceeded the 200-message response budget");
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    assert!(owner.get_transcript_entries().expect("unchanged transcript").is_empty());
    assert_eq!(owner.get_metadata("humanHistoryCoverage").expect("checkpoint"), None);
    let requests = server.join().expect("fixture");
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("&afterId=0"));
    runtime.shutdown(); drop(owner); drop(runtime);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_history_rejects_damaged_checkpoint_before_any_network_request() {
    let root = temp_root("human-bad-history-checkpoint");
    // Keep the socket open through every refusal. A forbidden POST would
    // either change the causal error or remain observable in its accept queue.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind no-dispatch observer");
    listener.set_nonblocking(true).expect("observe without waiting for HTTP");
    let url = format!("http://{}", listener.local_addr().expect("observer address"));
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("conversation id").to_string();
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    let original = json!({
        "id":"human-server-message:777","kind":"message","role":"user",
        "authorKind":"human","authorId":"2","content":"reaction target",
        "timestampMs":1000.0,"delivery":"sent","remoteMessageId":"777",
        "remoteReactions":[],"reactions":[]
    });
    let pending = json!({
        "id":"human-message:coverage-pending","kind":"message","role":"user",
        "authorKind":"human","authorId":"1","clientNonce":"coverage-pending",
        "content":"keep pending","timestampMs":1001.0,"delivery":"pending"
    });
    let intent = json!({
        "entryId":"human-server-message:777","remoteMessageId":"777",
        "emoji":"👍","active":true,"localHumanId":"1"
    });
    assert!(owner.append_transcript_entry(&original).expect("persist reaction target"));
    assert!(owner.append_transcript_entry(&pending).expect("persist pending outbox"));
    // Test the pending send both by itself and behind a valid reaction intent.
    // No forwarding batch is staged: its independent recovery remains unchanged.
    for with_reaction in [false, true] {
        let staged_intent = if with_reaction { intent.clone() } else { serde_json::Value::Null };
        assert!(owner.set_metadata("humanReactionIntent", staged_intent.clone()).expect("stage reaction recovery"));
        for (checkpoint, expected) in [
            (json!({"afterId":"corrupt","complete":false}), "persisted Human history coverage cursor is invalid"),
            (json!({"version":1,"afterId":9_007_199_254_740_992_u64,"complete":false}), "persisted Human history coverage cursor is invalid"),
            (json!({"version":1,"afterId":200,"complete":"false"}), "persisted Human history coverage completion flag is invalid"),
            (json!({"version":null,"afterId":200,"complete":false}), "persisted Human history coverage version is unsupported"),
            (json!({"version":"1","afterId":200,"complete":false}), "persisted Human history coverage version is unsupported"),
            (json!({"version":2,"afterId":200,"complete":false}), "persisted Human history coverage version is unsupported"),
            (json!({"version":{},"afterId":200,"complete":false}), "persisted Human history coverage version is unsupported"),
        ] {
            assert!(owner.set_metadata("humanHistoryCoverage", checkpoint.clone()).expect("stage invalid checkpoint"));
            let metadata = owner.read_metadata().expect("metadata before rejection");
            assert_eq!(runtime.sync_human_conversation(&id).expect_err("invalid checkpoint must fail"), expected);
            assert_eq!(owner.read_metadata().expect("no checkpoint or intent mutation"), metadata);
            assert_eq!(owner.get_transcript_entries().expect("outbox was not dispatched"),
                vec![original.clone(), pending.clone()]);
            assert!(matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
                "same-conversation recovery must not dispatch before coverage validation");
        }
    }
    runtime.shutdown(); drop(owner); drop(runtime);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_reaction_unconfirmed_success_keeps_intent_until_restart_replay() {
    let root = temp_root("human-unconfirmed-reaction");
    let (url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(200, json!({
            "success":true,"messageId":777,"reactions":[]
        })),
        HumanMessageFixtureReply::Json(200, json!({
            "success":true,"messageId":777,
            "reactions":[{"emoji":"👍","count":1,"reactedByMe":true}]
        })),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2","Peer").expect("conversation")["id"]
        .as_str().expect("conversation id").to_string();
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    let entry_id = "human-server-message:777";
    let original = json!({
        "id":entry_id,"kind":"message","role":"user","authorKind":"human",
        "authorId":"2","content":"reaction target","timestampMs":1000.0,
        "delivery":"sent","remoteMessageId":"777","remoteReactions":[],"reactions":[]
    });
    assert!(owner.append_transcript_entry(&original).expect("seed"));
    let error = runtime.toggle_human_message_reaction(&id, entry_id, "👍")
        .expect_err("successful HTTP without desired state must fail closed");
    assert_eq!(error, "Fabushi Human reaction response did not confirm the desired state");
    assert_eq!(owner.get_metadata("humanReactionIntent").expect("staged intent"), Some(json!({
        "entryId":entry_id,"remoteMessageId":"777",
        "emoji":"👍","active":true,"localHumanId":"1"
    })));
    assert_eq!(owner.get_transcript_entries().expect("not falsely settled"), vec![original]);
    runtime.shutdown(); drop(owner); drop(runtime);

    let restarted = human_message_fixture_runtime(&root, &url);
    let settled = restarted.toggle_human_message_reaction(&id, entry_id, "👍")
        .expect("replay the same desired state instead of toggling off");
    assert_eq!(settled["remoteReactions"], json!([{"emoji":"👍","count":1,"reactedByMe":true}]));
    let owner = restarted.open_human_conversation_db_owner(&id).expect("owner");
    assert_eq!(owner.get_metadata("humanReactionIntent").expect("ack"), Some(serde_json::Value::Null));
    let requests = server.join().expect("fixture");
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert!(request.starts_with("POST /api/social/messages/777/reactions "));
        assert_eq!(fixture_request_json(&request), json!({"emoji":"👍","active":true}));
    }
    restarted.shutdown(); drop(owner); drop(restarted);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_reaction_missing_summary_does_not_acknowledge_intent() {
    let root = temp_root("human-missing-reaction-summary");
    let (url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(200, json!({"success":true,"messageId":778})),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2","Peer").expect("conversation")["id"]
        .as_str().expect("conversation id").to_string();
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    let entry_id = "human-server-message:778";
    let original = json!({
        "id":entry_id,"kind":"message","role":"user","authorKind":"human",
        "authorId":"2","content":"missing summary","timestampMs":1000.0,
        "delivery":"sent","remoteMessageId":"778","remoteReactions":[],"reactions":[]
    });
    assert!(owner.append_transcript_entry(&original).expect("seed"));
    let error = runtime.toggle_human_message_reaction(&id, entry_id, "👍")
        .expect_err("missing response summary must fail closed");
    assert_eq!(error, "Fabushi Human reaction response omitted the reaction summary");
    assert_eq!(owner.get_metadata("humanReactionIntent").expect("still staged"), Some(json!({
        "entryId":entry_id,"remoteMessageId":"778",
        "emoji":"👍","active":true,"localHumanId":"1"
    })));
    assert_eq!(owner.get_transcript_entries().expect("no false settlement"), vec![original]);
    assert_eq!(server.join().expect("fixture").len(), 1);
    runtime.shutdown(); drop(owner); drop(runtime);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_history_cursor_echo_rejects_legacy_and_conflicting_backend_pages() {
    for (label, cursor, next_after) in [
        ("no-cursor", serde_json::Value::Null, json!(1)),
        ("wrong-cursor", json!({"mode":"after-id","afterId":1}), json!(1)),
        ("wrong-mode", json!({"mode":"latest","afterId":0}), json!(1)),
        ("wrong-next", json!({"mode":"after-id","afterId":0}), json!(42)),
    ] {
        let root = temp_root(label);
        let row = fixture_remote_human_message(1, 2, 1, "first", None, json!([]));
        let (url, server) = spawn_human_message_fixture(vec![
            HumanMessageFixtureReply::Json(200, json!({
                "success":true,"data":{
                    "messages":[row], "historyCursor":cursor, "nextAfterId":next_after
                }
            })),
        ]);
        let runtime = human_message_fixture_runtime(&root, &url);
        let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
            .as_str().expect("id").to_string();
        let failure = runtime.sync_human_conversation(&id).expect_err("untrusted page rejected");
        assert!(failure.contains("historyCursor") || failure.contains("nextAfterId"), "{label}: {failure}");
        let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
        assert!(owner.get_transcript_entries().expect("no materialization").is_empty(), "{label}");
        assert!(owner.get_metadata("humanHistoryCoverage").expect("no checkpoint").is_none(), "{label}");
        assert_eq!(server.join().expect("HTTP fixture").len(), 1, "{label}");
        runtime.shutdown();
        drop(owner);
        drop(runtime);
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn shipping_human_6400_messages_use_exact_32_page_budget_and_resume_after_restart() {
    let root = temp_root("human-6400-page-budget");
    let responses = (0..32_u64).map(|page| {
        let rows = (page * 200 + 1..=(page + 1) * 200)
            .map(|id| {
                let mut row = fixture_remote_human_message(id, 2, 1, &format!("row-{id}"), None, json!([]));
                row["createdAt"] = json!("2026-10-05T01:00:00Z");
                row
            })
            .collect::<Vec<_>>();
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":rows}}))
    }).collect::<Vec<_>>();
    let (url, first_server) = spawn_human_message_fixture(responses);
    let first = human_message_fixture_runtime(&root, &url);
    let id = first.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("id").to_string();
    let (first_extension, first_feed) = start_human_transcript_feed(&root, &first);
    let rows = first.sync_human_conversation(&id).expect("bounded first sync");
    assert_eq!(rows.len(), 6400);
    assert_eq!(rows.iter().map(|row| row["remoteMessageId"].as_str().expect("remote identity").to_string())
        .collect::<Vec<_>>(), (1..=6400).map(|id| id.to_string()).collect::<Vec<_>>(),
        "every row appears exactly once in canonical order across all 32 pages");
    let projected = human_transcript_feed_events(&first_feed);
    assert_eq!(projected.len(), 6400, "the real production bridge projects every committed row once");
    for (event, row) in projected.iter().zip(&rows) {
        assert_eq!(event, &json!({
            "channel":"transcript","payload":{"type":"updated","agentId":id,"entry":row},
        }), "the 6400-row bridge uses each current indexed row in durable order");
    }
    let owner = first.open_human_conversation_db_owner(&id).expect("owner");
    assert_eq!(owner.get_metadata("humanHistoryCoverage").expect("checkpoint"),
        Some(json!({"version":1,"afterId":6400,"complete":false})));
    drop(first_extension);
    first.shutdown();
    drop(owner);
    drop(first);
    let requests = first_server.join().expect("32-page fixture");
    assert_eq!(requests.len(), 32, "no 33rd probe or unbudgeted latest refresh");
    assert!(requests.first().expect("first").contains("&afterId=0"));
    assert!(requests.last().expect("last").contains("&afterId=6200"));

    let mut row = fixture_remote_human_message(6401, 2, 1, "row-6401", None, json!([]));
    row["createdAt"] = json!("2026-10-05T01:00:00Z");
    let (resume_url, second_server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[row.clone()]}})),
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[row]}})),
    ]);
    let resumed = human_message_fixture_runtime(&root, &resume_url);
    let (resumed_extension, resumed_feed) = start_human_transcript_feed(&root, &resumed);
    let resumed_rows = resumed.sync_human_conversation(&id).expect("resumed cursor");
    assert_eq!(resumed_rows.len(), 6401);
    assert_eq!(&resumed_rows[..6400], rows.as_slice(), "restart preserves every previously covered row and its order");
    assert_eq!(resumed_rows[6400]["id"], "human-server-message:6401");
    assert_eq!(human_transcript_feed_events(&resumed_feed), vec![json!({
        "channel":"transcript","payload":{"type":"updated","agentId":id,"entry":resumed_rows[6400]},
    })], "restart installs one observer and unchanged latest refresh adds no duplicate");
    assert!(human_transcript_feed_events(&first_feed).is_empty(), "the stopped bridge cannot project resumed writes");
    let owner = resumed.open_human_conversation_db_owner(&id).expect("owner");
    assert_eq!(owner.get_metadata("humanHistoryCoverage").expect("complete checkpoint"),
        Some(json!({"version":1,"afterId":6401,"complete":true})));
    let requests = second_server.join().expect("resume fixture");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("&afterId=6400"), "resume must not replay first 6400");
    assert!(!requests[1].contains("&afterId="), "spare request refreshes known latest");
    drop(resumed_extension);
    resumed.shutdown();
    drop(owner);
    drop(resumed);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_history_unversioned_checkpoint_rebuilds_only_after_qualified_page() {
    let root = temp_root("human-legacy-coverage-upgrade");
    let remote = fixture_remote_human_message(501, 2, 1, "retained content", None, json!([]));
    let next = fixture_remote_human_message(502, 2, 1, "new content", None, json!([]));
    let (url, server) = spawn_human_message_fixture(vec![
        // Raw bytes deliberately bypass the fixture's compatible-backend cursor
        // default. An old service cannot qualify a previously stored watermark.
        HumanMessageFixtureReply::Bytes(200, json!({
            "success":true,"data":{"messages":[remote.clone()]}
        }).to_string().into_bytes()),
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{
            "messages":[remote.clone()],"historyCursor":{"mode":"after-id","afterId":0},"nextAfterId":501
        }})),
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[remote.clone()]}})),
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{
            "messages":[next.clone()],"historyCursor":{"mode":"after-id","afterId":501},"nextAfterId":502
        }})),
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[remote,next]}})),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("conversation id").to_string();
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    let original = json!({
        "id":"human-server-message:501","kind":"message","role":"user",
        "authorKind":"human","authorId":"2","content":"retained content",
        "timestampMs":1000.0,"delivery":"sent","remoteMessageId":"501",
        "retainedLocalMetadata":{"keep":true},"remoteReactions":[],"reactions":[]
    });
    assert!(owner.append_transcript_entry(&original).expect("retain existing user content"));
    let legacy = json!({"afterId":9999,"complete":true});
    assert!(owner.set_metadata("humanHistoryCoverage", legacy.clone()).expect("legacy checkpoint"));
    let original_metadata = owner.read_metadata().expect("legacy metadata");
    assert_eq!(runtime.sync_human_conversation(&id).expect_err("old service cannot qualify coverage"),
        "Fabushi Human history response omitted historyCursor");
    assert_eq!(owner.read_metadata().expect("unqualified response preserves metadata"), original_metadata);
    assert_eq!(owner.get_transcript_entries().expect("unqualified response preserves content"), vec![original.clone()]);
    runtime.shutdown(); drop(owner); drop(runtime);

    let restarted = human_message_fixture_runtime(&root, &url);
    let owner = restarted.open_human_conversation_db_owner(&id).expect("owner after restart");
    assert_eq!(owner.get_metadata("humanHistoryCoverage").expect("legacy survives restart"), Some(legacy));
    assert_eq!(owner.get_transcript_entries().expect("pure reopen retains content"), vec![original.clone()]);
    let recovered = restarted.sync_human_conversation(&id).expect("qualified origin rebuild");
    assert_eq!(recovered.len(), 1, "existing canonical identity is refreshed without duplication");
    assert_eq!(recovered[0]["id"], original["id"]);
    assert_eq!(recovered[0]["content"], original["content"]);
    assert_eq!(recovered[0]["retainedLocalMetadata"], original["retainedLocalMetadata"]);
    assert_eq!(owner.get_metadata("humanHistoryCoverage").expect("qualified checkpoint"),
        Some(json!({"version":1,"afterId":501,"complete":true})));
    restarted.shutdown(); drop(owner); drop(restarted);

    let resumed = human_message_fixture_runtime(&root, &url);
    assert_eq!(resumed.read_human_conversation_transcript(&id).expect("qualified reopen"), recovered);
    let rows = resumed.sync_human_conversation(&id).expect("resume the qualified interval");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], recovered[0], "later sync preserves the rebuilt row and local metadata");
    assert_eq!(rows[1]["id"], "human-server-message:502");
    let owner = resumed.open_human_conversation_db_owner(&id).expect("resumed owner");
    assert_eq!(owner.get_metadata("humanHistoryCoverage").expect("resumed checkpoint"),
        Some(json!({"version":1,"afterId":502,"complete":true})));
    let requests = server.join().expect("upgrade and restart request trace");
    assert_eq!(requests.len(), 5);
    for index in [0, 1] {
        assert!(requests[index].starts_with("GET /api/social/messages?contactId=2&limit=200&afterId=0 "));
    }
    assert!(requests[3].starts_with("GET /api/social/messages?contactId=2&limit=200&afterId=501 "));
    for index in [2, 4] {
        assert!(requests[index].starts_with("GET /api/social/messages?contactId=2&limit=200 "));
    }
    resumed.shutdown(); drop(owner); drop(resumed);
    let reopened = human_message_fixture_runtime(&root, &url);
    assert_eq!(reopened.read_human_conversation_transcript(&id).expect("durable resumed content"), rows);
    reopened.shutdown(); drop(reopened);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_concurrent_reactions_serialize_one_conversation_without_blocking_another() {
    let root = temp_root("human-concurrent-reactions");
    let (first_seen, first_wait) = std::sync::mpsc::channel();
    let (first_release, first_resume) = std::sync::mpsc::channel();
    let (second_seen, second_wait) = std::sync::mpsc::channel();
    let (second_release, second_resume) = std::sync::mpsc::channel();
    let (url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Deferred(Box::new(HumanMessageFixtureReply::Json(200, json!({
            "success":true,"messageId":777,
            "reactions":[{"emoji":"👍","count":1,"reactedByMe":true}]
        }))), first_seen, first_resume),
        HumanMessageFixtureReply::Json(200, json!({
            "success":true,"messageId":888,
            "reactions":[{"emoji":"👍","count":1,"reactedByMe":true}]
        })),
        HumanMessageFixtureReply::Deferred(Box::new(HumanMessageFixtureReply::Json(200, json!({
            "success":true,"messageId":777,"reactions":[]
        }))), second_seen, second_resume),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("id").to_string();
    let other = runtime.create_human_conversation("3", "Other").expect("other conversation")["id"]
        .as_str().expect("other id").to_string();
    for (conversation, entry, remote, author) in [
        (id.as_str(), "human-server-message:777", "777", "2"),
        (other.as_str(), "human-server-message:888", "888", "3"),
    ] {
        assert!(runtime.open_human_conversation_db_owner(conversation).expect("owner")
            .append_transcript_entry(&json!({
                "id":entry,"kind":"message","role":"user","authorKind":"human",
                "authorId":author,"content":"target","timestampMs":1000.0,
                "delivery":"sent","remoteMessageId":remote,"remoteReactions":[],"reactions":[]
            })).expect("seed target"));
    }
    let first = {
        let runtime = Arc::clone(&runtime);
        let id = id.clone();
        thread::spawn(move || runtime.toggle_human_message_reaction(&id, "human-server-message:777", "👍"))
    };
    first_wait.recv_timeout(std::time::Duration::from_secs(15)).expect("first POST is held");
    let (started, waiting) = std::sync::mpsc::channel();
    let second = {
        let runtime = Arc::clone(&runtime);
        let id = id.clone();
        thread::spawn(move || {
            started.send(()).expect("second toggle started");
            runtime.toggle_human_message_reaction(&id, "human-server-message:777", "👍")
        })
    };
    waiting.recv_timeout(std::time::Duration::from_secs(15)).expect("competing toggle invoked");
    let other_settled = runtime.toggle_human_message_reaction(&other, "human-server-message:888", "👍")
        .expect("unrelated conversation progresses while first POST is held");
    assert_eq!(other_settled["reactions"], json!([{"emoji":"👍","by":"me"}]));
    assert!(matches!(second_wait.recv_timeout(std::time::Duration::from_millis(150)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)), "second same-conversation POST must wait");
    first_release.send(()).expect("release first settlement");
    let activated = first.join().expect("first thread").expect("first toggle");
    assert_eq!(activated["reactions"], json!([{"emoji":"👍","by":"me"}]));
    second_wait.recv_timeout(std::time::Duration::from_secs(15)).expect("second POST starts after first settlement");
    second_release.send(()).expect("release second settlement");
    let deactivated = second.join().expect("second thread").expect("second toggle");
    assert_eq!(deactivated["reactions"], json!([]));
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    assert_eq!(owner.get_metadata("humanReactionIntent").expect("settled intent"), Some(serde_json::Value::Null));
    let requests = server.join().expect("concurrent fixture");
    assert_eq!(requests.len(), 3);
    assert!(requests[0].starts_with("POST /api/social/messages/777/reactions "));
    assert!(requests[1].starts_with("POST /api/social/messages/888/reactions "));
    assert!(requests[2].starts_with("POST /api/social/messages/777/reactions "));
    assert_eq!(fixture_request_json(&requests[0]), json!({"emoji":"👍","active":true}));
    assert_eq!(fixture_request_json(&requests[2]), json!({"emoji":"👍","active":false}));
    runtime.shutdown(); drop(owner); drop(runtime);
    let restarted = human_message_fixture_runtime(&root, &url);
    assert_eq!(restarted.read_human_conversation_transcript(&id).expect("durable serialized result"), vec![deactivated]);
    assert_eq!(restarted.read_human_conversation_transcript(&other).expect("durable independent result"), vec![other_settled]);
    restarted.shutdown(); drop(restarted);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_reaction_waits_for_inflight_history_before_deriving_desired_state() {
    let root = temp_root("human-history-reaction-race");
    let (history_seen, history_wait) = std::sync::mpsc::channel();
    let (history_release, history_resume) = std::sync::mpsc::channel();
    let (reaction_seen, reaction_wait) = std::sync::mpsc::channel();
    let (reaction_release, reaction_resume) = std::sync::mpsc::channel();
    let mut snapshot = fixture_remote_human_message(31, 2, 1, "history target", None, json!([]));
    snapshot["reactions"] = json!([{"emoji":"👍","count":1,"reactedByMe":true}]);
    let (url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Deferred(Box::new(HumanMessageFixtureReply::Json(200,
            json!({"success":true,"data":{"messages":[snapshot]}}))), history_seen, history_resume),
        HumanMessageFixtureReply::Deferred(Box::new(HumanMessageFixtureReply::Json(200,
            json!({"success":true,"messageId":31,"reactions":[]}))), reaction_seen, reaction_resume),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("id").to_string();
    assert!(runtime.open_human_conversation_db_owner(&id).expect("owner")
        .append_transcript_entry(&json!({
            "id":"human-server-message:31","kind":"message","role":"user","authorKind":"human",
            "authorId":"2","content":"history target","timestampMs":1000.0,
            "delivery":"sent","remoteMessageId":"31","remoteReactions":[],"reactions":[]
        })).expect("seed"));
    let history = {
        let runtime = Arc::clone(&runtime);
        let id = id.clone();
        thread::spawn(move || runtime.sync_human_conversation(&id))
    };
    history_wait.recv_timeout(std::time::Duration::from_secs(15)).expect("history is in flight");
    let (started, waiting) = std::sync::mpsc::channel();
    let reaction = {
        let runtime = Arc::clone(&runtime);
        let id = id.clone();
        thread::spawn(move || {
            started.send(()).expect("reaction started");
            runtime.toggle_human_message_reaction(&id, "human-server-message:31", "👍")
        })
    };
    waiting.recv_timeout(std::time::Duration::from_secs(15)).expect("reaction invoked");
    assert!(matches!(reaction_wait.recv_timeout(std::time::Duration::from_millis(150)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)), "reaction must not overtake the pending snapshot");
    history_release.send(()).expect("release snapshot");
    let refreshed = history.join().expect("history thread").expect("history refresh");
    assert_eq!(refreshed[0]["reactions"], json!([{"emoji":"👍","by":"me"}]));
    reaction_wait.recv_timeout(std::time::Duration::from_secs(15)).expect("reaction follows snapshot");
    reaction_release.send(()).expect("release reaction");
    let settled = reaction.join().expect("reaction thread").expect("reaction settles");
    assert_eq!(settled["reactions"], json!([]));
    let requests = server.join().expect("ordered fixture");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("&afterId=0"));
    assert_eq!(fixture_request_json(&requests[1]), json!({"emoji":"👍","active":false}));
    runtime.shutdown(); drop(runtime);
    let restarted = human_message_fixture_runtime(&root, &url);
    assert_eq!(restarted.read_human_conversation_transcript(&id).expect("pure reopen"), vec![settled]);
    restarted.shutdown(); drop(restarted);
    let _ = fs::remove_dir_all(root);
}

fn exercise_late_human_send_response(
    label: &str,
    late_response: HumanMessageFixtureReply,
    expected_error: Option<&str>,
) {
    let root = temp_root(label);
    let (send_seen, send_wait) = std::sync::mpsc::channel();
    let (send_release, send_resume) = std::sync::mpsc::channel();
    let mut accepted = fixture_remote_human_message(77, 1, 2, "late receipt", Some("late-send"), json!([]));
    accepted["readAt"] = json!("2026-10-05T01:05:00Z");
    accepted["reactions"] = json!([{"emoji":"👍","count":1,"reactedByMe":false}]);
    let (url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Deferred(Box::new(late_response), send_seen, send_resume),
        // Sync retries the same durable nonce before discovering the accepted
        // history row; the original POST remains held throughout both steps.
        HumanMessageFixtureReply::Json(200, json!({"success":true,"deduplicated":true,"message":accepted.clone()})),
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[accepted]}})),
        HumanMessageFixtureReply::Json(200, json!({
            "success":true,"messageId":77,
            "reactions":[{"emoji":"👍","count":2,"reactedByMe":true}]
        })),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("id").to_string();
    let sending = {
        let runtime = Arc::clone(&runtime);
        let id = id.clone();
        thread::spawn(move || runtime.append_human_message(&id, "late receipt", "late-send", Some(1000.0), None, &[]))
    };
    send_wait.recv_timeout(std::time::Duration::from_secs(15)).expect("original POST in flight");
    let synced = runtime.sync_human_conversation(&id).expect("idempotent retry and history settle independently");
    assert_eq!(synced.len(), 1);
    assert_eq!(synced[0]["id"], "human-server-message:77");
    let reacted = runtime.toggle_human_message_reaction(&id, "human-server-message:77", "👍")
        .expect("newer reaction is durably acknowledged before original POST");
    assert_eq!(reacted["readAt"], "2026-10-05T01:05:00Z");
    assert_eq!(reacted["reactions"], json!([{"emoji":"👍","by":"me"},{"emoji":"👍","by":"2"}]));
    send_release.send(()).expect("release original response after newer settlement");
    let result = sending.join().expect("send thread");
    match expected_error {
        Some(expected) => assert!(result.expect_err("conflicting or malformed acknowledgement stays rejected").contains(expected), "{label}"),
        None => assert_eq!(result.expect("durable acknowledgement wins over late transport/state"), reacted, "{label}"),
    }
    assert_eq!(runtime.read_human_conversation_transcript(&id).expect("one settled row"), vec![reacted.clone()]);
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    assert!(owner.get_transcript_entry_by_id("human-message:late-send").expect("local intent lookup").is_none());
    assert_eq!(owner.get_metadata("humanReactionIntent").expect("reaction acknowledged"), Some(serde_json::Value::Null));
    let requests = server.join().expect("late acknowledgement fixture");
    assert_eq!(requests.len(), 4);
    assert!(requests[0].starts_with("POST /api/social/messages "));
    assert!(requests[1].starts_with("POST /api/social/messages "));
    assert_eq!(fixture_request_json(&requests[0]), fixture_request_json(&requests[1]), "retry preserves exact identity/body");
    assert!(requests[2].contains("&afterId=0"));
    assert_eq!(fixture_request_json(&requests[3]), json!({"emoji":"👍","active":true}));
    runtime.shutdown(); drop(owner); drop(runtime);
    let restarted = human_message_fixture_runtime(&root, &url);
    assert_eq!(restarted.read_human_conversation_transcript(&id).expect("restart cannot restore stale receipt"), vec![reacted]);
    restarted.shutdown(); drop(restarted);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_late_send_acknowledgement_preserves_newer_reaction_and_read_receipt() {
    let stale = fixture_remote_human_message(77, 1, 2, "late receipt", Some("late-send"), json!([]));
    exercise_late_human_send_response("human-late-valid-send",
        HumanMessageFixtureReply::Json(201, json!({"success":true,"message":stale})), None);
}

#[test]
fn shipping_human_late_send_transport_failure_returns_durable_settlement_after_restart() {
    exercise_late_human_send_response("human-late-disconnected-send", HumanMessageFixtureReply::Disconnect, None);
    exercise_late_human_send_response("human-late-truncated-success", HumanMessageFixtureReply::Truncated(200), None);
}

#[test]
fn shipping_human_late_invalid_send_acknowledgements_stay_rejected_without_rolling_back_settlement() {
    let mut wrong_nonce = fixture_remote_human_message(77, 1, 2, "late receipt", Some("other-send"), json!([]));
    let mut wrong_identity = wrong_nonce.clone();
    wrong_identity["clientRequestId"] = json!("late-send");
    wrong_identity["id"] = json!(78);
    wrong_nonce["reactions"] = json!([]);
    let mut invalid_reactions = fixture_remote_human_message(77, 1, 2, "late receipt", Some("late-send"), json!([]));
    invalid_reactions["reactions"] = json!([{"emoji":"👍","count":3,"reactedByMe":true}]);
    let mut invalid_timestamp = fixture_remote_human_message(77, 1, 2, "late receipt", Some("late-send"), json!([]));
    invalid_timestamp["createdAt"] = json!("not-a-timestamp");
    let mut invalid_receipt = fixture_remote_human_message(77, 1, 2, "late receipt", Some("late-send"), json!([]));
    invalid_receipt["readAt"] = json!("not-a-timestamp");
    for (label, response, expected) in [
        ("late-missing-message", HumanMessageFixtureReply::Json(201, json!({"success":true})), "omitted the persisted message"),
        ("late-invalid-json", HumanMessageFixtureReply::Bytes(201, b"not-json".to_vec()), "invalid JSON"),
        ("late-permission-rejection", HumanMessageFixtureReply::Json(403, json!({"success":false,"error":"denied"})), "send was rejected (HTTP 403)"),
        ("late-truncated-auth-rejection", HumanMessageFixtureReply::Truncated(401), "response could not be read"),
        ("late-truncated-permission-rejection", HumanMessageFixtureReply::Truncated(403), "response could not be read"),
        ("late-wrong-nonce", HumanMessageFixtureReply::Json(201, json!({"success":true,"message":wrong_nonce})), "mismatched persisted content"),
        ("late-wrong-identity", HumanMessageFixtureReply::Json(201, json!({"success":true,"message":wrong_identity})), "changed a settled remote identity"),
        ("late-invalid-reactions", HumanMessageFixtureReply::Json(201, json!({"success":true,"message":invalid_reactions})), "inconsistent with direct participants"),
        ("late-invalid-timestamp", HumanMessageFixtureReply::Json(201, json!({"success":true,"message":invalid_timestamp})), "timestamp is invalid"),
        ("late-invalid-receipt", HumanMessageFixtureReply::Json(201, json!({"success":true,"message":invalid_receipt})), "read timestamp is invalid"),
    ] {
        exercise_late_human_send_response(label, response, Some(expected));
    }
}

fn exercise_human_send_credential_change(
    with_attachment: bool,
    during_request: bool,
    field: &str,
    replacement: serde_json::Value,
    expected_error: &str,
) {
    let root = temp_root(&format!("human-credential-{with_attachment}-{during_request}-{field}"));
    let resource = json!({
        "resourceId":"credential-resource","name":"private.bin","contentType":"application/octet-stream",
        "size":4,"createdAt":"2026-10-05T01:00:00Z"
    });
    let remote = fixture_remote_human_message(91, 1, 2, "credential-bound send", Some("credential-send"),
        if with_attachment { json!([resource.clone()]) } else { json!([]) });
    let (seen, waiting) = std::sync::mpsc::channel();
    let (release, resume) = std::sync::mpsc::channel();
    let mut replies = Vec::new();
    if during_request {
        let first = if with_attachment {
            HumanMessageFixtureReply::Json(201, json!({"success":true,"resource":resource.clone()}))
        } else {
            HumanMessageFixtureReply::Json(201, json!({"success":true,"message":remote.clone()}))
        };
        replies.push(HumanMessageFixtureReply::Deferred(Box::new(first), seen, resume));
    }
    if with_attachment {
        replies.push(HumanMessageFixtureReply::Json(201, json!({"success":true,"resource":resource})));
    }
    replies.push(HumanMessageFixtureReply::Json(201, json!({"success":true,"message":remote})));
    if with_attachment {
        replies.push(HumanMessageFixtureReply::Bytes(200, vec![1, 2, 3, 4]));
    }
    let (url, server) = spawn_human_message_fixture(replies);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("id").to_string();
    let attachment_path = root.join("private.bin");
    fs::write(&attachment_path, [1, 2, 3, 4]).expect("attachment fixture");
    let attachments = if with_attachment {
        vec![json!({"path":attachment_path.to_string_lossy(),"name":"private.bin"})]
    } else { vec![] };
    let change_credentials = || {
        let path = root.join("host-credential.json");
        let mut credentials: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).expect("read private fixture credential"))
            .expect("credential JSON");
        credentials[field] = replacement.clone();
        fs::write(path, credentials.to_string()).expect("replace same-issuer credential");
    };
    let failure = if during_request {
        let sending = {
            let runtime = Arc::clone(&runtime);
            let id = id.clone();
            let attachments = attachments.clone();
            thread::spawn(move || runtime.append_human_message(&id, "credential-bound send", "credential-send", Some(1000.0), None, &attachments))
        };
        waiting.recv_timeout(std::time::Duration::from_secs(15)).expect("request accepted under original credential");
        change_credentials();
        release.send(()).expect("release response after credential change");
        sending.join().expect("send thread").expect_err("changed credential response is rejected")
    } else {
        change_credentials();
        runtime.append_human_message(&id, "credential-bound send", "credential-send", Some(1000.0), None, &attachments)
            .expect_err("changed-account or invalid credential is rejected before dispatch")
    };
    assert!(failure.contains(expected_error), "{failure}");
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    let pending = owner.get_transcript_entries().expect("recoverable outbox");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["id"], "human-message:credential-send");
    assert_eq!(pending[0]["delivery"], "pending");
    assert!(pending[0].get("remoteMessageId").is_none());
    assert!(pending[0].get("remoteDispatchAttachments").is_none(), "foreign/stale upload response is not reused");
    runtime.shutdown(); drop(owner); drop(runtime);

    // Reopen restores the original private account fixture. Retry keeps the
    // same durable nonce and content; no stale response supplied a settlement.
    let resumed = human_message_fixture_runtime(&root, &url);
    let settled = resumed.append_human_message(&id, "credential-bound send", "credential-send", Some(1000.0), None, &attachments)
        .expect("original account recovers pending send");
    assert_eq!(settled["id"], "human-server-message:91");
    assert_eq!(settled["delivery"], "sent");
    let requests = server.join().expect("credential boundary fixture");
    assert_eq!(requests.len(), (if with_attachment { 3 } else { 1 }) + usize::from(during_request));
    let message_posts = requests.iter().filter(|request| request.starts_with("POST /api/social/messages ")).collect::<Vec<_>>();
    assert_eq!(message_posts.len(), if during_request && !with_attachment { 2 } else { 1 });
    assert!(message_posts.iter().all(|request| fixture_request_json(request)["clientRequestId"] == "credential-send"));
    assert_eq!(requests.iter().filter(|request| request.starts_with("POST /api/social/message-resources ")).count(),
        if with_attachment { 1 + usize::from(during_request) } else { 0 }, "pre-dispatch account rejection sends no bytes");
    resumed.shutdown(); drop(resumed);
    let reopened = human_message_fixture_runtime(&root, &url);
    assert_eq!(reopened.read_human_conversation_transcript(&id).expect("pure reopen"), vec![settled]);
    reopened.shutdown(); drop(reopened);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_send_rejects_changed_account_before_message_or_resource_dispatch() {
    for with_attachment in [false, true] {
        exercise_human_send_credential_change(with_attachment, false, "userId", json!(3), "changed Human account");
    }
    exercise_human_send_credential_change(false, false, "accessToken", json!("invalid\nheader"), "send failed");
}

#[test]
fn shipping_human_send_rejects_midflight_credential_changes_and_recovers_after_restart() {
    for with_attachment in [false, true] {
        for (field, replacement) in [("userId", json!(3)), ("deviceId", json!("device-b")), ("accessToken", json!("replacement-token"))] {
            exercise_human_send_credential_change(with_attachment, true, field, replacement, "changed during Human request");
        }
    }
}

#[test]
fn shipping_human_read_receipts_compare_rfc3339_instants_and_reject_invalid_ingress_before_resources() {
    let root = temp_root("human-read-receipt-instants");
    let make_row = |id, read_at| {
        let mut row = fixture_remote_human_message(id, 2, 1, "receipt target", None, json!([]));
        row["readAt"] = read_at;
        row
    };
    let initial = "2026-10-05T00:20:00.500Z";
    let advanced = "2026-10-04T20:20:00.750-04:00";
    let precise = "2026-10-05T00:20:00.750000001Z";
    let repaired = "2026-10-05T00:30:00.250Z";
    let mut rows = vec![
        make_row(81, json!(initial)),
        make_row(81, json!("2026-10-05T01:10:00+01:00")),
        make_row(81, json!("2026-10-05T01:20:00.5000+01:00")),
        make_row(81, json!(advanced)),
        make_row(81, json!(precise)),
        make_row(81, serde_json::Value::Null),
    ];
    for id in [81, 82] {
        let mut invalid = make_row(id, json!("not-rfc3339"));
        invalid["attachments"] = json!([{
            "resourceId":"forbidden-invalid-receipt-resource","name":"private.bin",
            "contentType":"application/octet-stream","size":4,
            "createdAt":"2026-10-05T01:00:00Z"
        }]);
        rows.push(invalid);
    }
    rows.push(make_row(81, serde_json::Value::Null));
    rows.push(make_row(81, json!(repaired)));
    let (url, server) = spawn_human_message_fixture(rows.into_iter().map(|row| {
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[row]}}))
    }).collect());
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("id").to_string();
    for (stage, expected) in [("initial", initial), ("older offset", initial),
        ("equivalent fractional instant", initial), ("newer offset", advanced),
        ("nanosecond advancement", precise), ("omitted", precise)]
    {
        let current = runtime.sync_human_conversation(&id).expect(stage);
        assert_eq!(current.len(), 1, "{stage}");
        assert_eq!(current[0]["id"], "human-server-message:81", "{stage}");
        assert_eq!(current[0]["readAt"], expected, "{stage}");
    }
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    let prior = owner.get_transcript_entries().expect("prior receipt");
    let coverage = owner.get_metadata("humanHistoryCoverage").expect("prior coverage");
    for stage in ["known row", "new row"] {
        let error = runtime.sync_human_conversation(&id).expect_err("invalid receipt fails before resource access");
        assert!(error.contains("read timestamp is invalid"), "{stage}: {error}");
        assert_eq!(owner.get_transcript_entries().expect("no durable mutation"), prior, "{stage}");
        assert_eq!(owner.get_metadata("humanHistoryCoverage").expect("no coverage advance"), coverage, "{stage}");
        assert!(!root.join("agents").join("human-message-resources").exists(), "{stage}");
    }
    let mut legacy = prior[0].clone();
    legacy["readAt"] = json!("legacy-invalid-receipt");
    owner.update_transcript_entry("human-server-message:81", &legacy).expect("legacy fixture").expect("target");
    assert_eq!(runtime.sync_human_conversation(&id).expect("omitted field preserves legacy value"), vec![legacy.clone()]);
    runtime.shutdown(); drop(owner); drop(runtime);
    let restarted = human_message_fixture_runtime(&root, &url);
    assert_eq!(restarted.read_human_conversation_transcript(&id).expect("reopen preserves omitted legacy receipt"), vec![legacy]);
    let repaired_rows = restarted.sync_human_conversation(&id).expect("valid receipt repairs legacy invalid value");
    assert_eq!(repaired_rows.len(), 1);
    assert_eq!(repaired_rows[0]["readAt"], repaired);
    let requests = server.join().expect("receipt fixture");
    assert_eq!(requests.len(), 10);
    assert!(requests.iter().all(|request| request.starts_with("GET /api/social/messages?contactId=2&limit=200&afterId=0 ")),
        "invalid timestamps never initiate resource requests");
    restarted.shutdown(); drop(restarted);
    let reopened = human_message_fixture_runtime(&root, &url);
    assert_eq!(reopened.read_human_conversation_transcript(&id).expect("pure reopen of repaired receipt"), repaired_rows);
    reopened.shutdown(); drop(reopened);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_reaction_rejects_midflight_credential_rotation_and_recovers_after_restart() {
    // The account-bound HTTP request is already dispatched when the private
    // credential rotates. A late 200 response must not settle the old intent.
    for (label, changed) in [
        ("token", json!({"accessToken":"replacement-token"})),
        ("device", json!({"deviceId":"replacement-device"})),
        ("account", json!({"userId":99})),
        ("session", json!({"sessionId":"replacement-session"})),
        ("expiry", json!({"expiresAtMs":12345})),
        ("issuer", json!({"issuer":"https://other.example"})),
    ] {
        let root = temp_root(&format!("human-reaction-credential-rotation-{label}"));
        let (request_seen, wait_for_request) = std::sync::mpsc::channel();
        let (release_response, wait_for_release) = std::sync::mpsc::channel();
        let successful = json!({
            "success":true,"messageId":777,
            "reactions":[{"emoji":"👍","count":1,"reactedByMe":true}]
        });
        let (url, server) = spawn_human_message_fixture(vec![
            HumanMessageFixtureReply::Deferred(
                Box::new(HumanMessageFixtureReply::Json(200, successful.clone())),
                request_seen, wait_for_release,
            ),
            HumanMessageFixtureReply::Json(200, successful),
        ]);
        let runtime = human_message_fixture_runtime(&root, &url);
        let id = runtime.create_human_conversation("2", "Peer")
            .expect("conversation")["id"].as_str().expect("id").to_string();
        let entry_id = "human-server-message:777";
        let original = json!({
            "id":entry_id,"kind":"message","role":"user","authorKind":"human",
            "authorId":"2","content":"retain on credential rotation","timestampMs":1000.0,
            "delivery":"sent","remoteMessageId":"777","remoteReactions":[],"reactions":[]
        });
        let owner = runtime.open_human_conversation_db_owner(&id).expect("durable owner");
        assert!(owner.append_transcript_entry(&original).expect("seed"));
        let running = {
            let runtime = Arc::clone(&runtime);
            let id = id.clone();
            thread::spawn(move || runtime.toggle_human_message_reaction(&id, entry_id, "👍"))
        };
        wait_for_request.recv_timeout(std::time::Duration::from_secs(15))
            .expect("old credential POST is in flight");
        let credential_path = root.join("host-credential.json");
        let mut credential: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&credential_path).expect("original private credential")
        ).expect("credential JSON");
        for (key, value) in changed.as_object().expect("credential patch") {
            credential[key] = value.clone();
        }
        fs::write(&credential_path, credential.to_string()).expect("rotate credential");
        release_response.send(()).expect("release obsolete success response");
        let error = running.join().expect("reaction thread")
            .expect_err("obsolete response must not commit");
        if label == "issuer" {
            assert_eq!(
                error,
                "Fabushi messaging API must use an approved Native issuer origin.",
                "{label}",
            );
        } else {
            assert!(error.contains("credential"), "{label}: {error}");
        }
        assert_eq!(owner.get_transcript_entries().expect("no stale projection"), vec![original.clone()], "{label}");
        assert_eq!(owner.get_metadata("humanReactionIntent").expect("retained intent"), Some(json!({
            "entryId":entry_id,"remoteMessageId":"777",
            "emoji":"👍","active":true,"localHumanId":"1"
        })), "{label}");
        runtime.shutdown();
        drop(owner);
        drop(runtime);

        // Restoring the same account's valid credential after restart must
        // retry the desired state, never toggle it back or duplicate the row.
        let resumed = human_message_fixture_runtime(&root, &url);
        let settled = resumed.toggle_human_message_reaction(&id, entry_id, "👍")
            .expect("restart replays durable intent");
        assert_eq!(settled["remoteReactions"], json!([
            {"emoji":"👍","count":1,"reactedByMe":true}
        ]), "{label}");
        let owner = resumed.open_human_conversation_db_owner(&id).expect("reopen");
        assert_eq!(owner.get_metadata("humanReactionIntent").expect("ack"), Some(serde_json::Value::Null), "{label}");
        assert_eq!(owner.get_transcript_entries().expect("single settled entry"), vec![settled], "{label}");
        let requests = server.join().expect("both authenticated POSTs");
        assert_eq!(requests.len(), 2, "{label}");
        for request in requests {
            assert!(request.starts_with("POST /api/social/messages/777/reactions "), "{label}");
            assert_eq!(fixture_request_json(&request), json!({"emoji":"👍","active":true}), "{label}");
        }
        resumed.shutdown();
        drop(owner);
        drop(resumed);
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn shipping_human_reaction_rejects_credential_change_while_response_body_is_streaming() {
    let root = temp_root("human-reaction-credential-body-race");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind streaming reaction fixture");
    let url = format!("http://{}", listener.local_addr().expect("bound address"));
    let (body_started, wait_for_body) = std::sync::mpsc::channel();
    let (release_body, wait_for_release) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept reaction POST");
        let request = read_fixture_http_request(&mut stream);
        let body = json!({
            "success":true,"messageId":778,
            "reactions":[{"emoji":"👍","count":1,"reactedByMe":true}]
        }).to_string();
        write!(stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        ).expect("write headers");
        stream.write_all(&body.as_bytes()[..1]).expect("partial response body");
        stream.flush().expect("send headers and first body byte");
        body_started.send(()).expect("notify response is streaming");
        wait_for_release.recv_timeout(std::time::Duration::from_secs(15))
            .expect("explicitly release final response body");
        // Credential fencing may reject after the headers, before the rest
        // of this body is consumed. That valid cancellation closes the socket;
        // the caller below still requires the credential error, unchanged
        // durable state and successful restart replay.
        if let Err(error) = stream.write_all(&body.as_bytes()[1..]).and_then(|_| stream.flush()) {
            assert!(matches!(error.kind(),
                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
            ), "unexpected streaming fixture delivery failure: {error}");
        }
        request
    });
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2","Peer").expect("conversation")["id"]
        .as_str().expect("id").to_string();
    let entry_id = "human-server-message:778";
    let original = json!({
        "id":entry_id,"kind":"message","role":"user","authorKind":"human",
        "authorId":"2","content":"body-read race","timestampMs":1000.0,
        "delivery":"sent","remoteMessageId":"778","remoteReactions":[],"reactions":[]
    });
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    assert!(owner.append_transcript_entry(&original).expect("seed"));
    let running = {
        let runtime = Arc::clone(&runtime);
        let id = id.clone();
        thread::spawn(move || runtime.toggle_human_message_reaction(&id, entry_id, "👍"))
    };
    wait_for_body.recv_timeout(std::time::Duration::from_secs(15))
        .expect("HTTP headers received while body is incomplete");
    let credential_path = root.join("host-credential.json");
    let mut credential: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&credential_path).expect("private credential")
    ).expect("credential JSON");
    credential["accessToken"] = json!("rotated-during-body");
    fs::write(&credential_path, credential.to_string()).expect("rotate during body");
    release_body.send(()).expect("release body");
    let error = running.join().expect("reaction thread").expect_err("stale body response");
    assert!(error.contains("credential"), "{error}");
    assert_eq!(owner.get_transcript_entries().expect("unmodified target"), vec![original]);
    assert_eq!(owner.get_metadata("humanReactionIntent").expect("pending intent").expect("staged intent")["active"], true);
    assert!(server.join().expect("old POST").starts_with("POST /api/social/messages/778/reactions "));
    runtime.shutdown();
    drop(owner);
    drop(runtime);

    let (recovered_url, replay_server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(200, json!({
            "success":true,"messageId":778,
            "reactions":[{"emoji":"👍","count":1,"reactedByMe":true}]
        }))
    ]);
    let resumed = human_message_fixture_runtime(&root, &recovered_url);
    let recovered = resumed.toggle_human_message_reaction(&id, entry_id, "👍")
        .expect("durable desired state replayed");
    assert_eq!(recovered["reactions"], json!([{"emoji":"👍","by":"me"}]));
    assert_eq!(replay_server.join().expect("single replay").len(), 1);
    resumed.shutdown();
    drop(resumed);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_send_fences_credential_rotation_while_waiting_to_settle_after_http() {
    for (label, first_reply) in [
        ("accepted", HumanMessageFixtureReply::Json(201, json!({
            "success":true,"message":fixture_remote_human_message(
                96, 1, 2, "post-lock lease", Some("post-lock-lease"), json!([])
            )
        }))),
        ("disconnected", HumanMessageFixtureReply::Disconnect),
    ] {
        let root = temp_root(&format!("human-post-lock-credential-{label}"));
        let (send_seen, send_wait) = std::sync::mpsc::channel();
        let (send_release, send_resume) = std::sync::mpsc::channel();
        let (retry_seen, retry_wait) = std::sync::mpsc::channel();
        let (retry_release, retry_resume) = std::sync::mpsc::channel();
        let (url, server) = spawn_human_message_fixture(vec![
            HumanMessageFixtureReply::Deferred(Box::new(first_reply), send_seen, send_resume),
            // Sync owns the Session mutation lock during this pending-send retry.
            HumanMessageFixtureReply::Deferred(
                Box::new(HumanMessageFixtureReply::Json(503, json!({
                    "success":false,"error":"temporary backend failure"
                }))),
                retry_seen, retry_resume,
            ),
            HumanMessageFixtureReply::Json(200, json!({
                "success":true,"data":{"messages":[]}
            })),
            HumanMessageFixtureReply::Json(201, json!({
                "success":true,"message":fixture_remote_human_message(
                    96, 1, 2, "post-lock lease", Some("post-lock-lease"), json!([])
                )
            })),
        ]);
        let runtime = human_message_fixture_runtime(&root, &url);
        let conversation_id = runtime.create_human_conversation("2", "Peer")
            .expect("conversation")["id"].as_str().expect("id").to_string();
        let sending = {
            let runtime = Arc::clone(&runtime);
            let id = conversation_id.clone();
            thread::spawn(move || runtime.append_human_message(
                &id, "post-lock lease", "post-lock-lease", Some(1000.0), None, &[]
            ))
        };
        send_wait.recv_timeout(std::time::Duration::from_secs(15))
            .expect("original HTTP request accepted");
        let syncing = {
            let runtime = Arc::clone(&runtime);
            let id = conversation_id.clone();
            thread::spawn(move || runtime.sync_human_conversation(&id))
        };
        retry_wait.recv_timeout(std::time::Duration::from_secs(15))
            .expect("sync holds the Session lock for retry");
        send_release.send(()).expect("release older HTTP response");
        // This integration case proves pending-state and restart recovery
        // regardless of whether transport or settlement observes rotation.
        // The private shipping_human_settlement_credential_ tests use an
        // explicit HTTP-completion barrier to prove the post-lock boundary.
        let path = root.join("host-credential.json");
        let mut credential: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&path).expect("private credential")
        ).expect("credential JSON");
        credential["accessToken"] = json!("rotated-after-http");
        let replacement_path = root.join("host-credential-rotated.json");
        fs::write(&replacement_path, credential.to_string()).expect("write complete rotated credential");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&replacement_path, fs::Permissions::from_mode(0o600))
                .expect("private rotated credential");
        }
        fs::rename(&replacement_path, &path).expect("atomically rotate same-user bearer");
        retry_release.send(()).expect("release sync lock");
        syncing.join().expect("sync thread").expect("independent empty coverage");
        let error = sending.join().expect("original send")
            .expect_err("obsolete response cannot settle");
        assert!(matches!(error.as_str(),
            "Fabushi native messaging credential changed during Human request"
                | "Fabushi native messaging credential changed before Human settlement"
        ), "{label}: {error}");
        let owner = runtime.open_human_conversation_db_owner(&conversation_id).expect("owner");
        let entries = owner.get_transcript_entries().expect("durable outbox");
        assert_eq!(entries.len(), 1, "{label}");
        assert_eq!(entries[0]["id"], "human-message:post-lock-lease", "{label}");
        assert_eq!(entries[0]["delivery"], "pending", "{label}");
        assert!(entries[0].get("remoteMessageId").is_none(), "{label}");
        runtime.shutdown(); drop(owner); drop(runtime);

        // A fresh Host restores its original account fixture and retries the
        // persisted client nonce, never reusing the stale response.
        let resumed = human_message_fixture_runtime(&root, &url);
        let settled = resumed.append_human_message(
            &conversation_id, "post-lock lease", "post-lock-lease", Some(1000.0), None, &[]
        ).expect("replay from stable account");
        assert_eq!(settled["id"], "human-server-message:96", "{label}");
        let requests = server.join().expect("scripted fixture");
        assert_eq!(requests.len(), 4, "{label}");
        assert!(requests[0].starts_with("POST /api/social/messages "), "{label}");
        assert!(requests[1].starts_with("POST /api/social/messages "), "{label}");
        assert!(requests[2].contains("&afterId=0"), "{label}");
        assert!(requests[3].starts_with("POST /api/social/messages "), "{label}");
        for index in [0, 1, 3] {
            assert_eq!(
                fixture_request_json(&requests[index])["clientRequestId"],
                "post-lock-lease", "{label}"
            );
        }
        resumed.shutdown(); drop(resumed);
        let _ = fs::remove_dir_all(root);
    }
}

// NMR-47: these integration cases use the shipping gateway, an independently
// opened file-backed SQLite database and an actual child test process. The
// server accepts the first POST before releasing its response, making process
// termination and durable-progress failure deterministic rather than timed.
#[derive(Default)]
struct ForwardHttpState {
    requests: Vec<String>,
    accepted: std::collections::BTreeMap<String, serde_json::Value>,
    resources: std::collections::BTreeMap<String, (serde_json::Value, Vec<u8>)>,
    uploaded_files: Vec<(String, Vec<u8>)>,
    observe_source: Option<(std::path::PathBuf, String)>,
    first_upload_snapshot: Option<(serde_json::Value, Vec<Vec<u8>>)>,
    mutate_second_pin_after_first_upload: bool,
    mutated_second_pin: Option<(std::path::PathBuf, Vec<u8>)>,
    mutation_observed_by_second_upload: bool,
    mutation_restored_before_message_ack: bool,
}

// Keep multipart payloads binary. The older text helper is sufficient for JSON
// contracts but its UTF-8 replacement would destroy this test's byte oracle.
fn read_forward_http_bytes(stream: &mut std::net::TcpStream) -> Vec<u8> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    let mut expected = None;
    loop {
        let remaining = deadline.checked_duration_since(std::time::Instant::now())
            .filter(|duration| !duration.is_zero()).expect("forward HTTP total deadline");
        stream.set_read_timeout(Some(remaining)).expect("bounded binary request");
        let count = stream.read(&mut chunk).expect("read binary forward request");
        assert_ne!(count, 0, "forward client closed an incomplete request");
        bytes.extend_from_slice(&chunk[..count]);
        if expected.is_none() {
            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&bytes[..end]).expect("HTTP headers are text");
                assert!(!headers.to_ascii_lowercase().contains("transfer-encoding: chunked"),
                    "the bytes-backed multipart client must provide a bounded Content-Length");
                let length = headers.lines().find_map(|line| line.split_once(':')
                    .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse::<usize>().expect("request length")))
                    .unwrap_or(0);
                assert!(length <= 32 * 1024 * 1024 + 64 * 1024, "fixture body budget");
                expected = Some(end + 4 + length);
            } else {
                assert!(bytes.len() <= 64 * 1024, "fixture header budget");
            }
        }
        if let Some(expected) = expected {
            if bytes.len() >= expected {
                assert_eq!(bytes.len(), expected, "one complete request per connection");
                return bytes;
            }
        }
    }
}

fn forward_multipart_file(request: &[u8]) -> (String, Vec<u8>) {
    let header_end = request.windows(4).position(|part| part == b"\r\n\r\n")
        .expect("multipart HTTP headers");
    let headers = std::str::from_utf8(&request[..header_end]).expect("multipart headers");
    let content_type = headers.lines().find_map(|line| line.split_once(':')
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.trim())).expect("multipart content type");
    assert!(content_type.starts_with("multipart/form-data;"));
    let boundary = content_type.split(';').find_map(|value|
        value.trim().strip_prefix("boundary=")).expect("multipart boundary").trim_matches('"');
    let body = &request[header_end + 4..];
    let prefix = format!("--{boundary}\r\n");
    let suffix = format!("\r\n--{boundary}--\r\n");
    assert!(body.starts_with(prefix.as_bytes()));
    assert!(body.ends_with(suffix.as_bytes()), "single file part has an exact closing boundary");
    let part_end = body.windows(4).position(|part| part == b"\r\n\r\n")
        .expect("multipart file headers");
    let part_headers = std::str::from_utf8(&body[prefix.len()..part_end]).expect("file headers");
    assert!(part_headers.contains("name=\"file\""));
    let name = part_headers.split("filename=\"").nth(1)
        .and_then(|tail| tail.split('"').next()).expect("file name").to_string();
    (name, body[part_end + 4..body.len() - suffix.len()].to_vec())
}

struct ForwardHttpFixture {
    url: String,
    state: Arc<std::sync::Mutex<ForwardHttpState>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    first_post: std::sync::mpsc::Receiver<()>,
    release: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ForwardHttpFixture {
    fn start(hold_first_post: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("forward listener");
        listener.set_nonblocking(true).expect("bounded forward listener");
        let url = format!("http://{}", listener.local_addr().expect("forward address"));
        let state = Arc::new(std::sync::Mutex::new(ForwardHttpState::default()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (seen, first_post) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let thread_state = Arc::clone(&state);
        let thread_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let mut post_count = 0;
            while !thread_stop.load(std::sync::atomic::Ordering::Acquire) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        // Accept polling is bounded by the fixture's owned stop
                        // flag; every behavior boundary uses a channel or return.
                        thread::sleep(std::time::Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("forward accept: {error}"),
                };
                stream.set_nonblocking(false).expect("blocking forward stream");
                stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))
                    .expect("bounded forward write");
                let request_bytes = read_forward_http_bytes(&mut stream);
                let request = String::from_utf8_lossy(&request_bytes).into_owned();
                assert!(request.to_ascii_lowercase().contains("authorization: bearer fixture-token"));
                let is_post = request.starts_with("POST /api/social/messages ");
                let (body, content_type, status) = {
                    let mut state = thread_state.lock().expect("forward HTTP state");
                    state.requests.push(request.clone());
                    if request.starts_with("POST /api/social/message-resources ") {
                        if state.uploaded_files.len() == 1 {
                            if let Some((path, original)) = state.mutated_second_pin.as_ref() {
                                assert_eq!(fs::read(path).expect("pin remains altered during the next upload"),
                                    vec![b'X'; original.len()]);
                                state.mutation_observed_by_second_upload = true;
                            }
                        }
                        if state.uploaded_files.is_empty() {
                            if let Some((root, source)) = state.observe_source.clone() {
                                // Observe committed SQLite and the complete byte set
                                // before even the first resource upload is acknowledged.
                                let metadata = read_forward_metadata_from_disk(&root, &source);
                                let snapshots = metadata["humanForwardBatches"][0]["attachments"]
                                    .as_array().into_iter().flatten().map(|attachment|
                                        fs::read(attachment["path"].as_str().expect("staged file path"))
                                            .expect("snapshot exists before resource POST")
                                    ).collect();
                                state.first_upload_snapshot = Some((metadata, snapshots));
                            }
                        }
                        let (name, bytes) = forward_multipart_file(&request_bytes);
                        let id = format!("forward-resource-{}", state.resources.len() + 1);
                        let resource = json!({
                            "resourceId":id,"name":name,"size":bytes.len(),
                            "contentType":"application/octet-stream","createdAt":"2026-10-05T01:00:00Z"
                        });
                        state.uploaded_files.push((name, bytes.clone()));
                        state.resources.insert(id, (resource.clone(), bytes));
                        if state.uploaded_files.len() == 1 && state.mutate_second_pin_after_first_upload {
                            let (metadata, snapshots) = state.first_upload_snapshot.as_ref()
                                .expect("observe the committed snapshots before mutation");
                            let path = std::path::PathBuf::from(metadata["humanForwardBatches"][0]
                                ["attachments"][1]["path"].as_str().expect("second pin path"));
                            let original = snapshots[1].clone();
                            assert_ne!(original, vec![b'X'; original.len()]);
                            // The server has accepted the first file, while the
                            // client has not yet received its acknowledgement.
                            fs::write(&path, vec![b'X'; original.len()])
                                .expect("replace the next pin after preflight with same-length bytes");
                            state.mutated_second_pin = Some((path, original));
                        }
                        (json!({"success":true,"resource":resource}).to_string().into_bytes(),
                            "application/json", 201)
                    } else if request.starts_with("GET /api/social/message-resources/") {
                        let path = request.split_whitespace().nth(1).expect("resource GET path");
                        let id = path.strip_prefix("/api/social/message-resources/").expect("resource prefix");
                        let (_, bytes) = state.resources.get(id).expect("download only uploaded resource");
                        (bytes.clone(), "application/octet-stream", 200)
                    } else if is_post {
                        post_count += 1;
                        if post_count == 1 {
                            if let Some((path, original)) = state.mutated_second_pin.take() {
                                assert!(state.mutation_observed_by_second_upload,
                                    "next upload happened while its pin held different bytes");
                                fs::write(path, original).expect("restore pin before the next destination preflight");
                                state.mutation_restored_before_message_ack = true;
                            }
                        }
                        let submitted = fixture_request_json(&request);
                        let nonce = submitted["clientRequestId"].as_str().expect("forward nonce");
                        let peer = submitted["targetUserId"].as_str().expect("forward target")
                            .parse::<u64>().expect("numeric forward target");
                        let text = submitted["text"].as_str().expect("forward text");
                        let attachments = submitted["attachments"].as_array().into_iter().flatten()
                            .map(|item| {
                                let id = item["resourceId"].as_str().expect("submitted resource identity");
                                state.resources.get(id).expect("message binds an actually uploaded resource").0.clone()
                            }).collect::<Vec<_>>();
                        let existing = state.accepted.get(nonce).cloned();
                        let deduplicated = existing.is_some();
                        let message = existing.unwrap_or_else(|| fixture_remote_human_message(
                            70 + state.accepted.len() as u64, 1, peer, text, Some(nonce), json!(attachments)
                        ));
                        assert_eq!(message["recipientUserId"], peer, "nonce cannot rebind recipient");
                        assert_eq!(message["text"], text, "nonce cannot rebind submitted snapshot");
                        assert_eq!(message["attachments"], json!(attachments), "nonce cannot rebind uploaded resources");
                        state.accepted.insert(nonce.to_string(), message.clone());
                        (json!({"success":true,"deduplicated":deduplicated,"message":message})
                            .to_string().into_bytes(), "application/json", 200)
                    } else {
                        assert!(request.starts_with("GET /api/social/messages?"), "{request}");
                        let after = request.split("afterId=").nth(1)
                            .and_then(|tail| tail.split(|c: char| !c.is_ascii_digit()).next())
                            .and_then(|value| value.parse::<u64>().ok()).expect("coverage cursor");
                        (json!({"success":true,"data":{
                            "messages":[],"historyCursor":{"mode":"after-id","afterId":after},
                            "nextAfterId":after
                        }}).to_string().into_bytes(), "application/json", 200)
                    }
                };
                if is_post && post_count == 1 {
                    seen.send(()).expect("first POST observation");
                    if hold_first_post {
                        released.recv_timeout(std::time::Duration::from_secs(30))
                            .expect("first POST must be explicitly released");
                    }
                }
                let response = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let result = stream.write_all(response.as_bytes()).and_then(|_| stream.write_all(&body));
                // The killed child deliberately cannot receive the first ack.
                // All subsequent response writes must still be successful.
                if !(hold_first_post && is_post && post_count == 1) {
                    result.expect("forward response write");
                }
            }
        });
        Self { url, state, stop, first_post, release: Some(release), thread: Some(worker) }
    }

    fn wait_for_first_post(&self) {
        self.first_post.recv_timeout(std::time::Duration::from_secs(20))
            .expect("shipping process reached first POST");
    }

    fn release_first_post(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }

    fn post_count(&self) -> usize {
        self.state.lock().expect("forward requests").requests.iter()
            .filter(|request| request.starts_with("POST /api/social/messages ")).count()
    }

    fn history_count(&self) -> usize {
        self.state.lock().expect("forward history requests").requests.iter()
            .filter(|request| request.starts_with("GET /api/social/messages?")).count()
    }

    fn resource_post_count(&self) -> usize {
        self.state.lock().expect("forward resources").uploaded_files.len()
    }

    fn observe_snapshot_staging(&self, root: &std::path::Path, source: &str) {
        self.state.lock().expect("forward observer").observe_source =
            Some((root.to_path_buf(), source.to_string()));
    }

    fn mutate_second_pin_after_first_upload(&self) {
        self.state.lock().expect("owned pin mutation")
            .mutate_second_pin_after_first_upload = true;
    }

    fn finish(mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        self.release_first_post();
        self.thread.take().expect("owned fixture thread").join().expect("forward fixture");
    }
}

impl Drop for ForwardHttpFixture {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        self.release_first_post();
        if let Some(worker) = self.thread.take() { let _ = worker.join(); }
    }
}

struct ForwardProcess(std::process::Child);

impl ForwardProcess {
    fn start(root: &std::path::Path, url: &str) -> Self {
        Self(std::process::Command::new(std::env::current_exe().expect("integration executable"))
            .args(["--exact", "forward_process_child_entry", "--nocapture"])
            .env("FABUSHI_FORWARD_PROCESS_ROOT", root)
            .env("FABUSHI_FORWARD_PROCESS_URL", url)
            .spawn().expect("actual forward child process"))
    }

    fn terminate(&mut self) {
        assert!(self.0.try_wait().expect("inspect forward child").is_none(),
            "child must still be in the held HTTP request");
        self.0.kill().expect("terminate exact owned forward process");
        let status = self.0.wait().expect("reap terminated forward process");
        assert!(!status.success(), "termination cannot be normal Session shutdown");
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(status.signal(), Some(9), "require actual SIGKILL");
        }
    }
}

impl Drop for ForwardProcess {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

// A helper entry, not an extra selected shipping_human_ case. It has no
// side effects under an ordinary unfiltered test invocation.
#[test]
fn forward_process_child_entry() {
    let Some(root) = std::env::var_os("FABUSHI_FORWARD_PROCESS_ROOT") else { return; };
    let root = std::path::PathBuf::from(root);
    let url = std::env::var("FABUSHI_FORWARD_PROCESS_URL").expect("child fixture issuer");
    let args: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("forward-input.json")).expect("child input")
    ).expect("forward input JSON");
    let runtime = human_message_fixture_runtime(&root, &url);
    let _ = dispatch(&runtime, "forwardHumanMessage", args);
    panic!("parent must terminate the child before releasing the first response");
}

fn prepare_forward_process(root: &std::path::Path, url: &str) -> serde_json::Value {
    let runtime = human_message_fixture_runtime(root, url);
    let source = runtime.create_human_conversation("2", "Source").expect("source")["id"]
        .as_str().expect("source id").to_string();
    let first = runtime.create_human_conversation("3", "First").expect("first target")["id"]
        .as_str().expect("first id").to_string();
    let second = runtime.create_human_conversation("4", "Second").expect("second target")["id"]
        .as_str().expect("second id").to_string();
    let owner = runtime.open_human_conversation_db_owner(&source).expect("source owner");
    assert!(owner.append_transcript_entry(&json!({
        "id":"human-server-message:51","kind":"message","role":"user",
        "authorKind":"human","authorId":"2","authorName":"Source author",
        "content":"immutable forwarded snapshot","timestampMs":1000.0,"delivery":"sent",
        "remoteMessageId":"51","remoteCreatedAt":"1970-01-01T00:00:01Z"
    })).expect("canonical source row"));
    let args = json!({
        "sourceConversationId":source,"sourceEntryId":"human-server-message:51",
        "destinationConversationIds":[first,second],"clientNonce":"durable-process-forward",
        "dropSenderNames":false,"dropCaptions":false
    });
    fs::write(root.join("forward-input.json"), args.to_string()).expect("persist child arguments");
    runtime.shutdown();
    drop(owner); drop(runtime);
    args
}

fn forward_db_path(root: &std::path::Path, conversation: &str) -> std::path::PathBuf {
    root.join("agents").join(".conversations").join(conversation).join("store.db")
}

fn read_forward_metadata_from_disk(root: &std::path::Path, conversation: &str) -> serde_json::Value {
    let db = rusqlite::Connection::open_with_flags(
        forward_db_path(root, conversation), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
    ).expect("independent read-only SQLite connection");
    let hex: String = db.query_row("SELECT value FROM kv WHERE key = 'metadata'", [],
        |row| row.get(0)).expect("committed metadata row");
    assert_eq!(hex.len() % 2, 0);
    let bytes = (0..hex.len()).step_by(2).map(|offset|
        u8::from_str_radix(&hex[offset..offset + 2], 16).expect("metadata hex")
    ).collect::<Vec<_>>();
    serde_json::from_slice(&bytes).expect("committed metadata JSON")
}

fn forward_gateway_result(runtime: &Arc<ProductionSessionWorkers>, args: &serde_json::Value)
    -> Result<serde_json::Value, SessionGatewayError>
{
    dispatch_production_session_gateway_call(runtime, "forwardHumanMessage", args)
        .expect("registered forward gateway")
}

fn assert_forward_settled(runtime: &Arc<ProductionSessionWorkers>, args: &serde_json::Value) {
    let source = args["sourceConversationId"].as_str().expect("source id");
    let owner = runtime.open_human_conversation_db_owner(source).expect("source owner");
    let batch = owner.get_metadata("humanForwardBatches").expect("journal").expect("persisted batch");
    assert_eq!(batch.as_array().expect("batches").len(), 1);
    for (index, destination) in args["destinationConversationIds"].as_array().unwrap().iter().enumerate() {
        assert_eq!(batch[0]["destinations"][index]["completed"], true);
        let entries = runtime.read_human_conversation_transcript(destination.as_str().unwrap())
            .expect("settled destination");
        assert_eq!(entries.len(), 1, "one canonical row per target");
        assert_eq!(entries[0]["delivery"], "sent");
        assert_eq!(entries[0]["content"], "immutable forwarded snapshot");
        assert_eq!(entries[0]["clientNonce"], batch[0]["destinations"][index]["clientNonce"]);
        assert_eq!(entries[0]["forwardContext"]["requestId"], args["clientNonce"]);
        assert!(entries[0]["remoteMessageId"].is_string());
    }
}

#[test]
fn shipping_human_forward_process_kill_preserves_complete_intent_before_first_post() {
    let root = temp_root("forward-process-kill");
    let mut server = ForwardHttpFixture::start(true);
    let args = prepare_forward_process(&root, &server.url);
    let mut child = ForwardProcess::start(&root, &server.url);
    server.wait_for_first_post();
    // Read from another connection while the child is alive in its first POST.
    let metadata = read_forward_metadata_from_disk(&root, args["sourceConversationId"].as_str().unwrap());
    let batch = &metadata["humanForwardBatches"][0];
    assert_eq!(batch["version"], 1);
    assert_eq!(batch["accountId"], "1");
    assert_eq!(batch["issuer"], server.url);
    assert_eq!(batch["requestId"], args["clientNonce"]);
    assert_eq!(batch["text"], "immutable forwarded snapshot");
    assert!(!batch.to_string().contains("fixture-token"), "bearer must not enter the journal");
    assert_eq!(batch["destinations"].as_array().unwrap().len(), 2);
    for index in 0..2 {
        assert_eq!(batch["destinations"][index]["conversationId"], args["destinationConversationIds"][index]);
        assert_eq!(batch["destinations"][index]["completed"], false);
    }
    let first_request = {
        let state = server.state.lock().unwrap();
        assert_eq!(state.accepted.len(), 1, "backend accepted the first request before termination");
        fixture_request_json(&state.requests[0])
    };
    assert_eq!(first_request["clientRequestId"], batch["destinations"][0]["clientNonce"]);
    assert_eq!(first_request["targetUserId"], "3");
    child.terminate();
    server.release_first_post();
    let resumed = human_message_fixture_runtime(&root, &server.url);
    assert!(resumed.read_human_conversation_transcript(args["destinationConversationIds"][1].as_str().unwrap())
        .expect("unsubmitted second target").is_empty());
    assert_eq!(read_forward_metadata_from_disk(&root, args["sourceConversationId"].as_str().unwrap()), metadata);
    assert_eq!(server.post_count(), 1);
    resumed.shutdown(); drop(resumed); drop(child);
    server.finish();
    fs::remove_dir_all(root).expect("remove process-kill fixture");
}

#[test]
fn shipping_human_forward_restart_sync_recovers_unsubmitted_target_and_original_snapshot() {
    let root = temp_root("forward-process-resume");
    let mut server = ForwardHttpFixture::start(true);
    let args = prepare_forward_process(&root, &server.url);
    let mut child = ForwardProcess::start(&root, &server.url);
    server.wait_for_first_post();
    child.terminate();
    server.release_first_post();
    let resumed = human_message_fixture_runtime(&root, &server.url);
    let source = args["sourceConversationId"].as_str().unwrap();
    let owner = resumed.open_human_conversation_db_owner(source).expect("reopened source");
    let mut edited = owner.get_transcript_entries().expect("source entry")[0].clone();
    edited["content"] = json!("edited after accepted whole-batch intent");
    owner.update_transcript_entry(args["sourceEntryId"].as_str().unwrap(), &edited)
        .expect("source edited after crash").expect("updated source");
    let second = args["destinationConversationIds"][1].as_str().unwrap();
    assert!(resumed.read_human_conversation_transcript(second).expect("never dispatched target").is_empty());
    let transcript = dispatch(&resumed, "syncHumanConversation", json!({"conversationId":second}));
    assert_eq!(transcript.as_array().expect("gateway transcript").len(), 1);
    assert_forward_settled(&resumed, &args);
    assert_eq!(server.post_count(), 3, "accepted first POST must replay once and second target dispatch once");
    assert_eq!(server.state.lock().unwrap().accepted.len(), 2, "server nonce dedupe creates only two messages");
    resumed.shutdown(); drop(owner); drop(resumed); drop(child);
    let reopened = human_message_fixture_runtime(&root, &server.url);
    assert_forward_settled(&reopened, &args);
    assert_eq!(forward_gateway_result(&reopened, &args).expect("completed retry")["destinations"][1]["status"], "sent");
    assert_eq!(server.post_count(), 3, "completed journal must not POST on another restart");
    reopened.shutdown(); drop(reopened);
    server.finish(); fs::remove_dir_all(root).expect("remove process-resume fixture");
}

#[test]
fn shipping_human_forward_idempotent_replay_rejects_changed_scope_and_expanded_destinations() {
    let root = temp_root("forward-idempotent-replay");
    let server = ForwardHttpFixture::start(false);
    let args = prepare_forward_process(&root, &server.url);
    let runtime = human_message_fixture_runtime(&root, &server.url);
    let first = forward_gateway_result(&runtime, &args).expect("first complete batch");
    assert_eq!(first["destinations"][0]["status"], "sent");
    assert_eq!(first["destinations"][1]["status"], "sent");
    let mut subset = args.clone();
    subset["destinationConversationIds"] = json!([args["destinationConversationIds"][1]]);
    let replay = forward_gateway_result(&runtime, &subset).expect("original subset retry");
    assert_eq!(replay["destinations"][0]["clientNonce"], first["destinations"][1]["clientNonce"]);
    let concurrent = (0..2).map(|_| {
        let runtime = Arc::clone(&runtime);
        let args = args.clone();
        thread::spawn(move || forward_gateway_result(&runtime, &args).expect("concurrent identical retry"))
    }).collect::<Vec<_>>();
    for replay in concurrent { assert_eq!(replay.join().expect("replay thread"), first); }
    for changed in ["sourceEntryId", "sourceConversationId", "privacy", "destinations"] {
        let mut invalid = args.clone();
        match changed {
            "sourceEntryId" => invalid["sourceEntryId"] = json!("another-entry"),
            "sourceConversationId" => invalid["sourceConversationId"] = args["destinationConversationIds"][0].clone(),
            "privacy" => invalid["dropSenderNames"] = json!(true),
            _ => invalid["destinationConversationIds"].as_array_mut().unwrap().push(json!("human-extra-target")),
        }
        assert!(forward_gateway_result(&runtime, &invalid).is_err(), "reject changed {changed}");
    }
    assert_forward_settled(&runtime, &args);
    assert_eq!(server.post_count(), 2, "repeated, concurrent and invalid calls must not dispatch");
    runtime.shutdown(); drop(runtime);
    server.finish(); fs::remove_dir_all(root).expect("remove idempotent fixture");
}

#[test]
fn shipping_human_forward_formatted_comment_shifts_link_entities_and_fences_conflicts() {
    let root = temp_root("forward-comment-link");
    let server = ForwardHttpFixture::start(false);
    let mut args = prepare_forward_process(&root, &server.url);
    let runtime = human_message_fixture_runtime(&root, &server.url);

    // 1. Positive case: forward with formatted comment dispatches comment message first, then forward message
    args["clientNonce"] = json!("fwd-nonce-with-comment-1");
    args["comment"] = json!({
        "text": "Check this out!",
        "entities": [
            {
                "offsetUtf16": 6,
                "lengthUtf16": 4,
                "kind": { "type": "bold" }
            }
        ]
    });

    let first = forward_gateway_result(&runtime, &args).expect("forward with formatted comment");
    assert_eq!(first["destinations"][0]["status"], "sent");
    assert_eq!(first["destinations"][1]["status"], "sent");

    // Two destinations receive 2 messages each (comment + forward) = 4 HTTP posts total
    assert_eq!(server.post_count(), 4, "2 destinations x (1 comment + 1 forward) must dispatch 4 HTTP posts");

    // Inspect destination 0 transcript: comment message arrived first, then forwarded message
    let dest_0 = args["destinationConversationIds"][0].as_str().unwrap();
    let dest_0_entries = runtime.read_human_conversation_transcript(dest_0).expect("dest 0 transcript");
    assert_eq!(dest_0_entries.len(), 2, "destination must receive both comment and forwarded message");
    assert_eq!(dest_0_entries[0]["content"], "Check this out!", "first message must be the user comment");
    assert_eq!(dest_0_entries[0]["delivery"], "sent");
    assert!(dest_0_entries[0]["clientNonce"].as_str().unwrap().ends_with(":comment"));
    assert_eq!(dest_0_entries[1]["content"], "immutable forwarded snapshot", "second message must be the forwarded source content");
    assert_eq!(dest_0_entries[1]["delivery"], "sent");

    // Inspect disk metadata for the persisted batch context
    let metadata = read_forward_metadata_from_disk(&root, args["sourceConversationId"].as_str().unwrap());
    let batches = metadata["humanForwardBatches"].as_array().expect("batches array");
    let batch = batches.iter().find(|b| b["requestId"] == args["clientNonce"]).expect("batch found");
    let stored_comment = &batch["context"]["comment"];
    assert_eq!(stored_comment["text"], "Check this out!");
    assert_eq!(stored_comment["entities"][0]["offsetUtf16"], 6);
    assert_eq!(stored_comment["entities"][0]["lengthUtf16"], 4);
    assert_eq!(stored_comment["entities"][0]["kind"]["type"], "bold");
    assert_eq!(batch["destinations"][0]["commentCompleted"], true);
    assert_eq!(batch["destinations"][0]["completed"], true);

    // 2. Idempotent replay with identical comment succeeds without sending duplicate HTTP posts
    let posts_before_replay = server.post_count();
    let replay = forward_gateway_result(&runtime, &args).expect("idempotent replay with same comment");
    assert_eq!(replay, first);
    assert_eq!(server.post_count(), posts_before_replay, "replay must not send duplicate HTTP posts");
    let dest_0_replayed = runtime.read_human_conversation_transcript(dest_0).expect("dest 0 transcript after replay");
    assert_eq!(dest_0_replayed.len(), 2, "replay must not duplicate transcript messages");

    // 3. Nonce conflict negative case: same nonce with different comment is rejected
    let mut conflicting_args = args.clone();
    conflicting_args["comment"] = json!({
        "text": "Different comment text",
        "entities": []
    });
    let err = forward_gateway_result(&runtime, &conflicting_args).expect_err("same nonce with different comment rejected");
    assert!(
        format!("{err:?}").contains("already identifies a different comment"),
        "error was: {err:?}"
    );

    // Also verify same nonce with NO comment is rejected
    let mut no_comment_args = args.clone();
    no_comment_args.as_object_mut().unwrap().remove("comment");
    let err_no_comment = forward_gateway_result(&runtime, &no_comment_args).expect_err("same nonce without comment rejected");
    assert!(
        format!("{err_no_comment:?}").contains("already identifies a different comment"),
        "error was: {err_no_comment:?}"
    );

    // 4. Entity validation negative cases: out-of-bounds, overflow, empty text with entities are rejected before storage or POST
    let pre_posts = server.post_count();
    let mut invalid_span_args = args.clone();
    invalid_span_args["clientNonce"] = json!("fwd-nonce-invalid-span");
    invalid_span_args["comment"] = json!({
        "text": "hi",
        "entities": [
            {
                "offsetUtf16": 1,
                "lengthUtf16": 5,
                "kind": { "type": "bold" }
            }
        ]
    });
    let err_span = forward_gateway_result(&runtime, &invalid_span_args).expect_err("out of bounds entity span must fail");
    assert!(format!("{err_span:?}").contains("exceeds text UTF-16 code units length"), "error was: {err_span:?}");

    let mut overflow_args = args.clone();
    overflow_args["clientNonce"] = json!("fwd-nonce-overflow-atomic");
    overflow_args["comment"] = json!({
        "text": "overflow comment",
        "entities": [
            {
                "offsetUtf16": u32::MAX - 2,
                "lengthUtf16": 5,
                "kind": { "type": "bold" }
            }
        ]
    });
    let err_overflow = forward_gateway_result(&runtime, &overflow_args).expect_err("overflow entity offset must fail");
    assert!(format!("{err_overflow:?}").contains("overflow"), "error was: {err_overflow:?}");

    let mut empty_entities_args = args.clone();
    empty_entities_args["clientNonce"] = json!("fwd-nonce-empty-entities");
    empty_entities_args["comment"] = json!({
        "text": "",
        "entities": [
            {
                "offsetUtf16": 0,
                "lengthUtf16": 0,
                "kind": { "type": "bold" }
            }
        ]
    });
    let err_empty = forward_gateway_result(&runtime, &empty_entities_args).expect_err("empty text with entities must fail");
    assert!(format!("{err_empty:?}").contains("empty text cannot have formatting entities"), "error was: {err_empty:?}");
    assert_eq!(server.post_count(), pre_posts, "validation failures must not dispatch HTTP post");

    // 5. Legacy batch compatibility: forward without comment replays without comment, rejects comment injection
    let mut legacy_args = args.clone();
    legacy_args["clientNonce"] = json!("fwd-nonce-legacy-batch");
    legacy_args.as_object_mut().unwrap().remove("comment");
    let legacy_first = forward_gateway_result(&runtime, &legacy_args).expect("legacy forward without comment");
    assert_eq!(legacy_first["destinations"][0]["status"], "sent");

    // Idempotent replay of legacy batch without comment succeeds
    let legacy_replay = forward_gateway_result(&runtime, &legacy_args).expect("replay legacy forward without comment");
    assert_eq!(legacy_replay, legacy_first);

    // Replay of legacy batch attempting to inject a comment is rejected
    let mut inject_comment_args = legacy_args.clone();
    inject_comment_args["comment"] = json!({ "text": "injected comment", "entities": [] });
    let err_injected = forward_gateway_result(&runtime, &inject_comment_args).expect_err("legacy batch rejects injected comment");
    assert!(
        format!("{err_injected:?}").contains("already identifies a different comment"),
        "error was: {err_injected:?}"
    );

    // 6. Direct Session compose_share_comment verification (pure function exposed on ProductionSessionWorkers)
    let comment_for_session = fabushi_messaging_core::FormattedText {
        text: "direct comment".into(),
        entities: vec![fabushi_messaging_core::TextEntity {
            offset_utf16: 7,
            length_utf16: 7,
            kind: fabushi_messaging_core::TextEntityKind::Italic,
        }],
    };
    let link = "https://canonical.example/link";
    let shifted = runtime.compose_share_comment(link, comment_for_session).unwrap();
    let expected_prefix = format!("{link}\n");
    let shift = u32::try_from(expected_prefix.encode_utf16().count()).unwrap();
    assert_eq!(shifted.text, format!("{expected_prefix}direct comment"));
    assert_eq!(shifted.entities[0].offset_utf16, 7 + shift);

    // 6b. Gateway composeShareComment positive & overflow contract assertions
    let gateway_compose_val = dispatch_production_session_gateway_call(
        &runtime,
        "composeShareComment",
        &json!({
            "link": "https://docs.fabushi.dev/spec?a=1&b=2#sec",
            "comment": {
                "text": "check spec",
                "entities": [
                    {
                        "offsetUtf16": 6,
                        "lengthUtf16": 4,
                        "kind": { "type": "bold" }
                    }
                ]
            }
        }),
    )
    .expect("composeShareComment registered")
    .expect("composeShareComment successful");
    assert_eq!(
        gateway_compose_val["text"],
        "https://docs.fabushi.dev/spec?a=1&b=2#sec\ncheck spec"
    );
    assert_eq!(gateway_compose_val["entities"][0]["offsetUtf16"], 48);
    assert_eq!(gateway_compose_val["entities"][0]["lengthUtf16"], 4);

    let compose_overflow_err = dispatch_production_session_gateway_call(
        &runtime,
        "composeShareComment",
        &json!({
            "link": "https://docs.fabushi.dev/spec?a=1&b=2#sec",
            "comment": {
                "text": "overflow text",
                "entities": [
                    {
                        "offsetUtf16": u32::MAX - 2,
                        "lengthUtf16": 1,
                        "kind": { "type": "bold" }
                    }
                ]
            }
        }),
    )
    .expect("composeShareComment registered")
    .expect_err("composeShareComment with offset overflow must fail atomically");
    assert!(
        format!("{compose_overflow_err:?}").contains("TextEntityOffsetOverflow")
            || format!("{compose_overflow_err:?}").contains("overflow"),
        "error was: {compose_overflow_err:?}"
    );

    // composeShareComment link newline validation negative case
    let compose_nl_err = dispatch_production_session_gateway_call(
        &runtime,
        "composeShareComment",
        &json!({
            "link": "https://docs.fabushi.dev/spec\nbreak",
            "comment": "valid comment"
        }),
    )
    .expect("composeShareComment registered")
    .expect_err("composeShareComment with newline in link must fail");
    assert!(format!("{compose_nl_err:?}").contains("link must not contain newlines or control characters"));

    // 6c. Gateway forwardHumanMessage with shareUrl (including non-BMP emoji) & overflow negative case
    let mut share_url_args = args.clone();
    share_url_args["clientNonce"] = json!("fwd-nonce-with-share-url-emoji-1");
    share_url_args["shareUrl"] = json!("🔗 https://docs.fabushi.dev/share");
    share_url_args["comment"] = json!({
        "text": "Check emoji shift!",
        "entities": [
            {
                "offsetUtf16": 6,
                "lengthUtf16": 5,
                "kind": { "type": "italic" }
            }
        ]
    });
    let share_url_first = forward_gateway_result(&runtime, &share_url_args).expect("forward with shareUrl");
    assert_eq!(share_url_first["destinations"][0]["status"], "sent");

    // Inspect disk metadata for the shifted entities
    let share_metadata = read_forward_metadata_from_disk(&root, args["sourceConversationId"].as_str().unwrap());
    let share_batches = share_metadata["humanForwardBatches"].as_array().expect("batches array");
    let share_batch = share_batches.iter().find(|b| b["requestId"] == share_url_args["clientNonce"]).expect("share batch found");
    let share_stored_comment = &share_batch["context"]["comment"];
    assert_eq!(share_stored_comment["text"], "🔗 https://docs.fabushi.dev/share\nCheck emoji shift!");
    // '🔗' is 2 UTF-16 code units; prefix length is 2 (emoji) + 1 (space) + 30 (url) + 1 (\n) = 34 UTF-16 code units.
    // 6 + 34 = 40.
    assert_eq!(share_stored_comment["entities"][0]["offsetUtf16"], 40);
    assert_eq!(share_stored_comment["entities"][0]["lengthUtf16"], 5);

    // Negative case: forwardHumanMessage with shareUrl and newline/control chars
    let mut bad_url_args = args.clone();
    bad_url_args["clientNonce"] = json!("fwd-nonce-bad-share-url");
    bad_url_args["shareUrl"] = json!("https://docs.fabushi.dev/share\nattack");
    let err_bad_url = forward_gateway_result(&runtime, &bad_url_args).expect_err("shareUrl with newline must fail");
    assert!(format!("{err_bad_url:?}").contains("shareUrl must not contain newlines or control characters"));

    // Negative case: forwardHumanMessage with shareUrl and entity offset overflow
    let mut share_overflow_args = args.clone();
    share_overflow_args["clientNonce"] = json!("fwd-nonce-share-url-overflow");
    share_overflow_args["shareUrl"] = json!("https://docs.fabushi.dev/share");
    share_overflow_args["comment"] = json!({
        "text": "overflow",
        "entities": [
            {
                "offsetUtf16": u32::MAX - 2,
                "lengthUtf16": 1,
                "kind": { "type": "bold" }
            }
        ]
    });
    let err_share_overflow = forward_gateway_result(&runtime, &share_overflow_args).expect_err("shareUrl entity offset overflow must fail atomically");
    assert!(
        format!("{err_share_overflow:?}").contains("TextEntityOffsetOverflow")
            || format!("{err_share_overflow:?}").contains("overflow"),
        "error was: {err_share_overflow:?}"
    );

    // 7. Crash recovery: comment sent and recorded on disk -> replay does not duplicate comment POSTs
    let mut resume_args = args.clone();
    resume_args["clientNonce"] = json!("fwd-nonce-comment-crash-resume");
    resume_args["comment"] = json!({ "text": "crash recovery comment", "entities": [] });
    let pre_posts = server.post_count();
    let res = forward_gateway_result(&runtime, &resume_args).expect("first dispatch of crash-resume forward");
    assert_eq!(res["destinations"][0]["status"], "sent");
    let dest_entries = runtime.read_human_conversation_transcript(dest_0).expect("dest entries");
    let initial_count = dest_entries.len();
    assert_eq!(server.post_count(), pre_posts + 4);

    let res2 = forward_gateway_result(&runtime, &resume_args).expect("replay after recovery");
    assert_eq!(res2, res);
    assert_eq!(server.post_count(), pre_posts + 4, "recovery replay must not send additional comment or forward posts");
    let dest_entries_after = runtime.read_human_conversation_transcript(dest_0).expect("dest entries after");
    assert_eq!(dest_entries_after.len(), initial_count);

    runtime.shutdown(); drop(runtime);
    server.finish(); fs::remove_dir_all(root).expect("remove comment-link fixture");
}

#[test]
fn shipping_human_forward_restart_rechecks_source_permissions_and_credential_scope() {
    let root = temp_root("forward-permission-recovery");
    let mut server = ForwardHttpFixture::start(true);
    let args = prepare_forward_process(&root, &server.url);
    let mut child = ForwardProcess::start(&root, &server.url);
    server.wait_for_first_post(); child.terminate(); server.release_first_post();
    let runtime = human_message_fixture_runtime(&root, &server.url);
    let source = args["sourceConversationId"].as_str().unwrap();
    let first = args["destinationConversationIds"][0].as_str().unwrap();
    let second = args["destinationConversationIds"][1].as_str().unwrap();
    let owner = runtime.open_human_conversation_db_owner(source).expect("source owner");
    let original = owner.get_transcript_entries().unwrap()[0].clone();
    let journal = owner.get_metadata("humanForwardBatches").unwrap();
    let mut protected = original.clone(); protected["protectedContent"] = json!(true);
    owner.update_transcript_entry(args["sourceEntryId"].as_str().unwrap(), &protected).unwrap().unwrap();
    runtime.sync_human_conversation(second).expect("protected source cannot starve unrelated history");
    assert_eq!(server.history_count(), 1, "protected source still permits real history HTTP");
    let protected_result = forward_gateway_result(&runtime, &args).expect("durable policy-denial envelope");
    for destination in protected_result["destinations"].as_array().unwrap() {
        assert_eq!(destination["status"], "failed");
        assert!(destination["error"].as_str().unwrap().contains("protected"));
    }
    owner.delete_transcript_entry(args["sourceEntryId"].as_str().unwrap()).unwrap();
    runtime.sync_human_conversation(second).expect("deleted source cannot starve unrelated history");
    assert_eq!(server.history_count(), 2, "missing source still permits real history HTTP");
    let missing_result = forward_gateway_result(&runtime, &args).expect("deleted source failure envelope");
    assert!(missing_result["destinations"][0]["error"].as_str().unwrap().contains("missing"));
    assert!(owner.append_transcript_entry(&original).unwrap());
    assert!(owner.set_metadata("participantIds", json!(["2", "9"])).unwrap());
    // The source is excluded from the authorized conversation list, and the
    // first target has a real dispatching outbox row from the killed POST.
    // Its generic pending-send recovery must not bypass the source journal.
    let pending_before = runtime.read_human_conversation_transcript(first).unwrap();
    assert_eq!(pending_before.len(), 1);
    assert_eq!(pending_before[0]["delivery"], "dispatching");
    assert!(pending_before[0].get("remoteMessageId").is_none());
    runtime.sync_human_conversation(first).expect("authorized target can sync without bypassing source");
    assert_eq!(server.history_count(), 3, "removed membership cannot starve target history");
    assert_eq!(server.post_count(), 1, "generic outbox must not resend the revoked-source forward");
    assert_eq!(runtime.read_human_conversation_transcript(first).unwrap(), pending_before);
    let removed_result = forward_gateway_result(&runtime, &args).expect("removed membership failure envelope");
    assert_eq!(removed_result["destinations"][0]["status"], "failed");
    assert!(removed_result["destinations"][0]["error"].as_str().unwrap().contains("participant"));
    assert!(owner.set_metadata("participantIds", json!(["1", "2"])).unwrap());
    let credential_path = root.join("host-credential.json");
    let credential = fs::read_to_string(&credential_path).unwrap();
    for (key, value) in [("userId", json!(9)), ("issuer", json!("http://127.0.0.1:1"))] {
        let mut changed: serde_json::Value = serde_json::from_str(&credential).unwrap();
        changed[key] = value;
        fs::write(&credential_path, changed.to_string()).unwrap();
        assert!(runtime.sync_human_conversation(second).is_err(), "changed {key} cannot resume old intent");
    }
    fs::remove_file(&credential_path).unwrap();
    assert!(runtime.sync_human_conversation(second).is_err(), "missing private credentials cannot resume");
    assert_eq!(server.post_count(), 1, "all permission and credential negatives prevent further sends");
    let blocked = owner.get_metadata("humanForwardBatches").unwrap().unwrap();
    let original_journal = journal.unwrap();
    for key in ["version", "accountId", "issuer", "requestId", "sourceConversationId", "sourceEntryId", "text", "attachments", "context"] {
        assert_eq!(blocked[0][key], original_journal[0][key], "retain immutable blocked {key}");
    }
    for index in 0..2 {
        assert_eq!(blocked[0]["destinations"][index]["conversationId"], original_journal[0]["destinations"][index]["conversationId"]);
        assert_eq!(blocked[0]["destinations"][index]["clientNonce"], original_journal[0]["destinations"][index]["clientNonce"]);
        assert_eq!(blocked[0]["destinations"][index]["completed"], false);
        assert!(blocked[0]["destinations"][index]["error"].is_string(), "policy denial is durable");
    }
    assert!(runtime.read_human_conversation_transcript(second).unwrap().is_empty());
    runtime.shutdown(); drop(owner); drop(runtime); drop(child);
    let recovered = human_message_fixture_runtime(&root, &server.url);
    dispatch(&recovered, "syncHumanConversation", json!({"conversationId":second}));
    assert_forward_settled(&recovered, &args);
    assert_eq!(server.post_count(), 3);
    recovered.shutdown(); drop(recovered);
    server.finish(); fs::remove_dir_all(root).expect("remove permission fixture");
}

#[test]
fn shipping_human_forward_sqlite_failures_stop_initial_and_progress_dispatch() {
    for fail_progress in [false, true] {
        let root = temp_root(if fail_progress { "forward-sqlite-progress" } else { "forward-sqlite-staging" });
        let mut server = ForwardHttpFixture::start(fail_progress);
        let args = prepare_forward_process(&root, &server.url);
        let runtime = human_message_fixture_runtime(&root, &server.url);
        let source = args["sourceConversationId"].as_str().unwrap();
        let db = rusqlite::Connection::open(forward_db_path(&root, source)).expect("SQLite fault injector");
        db.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
        let running = if fail_progress {
            let runtime = Arc::clone(&runtime); let args = args.clone();
            let running = thread::spawn(move || forward_gateway_result(&runtime, &args));
            server.wait_for_first_post();
            Some(running)
        } else { None };
        let before = read_forward_metadata_from_disk(&root, source);
        db.execute_batch("CREATE TRIGGER reject_forward_metadata BEFORE UPDATE OF value ON kv
            WHEN NEW.key = 'metadata' BEGIN SELECT RAISE(ABORT, 'NMR-47 injected SQLite persistence failure'); END;")
            .expect("install real SQLite write rejection");
        if let Some(running) = running {
            server.release_first_post();
            assert!(running.join().expect("forward after ack").is_err(), "progress commit must fail");
        } else {
            for _ in 0..2 {
                assert!(forward_gateway_result(&runtime, &args).is_err(), "failed staging must not leak in-memory success");
            }
        }
        assert_eq!(server.post_count(), usize::from(fail_progress));
        assert_eq!(read_forward_metadata_from_disk(&root, source), before, "failed transaction preserves durable record");
        assert!(runtime.read_human_conversation_transcript(args["destinationConversationIds"][1].as_str().unwrap())
            .unwrap().is_empty(), "never dispatch second target after failed durability");
        if fail_progress {
            let first = runtime.read_human_conversation_transcript(args["destinationConversationIds"][0].as_str().unwrap()).unwrap();
            assert_eq!(first.len(), 1);
            assert_eq!(first[0]["delivery"], "sent", "ack settled before batch-progress failure");
            assert_eq!(before["humanForwardBatches"][0]["destinations"][0]["completed"], false);
        } else {
            assert!(before.get("humanForwardBatches").is_none());
        }
        db.execute_batch("DROP TRIGGER reject_forward_metadata;").expect("remove scoped injected fault");
        drop(db); runtime.shutdown(); drop(runtime);
        let resumed = human_message_fixture_runtime(&root, &server.url);
        if fail_progress {
            dispatch(&resumed, "syncHumanConversation", json!({"conversationId":args["destinationConversationIds"][1]}));
        } else {
            forward_gateway_result(&resumed, &args).expect("retry after persistence recovery");
        }
        assert_forward_settled(&resumed, &args);
        assert_eq!(server.post_count(), 2, "settled first target cannot be sent again after progress recovery");
        assert_eq!(server.state.lock().unwrap().accepted.len(), 2);
        resumed.shutdown(); drop(resumed);
        server.finish(); fs::remove_dir_all(root).expect("remove SQLite fault fixture");
    }
}

// NMR-48 byte oracles start from independently specified binary input and the
// actual multipart upload, not a serializer/helper from the snapshot owner.
type ForwardFileFixture = (String, std::path::PathBuf, Vec<u8>);

fn forward_attachment_files(root: &std::path::Path, count: usize) -> Vec<ForwardFileFixture> {
    fs::create_dir_all(root).expect("attachment fixture root");
    (0..count).map(|index| {
        let name = format!("report-{index}.bin");
        let path = root.join(format!("original-cache-{index}.dat"));
        let mut bytes = b"\x00original forward payload\xff\r\n".to_vec();
        bytes.extend_from_slice(&[index as u8, 0x80, 0]);
        fs::write(&path, &bytes).expect("original binary attachment");
        (name, path, bytes)
    }).collect()
}

fn prepare_forward_attachment_process(
    root: &std::path::Path,
    url: &str,
    files: &[ForwardFileFixture],
) -> serde_json::Value {
    let args = prepare_forward_process(root, url);
    let runtime = human_message_fixture_runtime(root, url);
    let owner = runtime.open_human_conversation_db_owner(
        args["sourceConversationId"].as_str().unwrap()
    ).expect("attachment source owner");
    let mut source = owner.get_transcript_entries().expect("source rows")[0].clone();
    source["attachments"] = json!(files.iter().enumerate().map(|(index, (name, path, bytes))|
        json!({"name":name,"path":path,"size":bytes.len(),"resourceId":format!("original-resource-{index}")})
    ).collect::<Vec<_>>());
    owner.update_transcript_entry(args["sourceEntryId"].as_str().unwrap(), &source)
        .expect("store source attachments").expect("source exists");
    runtime.shutdown(); drop(owner); drop(runtime);
    args
}

fn assert_forward_staged_bytes(
    root: &std::path::Path,
    args: &serde_json::Value,
    files: &[ForwardFileFixture],
    server: &ForwardHttpFixture,
) -> Vec<std::path::PathBuf> {
    use sha2::Digest;
    let source = args["sourceConversationId"].as_str().unwrap();
    let metadata = read_forward_metadata_from_disk(root, source);
    let batch = &metadata["humanForwardBatches"][0];
    assert_eq!(batch["destinations"].as_array().unwrap().len(), 2);
    let attachments = batch["attachments"].as_array().expect("durable pinned attachments");
    assert_eq!(attachments.len(), files.len());
    let directory = fs::canonicalize(forward_db_path(root, source).parent().unwrap())
        .expect("canonical source Session directory").join("forward-attachments");
    let paths = attachments.iter().zip(files.iter()).map(|(attachment, (name, original, bytes))| {
        assert_eq!(attachment["name"], json!(name));
        let path = std::path::PathBuf::from(attachment["path"].as_str().expect("snapshot path"));
        assert_ne!(&path, original, "accepted bytes cannot reference the mutable cache");
        assert!(path.starts_with(&directory), "snapshot remains inside this source Session owner");
        assert_eq!(fs::read(&path).expect("durable snapshot bytes"), *bytes);
        assert_eq!(attachment["forwardSnapshot"]["version"], 1);
        assert_eq!(attachment["forwardSnapshot"]["size"], json!(bytes.len()));
        assert_eq!(attachment["forwardSnapshot"]["sha256"],
            json!(format!("{:x}", sha2::Sha256::digest(bytes))));
        path
    }).collect::<Vec<_>>();
    let state = server.state.lock().expect("independent upload observation");
    let (observed, observed_bytes) = state.first_upload_snapshot.as_ref()
        .expect("observe committed intent during the first resource POST");
    assert_eq!(observed["humanForwardBatches"][0]["attachments"], batch["attachments"]);
    assert_eq!(*observed_bytes, files.iter().map(|(_, _, bytes)| bytes.clone()).collect::<Vec<_>>(),
        "all snapshots already exist before the first uploaded resource is acknowledged");
    assert!(observed["humanForwardBatches"][0]["destinations"].as_array().unwrap()
        .iter().all(|destination| destination["completed"] == false));
    paths
}

fn assert_forward_uploaded_bytes(server: &ForwardHttpFixture, files: &[ForwardFileFixture]) {
    let state = server.state.lock().expect("captured multipart data");
    assert_eq!(state.uploaded_files.len(), files.len() * 2, "one upload set per destination");
    for (index, (name, bytes)) in state.uploaded_files.iter().enumerate() {
        let expected = &files[index % files.len()];
        assert_eq!(name, &expected.0, "preserve submitted file name and order");
        assert_eq!(bytes, &expected.2, "actual multipart bytes match the original submitted input");
    }
    assert_eq!(state.accepted.len(), 2, "nonce dedupe creates one message per destination");
    let mut peers = std::collections::BTreeSet::new();
    for message in state.accepted.values() {
        peers.insert(message["recipientUserId"].as_u64().unwrap());
        let attachments = message["attachments"].as_array().unwrap();
        assert_eq!(attachments.len(), files.len());
        for (attachment, (name, _, expected)) in attachments.iter().zip(files.iter()) {
            assert_eq!(attachment["name"], json!(name));
            let (_, bytes) = state.resources.get(attachment["resourceId"].as_str().unwrap()).unwrap();
            assert_eq!(bytes, expected, "each accepted destination binds the actual uploaded bytes");
        }
    }
    assert_eq!(peers, [3_u64, 4].into_iter().collect::<std::collections::BTreeSet<_>>());
}

fn assert_forward_intent_unchanged(before: &serde_json::Value, after: &serde_json::Value) {
    for key in ["version", "accountId", "issuer", "requestId", "sourceConversationId", "sourceEntryId", "text", "attachments", "context"] {
        assert_eq!(after[key], before[key], "retain immutable forward {key}");
    }
    for index in 0..2 {
        for key in ["conversationId", "clientNonce"] {
            assert_eq!(after["destinations"][index][key], before["destinations"][index][key]);
        }
    }
}

#[test]
fn shipping_human_forward_attachment_bytes_survive_process_kill_and_source_cache_replacement() {
    for delete_original in [false, true] {
        let root = temp_root(if delete_original { "forward-byte-deleted-cache" } else { "forward-byte-replaced-cache" });
        let files = forward_attachment_files(&root, 2);
        let mut server = ForwardHttpFixture::start(true);
        let args = prepare_forward_attachment_process(&root, &server.url, &files);
        let source = args["sourceConversationId"].as_str().unwrap();
        server.observe_snapshot_staging(&root, source);
        let mut child = ForwardProcess::start(&root, &server.url);
        server.wait_for_first_post();
        let snapshots = assert_forward_staged_bytes(&root, &args, &files, &server);
        assert_eq!(server.resource_post_count(), 2);
        assert_eq!(server.post_count(), 1);
        child.terminate(); server.release_first_post();
        for (_, path, bytes) in &files {
            if delete_original { fs::remove_file(path).expect("delete mutable original cache"); }
            else { fs::write(path, vec![b'X'; bytes.len()]).expect("replace original with same-size bytes"); }
        }
        let resumed = human_message_fixture_runtime(&root, &server.url);
        dispatch(&resumed, "syncHumanConversation", json!({"conversationId":args["destinationConversationIds"][1]}));
        assert_forward_settled(&resumed, &args);
        assert_forward_uploaded_bytes(&server, &files);
        assert_eq!(server.post_count(), 3, "replay accepted first nonce once, send second once");
        for (path, (_, original, bytes)) in snapshots.iter().zip(files.iter()) {
            assert_eq!(fs::read(path).unwrap(), *bytes, "the retained snapshot is not rewritten on recovery");
            if delete_original { assert!(!original.exists(), "recovery must not recreate a deleted user cache"); }
            else { assert_eq!(fs::read(original).unwrap(), vec![b'X'; bytes.len()]); }
        }
        resumed.shutdown(); drop(resumed); drop(child);
        let reopened = human_message_fixture_runtime(&root, &server.url);
        dispatch(&reopened, "syncHumanConversation", json!({"conversationId":args["destinationConversationIds"][1]}));
        assert_forward_settled(&reopened, &args);
        assert_eq!(server.post_count(), 3);
        assert_eq!(server.resource_post_count(), 4, "another restart cannot re-upload completed resources");
        reopened.shutdown(); drop(reopened);
        server.finish(); fs::remove_dir_all(root).expect("remove immutable attachment fixture");
    }
}

#[test]
fn shipping_human_forward_missing_or_corrupt_snapshot_retains_intent_until_exact_byte_repair() {
    for fault in ["missing", "same-size-corrupt", "oversized", "directory"] {
        let root = temp_root(&format!("forward-snapshot-{fault}"));
        let files = forward_attachment_files(&root, 2);
        let mut server = ForwardHttpFixture::start(true);
        let args = prepare_forward_attachment_process(&root, &server.url, &files);
        let source = args["sourceConversationId"].as_str().unwrap();
        let second = args["destinationConversationIds"][1].as_str().unwrap();
        server.observe_snapshot_staging(&root, source);
        let mut child = ForwardProcess::start(&root, &server.url);
        server.wait_for_first_post();
        let snapshots = assert_forward_staged_bytes(&root, &args, &files, &server);
        child.terminate(); server.release_first_post();
        let runtime = human_message_fixture_runtime(&root, &server.url);
        let mut first_only = args.clone();
        first_only["destinationConversationIds"] = json!([args["destinationConversationIds"][0]]);
        assert_eq!(forward_gateway_result(&runtime, &first_only).expect("settle accepted resource identity")["destinations"][0]["status"], "sent");
        assert_eq!(server.post_count(), 2);
        assert_eq!(server.resource_post_count(), 2, "ambiguous POST reuses its persisted resource IDs");
        let before = read_forward_metadata_from_disk(&root, source)["humanForwardBatches"][0].clone();
        // The first file stays valid: a per-file read/upload loop would leak a
        // resource POST before discovering this later invalid attachment.
        let snapshot = &snapshots[1];
        match fault {
            "missing" => fs::remove_file(snapshot).unwrap(),
            "same-size-corrupt" => fs::write(snapshot, vec![b'X'; files[1].2.len()]).unwrap(),
            "oversized" => fs::File::create(snapshot).unwrap().set_len(32 * 1024 * 1024 + 1).unwrap(),
            _ => { fs::remove_file(snapshot).unwrap(); fs::create_dir(snapshot).unwrap(); }
        }
        let failed = forward_gateway_result(&runtime, &args).expect("snapshot failure is a durable per-target result");
        assert_eq!(failed["destinations"][0]["status"], "sent");
        assert_eq!(failed["destinations"][1]["status"], "failed");
        let expected_error = match fault {
            "missing" => "unavailable",
            "same-size-corrupt" => "snapshot digest",
            "oversized" => "size",
            _ => "regular non-symlink",
        };
        assert!(failed["destinations"][1]["error"].as_str()
            .is_some_and(|error| error.contains(expected_error)), "report the actual {fault} cause");
        let pending = runtime.read_human_conversation_transcript(second).expect("retryable pinned outbox");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0]["delivery"], "pending");
        assert!(pending[0].get("remoteMessageId").is_none());
        assert!(pending[0].get("remoteDispatchAttachments").is_none());
        let history_before = server.history_count();
        dispatch(&runtime, "syncHumanConversation", json!({"conversationId":second}));
        assert!(server.history_count() > history_before, "a damaged pin cannot starve unrelated inbound history");
        assert_eq!(server.post_count(), 2, "no message POST while pin is invalid");
        assert_eq!(server.resource_post_count(), 2, "no resource POST even for the valid first pin when a later pin is invalid");
        let blocked = read_forward_metadata_from_disk(&root, source)["humanForwardBatches"][0].clone();
        assert_forward_intent_unchanged(&before, &blocked);
        assert_eq!(blocked["destinations"][0]["completed"], true);
        assert_eq!(blocked["destinations"][1]["completed"], false);
        assert!(blocked["destinations"][1]["error"].is_string());
        runtime.shutdown(); drop(runtime); drop(child);
        let resumed = human_message_fixture_runtime(&root, &server.url);
        let reopened_pending = resumed.read_human_conversation_transcript(second).unwrap();
        for key in ["id", "content", "clientNonce", "attachments", "forwardContext", "delivery"] {
            assert_eq!(reopened_pending[0][key], pending[0][key], "persist {fault} outbox {key}");
        }
        if fault == "directory" { fs::remove_dir(snapshot).expect("remove invalid snapshot directory"); }
        fs::write(snapshot, &files[1].2).expect("restore the exact independently retained input bytes");
        dispatch(&resumed, "syncHumanConversation", json!({"conversationId":second}));
        assert_forward_settled(&resumed, &args);
        assert_forward_uploaded_bytes(&server, &files);
        assert_eq!(server.post_count(), 3);
        resumed.shutdown(); drop(resumed);
        server.finish(); fs::remove_dir_all(root).expect("remove damaged snapshot fixture");
    }
}

#[test]
fn shipping_human_forward_invalid_attachment_inputs_and_snapshot_staging_fail_before_post() {
    for fault in ["too-many", "empty", "oversized", "directory", "staging-path"] {
        let root = temp_root(&format!("forward-attachment-admission-{fault}"));
        let files = forward_attachment_files(&root, if fault == "too-many" { 7 } else { 1 });
        let server = ForwardHttpFixture::start(false);
        let args = prepare_forward_attachment_process(&root, &server.url, &files);
        let source = args["sourceConversationId"].as_str().unwrap();
        let staging = forward_db_path(&root, source).parent().unwrap().join("forward-attachments");
        match fault {
            "empty" => fs::write(&files[0].1, []).unwrap(),
            "oversized" => fs::File::create(&files[0].1).unwrap().set_len(32 * 1024 * 1024 + 1).unwrap(),
            "directory" => { fs::remove_file(&files[0].1).unwrap(); fs::create_dir(&files[0].1).unwrap(); }
            "staging-path" => fs::write(&staging, b"retained unrelated file").unwrap(),
            _ => {}
        }
        let runtime = human_message_fixture_runtime(&root, &server.url);
        for _ in 0..2 {
            assert!(forward_gateway_result(&runtime, &args).is_err(), "reject new batch for {fault}");
            assert!(read_forward_metadata_from_disk(&root, source).get("humanForwardBatches").is_none(),
                "failed staging cannot accept a durable batch for {fault}");
        }
        for destination in args["destinationConversationIds"].as_array().unwrap() {
            assert!(runtime.read_human_conversation_transcript(destination.as_str().unwrap()).unwrap().is_empty());
        }
        assert_eq!(server.post_count(), 0, "no message POST for {fault}");
        assert_eq!(server.resource_post_count(), 0, "no resource POST for {fault}");
        assert!(server.state.lock().unwrap().accepted.is_empty());
        if fault == "staging-path" {
            assert_eq!(fs::read(&staging).unwrap(), b"retained unrelated file");
        }
        runtime.shutdown(); drop(runtime);
        server.finish(); fs::remove_dir_all(root).expect("remove attachment admission fixture");
    }
}

#[test]
fn shipping_human_forward_legacy_unpinned_attachments_fail_without_blocking_history_or_rebinding_bytes() {
    let root = temp_root("forward-legacy-unpinned");
    let files = forward_attachment_files(&root, 1);
    let mut server = ForwardHttpFixture::start(true);
    let args = prepare_forward_attachment_process(&root, &server.url, &files);
    let source = args["sourceConversationId"].as_str().unwrap();
    let first = args["destinationConversationIds"][0].as_str().unwrap();
    let second = args["destinationConversationIds"][1].as_str().unwrap();
    server.observe_snapshot_staging(&root, source);
    let mut child = ForwardProcess::start(&root, &server.url);
    server.wait_for_first_post();
    let snapshots = assert_forward_staged_bytes(&root, &args, &files, &server);
    child.terminate(); server.release_first_post();
    let runtime = human_message_fixture_runtime(&root, &server.url);
    let mut first_only = args.clone(); first_only["destinationConversationIds"] = json!([first]);
    assert_eq!(forward_gateway_result(&runtime, &first_only).unwrap()["destinations"][0]["status"], "sent");
    let completed_first = runtime.read_human_conversation_transcript(first).unwrap();
    let owner = runtime.open_human_conversation_db_owner(source).unwrap();
    let mut legacy = owner.get_metadata("humanForwardBatches").unwrap().unwrap();
    legacy[0]["attachments"][0].as_object_mut().unwrap().remove("forwardSnapshot");
    legacy[0]["attachments"][0]["path"] = json!(files[0].1);
    assert!(owner.set_metadata("humanForwardBatches", legacy.clone()).unwrap());
    fs::write(&files[0].1, vec![b'X'; files[0].2.len()]).expect("old mutable path now holds different bytes");
    runtime.shutdown(); drop(owner); drop(runtime); drop(child);
    let resumed = human_message_fixture_runtime(&root, &server.url);
    let before_history = server.history_count();
    dispatch(&resumed, "syncHumanConversation", json!({"conversationId":second}));
    assert!(server.history_count() > before_history);
    let failure = forward_gateway_result(&resumed, &args).expect("legacy retry remains a visible failure envelope");
    assert_eq!(failure["destinations"][0]["status"], "sent");
    assert_eq!(failure["destinations"][1]["status"], "failed");
    assert!(failure["destinations"][1]["error"].as_str().is_some_and(|error|
        error.contains("snapshot") || error.contains("unpinned") || error.contains("legacy")));
    assert_eq!(server.post_count(), 2, "never upload/send mutable legacy input to the unfinished target");
    assert_eq!(server.resource_post_count(), 1);
    assert_eq!(resumed.read_human_conversation_transcript(first).unwrap(), completed_first);
    let blocked = read_forward_metadata_from_disk(&root, source)["humanForwardBatches"][0].clone();
    assert_forward_intent_unchanged(&legacy[0], &blocked);
    assert_eq!(blocked["destinations"][0]["completed"], true);
    assert_eq!(blocked["destinations"][1]["completed"], false);
    assert!(blocked["attachments"][0].get("forwardSnapshot").is_none(), "do not invent historical byte evidence");
    assert_eq!(fs::read(&snapshots[0]).unwrap(), files[0].2, "retain existing immutable data for recovery review");
    resumed.shutdown(); drop(resumed);
    server.finish(); fs::remove_dir_all(root).expect("remove legacy attachment fixture");
}

#[test]
fn shipping_human_forward_attachment_snapshot_respects_source_policy_and_normalized_privacy() {
    let root = temp_root("forward-attachment-privacy");
    let files = forward_attachment_files(&root, 6);
    let server = ForwardHttpFixture::start(false);
    let mut args = prepare_forward_attachment_process(&root, &server.url, &files);
    args["dropCaptions"] = json!(true);
    args["dropSenderNames"] = json!(false);
    let source = args["sourceConversationId"].as_str().unwrap();
    server.observe_snapshot_staging(&root, source);
    server.mutate_second_pin_after_first_upload();
    let runtime = human_message_fixture_runtime(&root, &server.url);
    let owner = runtime.open_human_conversation_db_owner(source).unwrap();
    let original = owner.get_transcript_entries().unwrap()[0].clone();
    let mut protected = original.clone(); protected["protectedContent"] = json!(true);
    owner.update_transcript_entry(args["sourceEntryId"].as_str().unwrap(), &protected).unwrap().unwrap();
    assert!(forward_gateway_result(&runtime, &args).is_err(), "source permission precedes snapshot and upload");
    let staging = forward_db_path(&root, source).parent().unwrap().join("forward-attachments");
    assert!(!staging.exists());
    assert!(read_forward_metadata_from_disk(&root, source).get("humanForwardBatches").is_none());
    assert_eq!(server.post_count(), 0); assert_eq!(server.resource_post_count(), 0);
    owner.update_transcript_entry(args["sourceEntryId"].as_str().unwrap(), &original).unwrap().unwrap();
    let sent = forward_gateway_result(&runtime, &args).expect("authorized six-attachment forward");
    assert_eq!(sent["privacy"], json!({"dropSenderNames":true,"dropCaptions":true}));
    assert_forward_staged_bytes(&root, &args, &files, &server);
    assert_forward_uploaded_bytes(&server, &files);
    {
        let state = server.state.lock().expect("preflight byte lifetime observation");
        assert!(state.mutation_observed_by_second_upload,
            "the second pin changed after preflight and before its actual upload");
        assert!(state.mutation_restored_before_message_ack,
            "restore the retained bytes before the next destination begins");
    }
    assert_eq!(server.post_count(), 2);
    for destination in args["destinationConversationIds"].as_array().unwrap() {
        let rows = runtime.read_human_conversation_transcript(destination.as_str().unwrap()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["content"], "", "caption privacy applies to actual persisted target");
        assert_eq!(rows[0]["delivery"], "sent");
        assert_eq!(rows[0]["forwardContext"]["privacy"], sent["privacy"]);
        assert!(rows[0]["forwardContext"].get("origin").is_none(), "hidden sender is not retained in target provenance");
        let attachments = rows[0]["attachments"].as_array().unwrap();
        for (attachment, (name, _, bytes)) in attachments.iter().zip(files.iter()) {
            assert_eq!(attachment["name"], json!(name));
            assert_eq!(fs::read(attachment["path"].as_str().unwrap()).unwrap(), *bytes,
                "downloaded target resource has the same uploaded bytes");
        }
    }
    assert_eq!(owner.get_transcript_entries().unwrap()[0], original, "privacy cannot mutate the source");
    runtime.shutdown(); drop(owner); drop(runtime);
    let reopened = human_message_fixture_runtime(&root, &server.url);
    let replay = forward_gateway_result(&reopened, &args).expect("completed privacy replay");
    assert_eq!(replay, sent);
    assert_eq!(server.post_count(), 2); assert_eq!(server.resource_post_count(), 12);
    reopened.shutdown(); drop(reopened);
    server.finish(); fs::remove_dir_all(root).expect("remove privacy snapshot fixture");
}

#[test]
fn shipping_human_forward_restart_corruption_fails_closed_and_recovers_exact_journal() {
    let root = temp_root("forward-corrupt-recovery");
    let mut server = ForwardHttpFixture::start(true);
    let args = prepare_forward_process(&root, &server.url);
    let mut child = ForwardProcess::start(&root, &server.url);
    server.wait_for_first_post();
    child.terminate();
    server.release_first_post();
    drop(child);
    let source = args["sourceConversationId"].as_str().unwrap();
    let db = rusqlite::Connection::open(forward_db_path(&root, source)).unwrap();
    let original_hex: String = db.query_row("SELECT value FROM kv WHERE key='metadata'", [], |row| row.get(0)).unwrap();
    assert_eq!(read_forward_metadata_from_disk(&root, source)["humanForwardBatches"][0]["requestId"], args["clientNonce"]);
    for cached_owner in [false, true] {
        // Independent bytes: invalid hex, invalid JSON, null, an array and a
        // string. Valid non-object JSON must not become "no forward journal".
        for damaged in ["zz", "6e6f742d6a736f6e", "6e756c6c", "5b5d", "2264616d6167656422"] {
            db.execute("UPDATE kv SET value=?1 WHERE key='metadata'", [&original_hex]).unwrap();
            let resumed = human_message_fixture_runtime(&root, &server.url);
            if cached_owner {
                resumed.open_human_conversation_db_owner(source).expect("cache valid source owner before damage");
            }
            db.execute("UPDATE kv SET value=?1 WHERE key='metadata'", [damaged]).unwrap();
            let target = args["destinationConversationIds"][1].as_str().unwrap();
            let error = resumed.sync_human_conversation(target).expect_err("corrupt source metadata must block recovery");
            assert!(!error.is_empty());
            assert_eq!(server.post_count(), 1);
            assert_eq!(server.history_count(), 0);
            let stored: String = db.query_row("SELECT value FROM kv WHERE key='metadata'", [], |row| row.get(0)).unwrap();
            assert_eq!(stored, damaged);
            resumed.shutdown(); drop(resumed);
        }
    }
    // Restore the original serialized metadata after malformed hex/JSON
    // cases before deriving the next valid-JSON corruption fixtures.
    db.execute("UPDATE kv SET value=?1 WHERE key='metadata'", [&original_hex]).unwrap();
    // Valid JSON may still contain a damaged journal or a no-longer-Human
    // source. Both must fail closed even when a Session cached its owner
    // before an independent SQLite handle changed the committed metadata.
    let original_metadata = read_forward_metadata_from_disk(&root, source);
    let valid_batch = original_metadata["humanForwardBatches"][0].clone();
    let mut wrong_version = valid_batch.clone();
    wrong_version["version"] = json!(2);
    let mut wrong_source = valid_batch.clone();
    wrong_source["sourceConversationId"] = json!("wrong-source");
    let mut wrong_nonce = valid_batch.clone();
    wrong_nonce["destinations"][0]["clientNonce"] = json!("wrong-nonce");
    let mut invalid_documents = Vec::new();
    for (label, journal) in [
        ("journal-null", serde_json::Value::Null),
        ("journal-object", json!({"not":"a batch array"})),
        ("journal-version", json!([wrong_version])),
        ("journal-source", json!([wrong_source])),
        ("journal-nonce", json!([wrong_nonce])),
        ("journal-duplicate", json!([valid_batch.clone(), valid_batch])),
    ] {
        let mut damaged = original_metadata.clone();
        damaged["humanForwardBatches"] = journal;
        invalid_documents.push((label, damaged));
    }
    let mut wrong_kind = original_metadata.clone();
    wrong_kind["conversationKind"] = json!("agent");
    invalid_documents.push(("source-kind", wrong_kind));
    let mut wrong_identity = original_metadata.clone();
    wrong_identity["conversationId"] = json!("different-conversation");
    invalid_documents.push(("source-identity", wrong_identity));
    for cached_owner in [false, true] {
        for (label, damaged) in &invalid_documents {
            db.execute("UPDATE kv SET value=?1 WHERE key='metadata'", [&original_hex]).unwrap();
            let resumed = human_message_fixture_runtime(&root, &server.url);
            if cached_owner {
                resumed.open_human_conversation_db_owner(source)
                    .expect("cache the authentic source before independent corruption");
            }
            let damaged_hex = damaged.to_string().as_bytes().iter()
                .map(|byte| format!("{byte:02x}")).collect::<String>();
            db.execute("UPDATE kv SET value=?1 WHERE key='metadata'", [&damaged_hex])
                .expect("inject valid JSON but invalid source/journal");
            let target = args["destinationConversationIds"][1].as_str().unwrap();
            let error = resumed.sync_human_conversation(target)
                .expect_err("damaged source or journal cannot resume dispatch");
            assert!(!error.is_empty(), "{label}: fail closed for both cold/cached owner");
            assert_eq!(server.post_count(), 1, "{label}: never retry an unverified forward");
            assert_eq!(server.history_count(), 0, "{label}: no later history after a corrupt source");
            let observed: String = db.query_row(
                "SELECT value FROM kv WHERE key='metadata'", [], |row| row.get(0)
            ).unwrap();
            assert_eq!(observed, damaged_hex, "{label}: recovery must not rewrite corrupt metadata");
            resumed.shutdown();
            drop(resumed);
        }
    }
    db.execute("UPDATE kv SET value=?1 WHERE key='metadata'", [&original_hex]).unwrap();
    drop(db);
    let restored = human_message_fixture_runtime(&root, &server.url);
    restored.sync_human_conversation(args["destinationConversationIds"][1].as_str().unwrap())
        .expect("recover original intent after repairing injected metadata");
    assert_forward_settled(&restored, &args);
    assert_eq!(server.post_count(), 3);
    assert_eq!(server.state.lock().unwrap().accepted.len(), 2);
    restored.shutdown(); drop(restored);
    server.finish(); fs::remove_dir_all(root).unwrap();
}



// NMR-49: ordinary reply intents share the existing Human SQLite owner and
// shipping gateway. These fixtures exercise durable edges, not a separate model.
fn pending_reply_fixture_row(nonce: &str, text: &str, reply: Option<&str>) -> serde_json::Value {
    let mut row = json!({
        "id":format!("human-message:{nonce}"),"kind":"message","role":"user",
        "authorKind":"human","authorId":"1","content":text,"clientNonce":nonce,
        "composedAtMs":1234.0,"timestampMs":1234.0,"delivery":"pending","attachments":[]
    });
    if let Some(reply) = reply { row["replyToId"] = json!(reply); }
    row
}

fn settled_reply_fixture_row(nonce: &str, text: &str, remote_id: u64) -> serde_json::Value {
    let mut row = pending_reply_fixture_row(nonce, text, None);
    row["id"] = json!(format!("human-server-message:{remote_id}"));
    row["remoteMessageId"] = json!(remote_id.to_string());
    row["delivery"] = json!("sent");
    row
}

#[test]
fn shipping_human_pending_reply_chain_recovers_after_restart_in_canonical_order() {
    let root = temp_root("nmr49-reply-chain");
    fs::create_dir_all(&root).expect("reply fixture root");
    let attachment_path = root.join("reply.txt");
    fs::write(&attachment_path, [1_u8, 3, 5, 7]).expect("reply attachment");
    let resource = json!({"resourceId":"nmr49-reply-resource","name":"reply.txt",
        "contentType":"application/octet-stream","size":4,"createdAt":"2026-10-05T02:00:00Z"});
    let parent = fixture_remote_human_message(894001, 1, 2, "parent", Some("nmr49-chain-a"), json!([]));
    let mut child = fixture_remote_human_message(894002, 1, 2, "child", Some("nmr49-chain-b"), json!([resource.clone()]));
    child["replyToMessageId"] = json!(894001);
    let mut grandchild = fixture_remote_human_message(894003, 1, 2, "grandchild", Some("nmr49-chain-c"), json!([]));
    grandchild["replyToMessageId"] = json!(894002);
    let empty = json!({"success":true,"data":{"messages":[]}});
    let (url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Disconnect,
        HumanMessageFixtureReply::Json(200, json!({"success":true,"deduplicated":true,"message":parent})),
        HumanMessageFixtureReply::Json(201, json!({"success":true,"resource":resource})),
        HumanMessageFixtureReply::Json(201, json!({"success":true,"message":child})),
        HumanMessageFixtureReply::Bytes(200, vec![1, 3, 5, 7]),
        HumanMessageFixtureReply::Json(201, json!({"success":true,"message":grandchild})),
        HumanMessageFixtureReply::Json(200, empty.clone()),
        HumanMessageFixtureReply::Json(200, empty),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"].as_str().unwrap().to_owned();
    for (text, nonce, reply, attachments) in [
        ("parent", "nmr49-chain-a", None, json!([])),
        ("child", "nmr49-chain-b", Some("human-message:nmr49-chain-a"),
            json!([{"path":attachment_path.to_string_lossy(),"name":"reply.txt"}])),
        ("grandchild", "nmr49-chain-c", Some("human-message:nmr49-chain-b"), json!([])),
    ] {
        let error = dispatch_production_session_gateway_call(&runtime, "sendHumanMessage", &json!({
            "conversationId":id,"text":text,"clientNonce":nonce,"replyToId":reply,
            "attachments":attachments,"composedAtMs":1234.0
        })).expect("registered send").expect_err("unsettled parent keeps a durable intent");
        if reply.is_some() {
            assert!(matches!(error, SessionGatewayError::Internal(message)
                if message.contains("no canonical remote message identity")));
        }
    }
    let staged = runtime.read_human_conversation_transcript(&id).expect("three durable intents");
    assert_eq!(staged.len(), 3);
    assert!(staged.iter().all(|row| row["delivery"] == "pending" && row.get("remoteMessageId").is_none()));
    runtime.shutdown(); drop(runtime);

    let events = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let captured = Arc::clone(&events);
    let event_id = id.clone();
    let subscription = mahayana_host_runtime::transcript_mutation_events::subscribe_transcript_mutations(move |mutation| {
        if mutation.get("agentId").and_then(serde_json::Value::as_str) == Some(event_id.as_str())
            && mutation.get("entries").and_then(serde_json::Value::as_array).is_some_and(|entries| {
                entries.iter().any(|entry| entry["clientNonce"].as_str().is_some_and(|nonce| nonce.starts_with("nmr49-chain-")))
            })
        { captured.lock().unwrap().push(serde_json::Value::Object(mutation.clone())); }
    });
    let recovered = human_message_fixture_runtime(&root, &url);
    let rows = dispatch(&recovered, "syncHumanConversation", json!({"conversationId":id}));
    let rows = rows.as_array().expect("gateway transcript").clone();
    assert_eq!(rows.len(), 3);
    for (index, (remote, nonce, text)) in [
        (894001, "nmr49-chain-a", "parent"),
        (894002, "nmr49-chain-b", "child"),
        (894003, "nmr49-chain-c", "grandchild"),
    ].into_iter().enumerate() {
        assert_eq!(rows[index]["id"], format!("human-server-message:{remote}"));
        assert_eq!(rows[index]["clientNonce"], nonce);
        assert_eq!(rows[index]["content"], text);
        assert_eq!(rows[index]["delivery"], "sent");
    }
    assert_eq!(rows[1]["replyToId"], "human-server-message:894001");
    assert_eq!(rows[2]["replyToId"], "human-server-message:894002");
    assert_eq!(rows[1]["attachments"][0]["resourceId"], "nmr49-reply-resource");
    {
        let events = events.lock().unwrap();
        for (parent_id, child_id, before) in [
            ("human-server-message:894001", "human-message:nmr49-chain-b", &staged[1]),
            ("human-server-message:894002", "human-message:nmr49-chain-c", &staged[2]),
        ] {
            let rewritten = events.iter().filter_map(|event| event["entries"].as_array())
                .find(|entries| entries.iter().any(|row| row["id"] == parent_id)
                    && entries.iter().any(|row| row["id"] == child_id))
                .expect("parent and child are published together after commit")
                .iter().find(|row| row["id"] == child_id).unwrap();
            let mut expected = before.clone();
            expected["replyToId"] = json!(parent_id);
            assert_eq!(rewritten, &expected, "repair preserves all other child fields");
        }
    }
    drop(subscription);
    recovered.shutdown(); drop(recovered);
    let reopened = human_message_fixture_runtime(&root, &url);
    assert_eq!(reopened.read_human_conversation_transcript(&id).unwrap(), rows);
    assert_eq!(reopened.sync_human_conversation(&id).expect("settled restart is idempotent"), rows);
    reopened.shutdown(); drop(reopened);
    let requests = server.join().expect("reply chain HTTP");
    assert_eq!(requests.len(), 8);
    let sends = requests.iter().filter(|request| request.starts_with("POST /api/social/messages ")).collect::<Vec<_>>();
    assert_eq!(sends.len(), 4, "only the ambiguous parent retries; children each dispatch once");
    assert_eq!(fixture_request_json(sends[0]), fixture_request_json(sends[1]));
    assert_eq!(fixture_request_json(sends[2])["clientRequestId"], "nmr49-chain-b");
    assert_eq!(fixture_request_json(sends[2])["replyToMessageId"], "894001");
    assert_eq!(fixture_request_json(sends[3])["replyToMessageId"], "894002");
    assert_eq!(requests.iter().filter(|request| request.starts_with("POST /api/social/message-resources ")).count(), 1);
    fs::remove_dir_all(root).expect("remove reply chain fixture");
}

#[test]
fn shipping_human_historical_reply_aliases_require_unique_local_canonical_parent() {
    for variant in ["valid", "missing", "peer-only", "foreign-only", "ambiguous", "conflicting-id", "unsettled", "conflicting-dispatch", "peer-local-id", "non-human-parent", "non-human-local-parent"] {
        let root = temp_root(&format!("nmr49-reply-alias-{variant}"));
        let parent_nonce = format!("nmr49-alias-parent-{variant}");
        let child_nonce = format!("nmr49-alias-child-{variant}");
        let old_parent = format!("human-message:{parent_nonce}");
        let mut remote = fixture_remote_human_message(895002, 1, 2, "historical child", Some(&child_nonce), json!([]));
        remote["replyToMessageId"] = json!(895001);
        let mut replies = Vec::new();
        if variant == "valid" {
            replies.push(HumanMessageFixtureReply::Json(201, json!({"success":true,"message":remote})));
        }
        replies.push(HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[]}})));
        let (url, server) = spawn_human_message_fixture(replies);
        let runtime = human_message_fixture_runtime(&root, &url);
        let id = runtime.create_human_conversation("2", "Peer").unwrap()["id"].as_str().unwrap().to_owned();
        let foreign_id = runtime.create_human_conversation("3", "Other peer").unwrap()["id"].as_str().unwrap().to_owned();
        let owner = runtime.open_human_conversation_db_owner(&id).unwrap();
        let foreign = runtime.open_human_conversation_db_owner(&foreign_id).unwrap();
        let foreign_parent = settled_reply_fixture_row(&parent_nonce, "foreign parent", 895901);
        assert!(foreign.append_transcript_entry(&foreign_parent).unwrap());
        let mut peer = settled_reply_fixture_row(&parent_nonce, "peer same nonce", 895101);
        peer["authorId"] = json!("2");
        if variant == "peer-local-id" { peer["id"] = json!(old_parent); }
        if variant != "missing" && variant != "foreign-only" {
            assert!(owner.append_transcript_entry(&peer).unwrap());
        }
        if matches!(variant, "valid" | "ambiguous" | "conflicting-id" | "unsettled" | "conflicting-dispatch" | "non-human-parent" | "non-human-local-parent") {
            let mut parent = settled_reply_fixture_row(&parent_nonce, "local parent", 895001);
            if variant == "conflicting-id" { parent["id"] = json!("human-server-message:895099"); }
            if variant == "unsettled" { parent["delivery"] = json!("pending"); }
            if matches!(variant, "non-human-parent" | "non-human-local-parent") { parent["authorKind"] = json!("agent"); }
            if variant == "non-human-local-parent" { parent["id"] = json!(old_parent); }
            assert!(owner.append_transcript_entry(&parent).unwrap());
            if variant == "ambiguous" {
                assert!(owner.append_transcript_entry(&settled_reply_fixture_row(&parent_nonce, "duplicate nonce", 895003)).unwrap());
            }
        }
        let mut pending = pending_reply_fixture_row(&child_nonce, "historical child", Some(&old_parent));
        if variant == "conflicting-dispatch" { pending["remoteReplyToMessageId"] = json!("895099"); }
        assert!(owner.append_transcript_entry(&pending).unwrap());
        let before = owner.get_transcript_entries().unwrap();
        // All fixture rows start at one timestamp. Canonical Human reads order
        // remote 895001, then 895003 when present, then peer 895101, then the
        // pending child. Physical insertion slots remain an independent oracle.
        let chronological_before = if variant == "ambiguous" {
            vec![before[1].clone(), before[2].clone(), before[0].clone(), before[3].clone()]
        } else if matches!(variant, "valid" | "conflicting-id" | "unsettled" | "conflicting-dispatch" | "non-human-parent" | "non-human-local-parent") {
            vec![before[1].clone(), before[0].clone(), before[2].clone()]
        } else {
            before.clone()
        };
        drop(owner); drop(foreign); runtime.shutdown(); drop(runtime);
        let recovered = human_message_fixture_runtime(&root, &url);
        let reply = if variant == "valid" { "human-server-message:895001" } else { old_parent.as_str() };
        let result = dispatch_production_session_gateway_call(&recovered, "sendHumanMessage", &json!({
            "conversationId":id,"text":"historical child","clientNonce":child_nonce,
            "replyToId":reply,"attachments":[],"composedAtMs":1234.0
        })).expect("registered historical send");
        if variant == "valid" {
            let settled = result.expect("normalize both requested canonical and persisted old reply");
            assert_eq!(settled["replyToId"], "human-server-message:895001");
            assert_eq!(settled["clientNonce"], child_nonce);
            let replay = dispatch(&recovered, "sendHumanMessage", json!({
                "conversationId":id,"text":"historical child","clientNonce":child_nonce,
                "replyToId":old_parent,"attachments":[],"composedAtMs":1234.0
            }));
            assert_eq!(replay, settled, "old and canonical IDs are the same immutable reply intent");
        } else {
            let error = result.expect_err("unsafe alias must remain rejected");
            assert!(matches!(error, SessionGatewayError::Internal(message) if message.contains("reply")), "{variant}");
            assert_eq!(recovered.open_human_conversation_db_owner(&id).unwrap()
                .get_transcript_entries().unwrap(), before, "{variant}: rejected ingress changes no physical row");
            assert_eq!(recovered.read_human_conversation_transcript(&id).unwrap(), chronological_before,
                "{variant}: rejected ingress preserves every complete canonical row");
        }
        let rows = recovered.sync_human_conversation(&id).expect("independent authenticated history still progresses");
        if variant != "valid" {
            assert_eq!(recovered.open_human_conversation_db_owner(&id).unwrap()
                .get_transcript_entries().unwrap(), before, "{variant}: retry retains its exact durable physical intent");
            assert_eq!(rows, chronological_before, "{variant}: retry retains every canonical row in its specified order");
        }
        if variant != "missing" && variant != "foreign-only" {
            assert!(rows.iter().any(|row| row == &peer), "{variant}: never borrow or rewrite peer nonce");
        }
        assert_eq!(recovered.read_human_conversation_transcript(&foreign_id).unwrap(), vec![foreign_parent]);
        recovered.shutdown(); drop(recovered);
        let reopened = human_message_fixture_runtime(&root, &url);
        assert_eq!(reopened.read_human_conversation_transcript(&id).unwrap(), rows);
        reopened.shutdown(); drop(reopened);
        let requests = server.join().expect("historical alias HTTP");
        assert_eq!(requests.len(), if variant == "valid" { 2 } else { 1 }, "{variant}");
        assert!(requests.last().unwrap().starts_with("GET /api/social/messages?"));
        if variant == "valid" {
            assert_eq!(fixture_request_json(&requests[0])["replyToMessageId"], "895001");
        } else {
            assert!(requests.iter().all(|request| request.starts_with("GET ")), "{variant}: no unsafe POST or resource access");
        }
        fs::remove_dir_all(root).expect("remove historical alias fixture");
    }

    // The strict message-ID contract applies to both a directly selected parent
    // and a historical local alias before either may mutate a durable child.
    let mut numeric_cases = Vec::new();
    for (label, remote_id, canonical_suffix) in [
        ("leading-zero", json!("0895001"), "0895001"),
        ("padded", json!(" 895001 "), "895001"),
        ("signed", json!("+895001"), "+895001"),
        ("fractional", json!(895001.5), "895001.5"),
        ("zero", json!(0), "0"),
        ("negative", json!(-1), "-1"),
        ("unsafe", json!(9_007_199_254_740_992_u64), "9007199254740992"),
    ] {
        for mode in ["direct", "historical"] {
            numeric_cases.push((mode, label, remote_id.clone(), canonical_suffix));
        }
    }
    numeric_cases.extend([
        ("direct", "canonical-integer", json!(895001), "895001"),
        ("historical", "max-safe-equivalence", json!("9007199254740991"), "9007199254740991"),
        ("direct", "peer-canonical", json!("895001"), "895001"),
    ]);
    for (mode, label, remote_id, canonical_suffix) in numeric_cases {
        let valid = matches!(label, "canonical-integer" | "max-safe-equivalence" | "peer-canonical");
        let root = temp_root(&format!("nmr49-numeric-reply-{mode}-{label}"));
        let parent_nonce = format!("nmr49-numeric-parent-{mode}-{label}");
        let child_nonce = format!("nmr49-numeric-child-{mode}-{label}");
        let old_parent = format!("human-message:{parent_nonce}");
        let canonical_parent = format!("human-server-message:{canonical_suffix}");
        let requested_reply = if mode == "historical" { old_parent.as_str() } else { canonical_parent.as_str() };
        let mut remote = fixture_remote_human_message(895002, 1, 2, "numeric reply child", Some(&child_nonce), json!([]));
        remote["replyToMessageId"] = if label == "max-safe-equivalence" {
            json!(9_007_199_254_740_991_u64)
        } else { json!(895001) };
        let mut responses = Vec::new();
        if valid {
            responses.push(HumanMessageFixtureReply::Json(201, json!({"success":true,"message":remote})));
        }
        responses.push(HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[]}})));
        let (url, server) = spawn_human_message_fixture(responses);
        let runtime = human_message_fixture_runtime(&root, &url);
        let id = runtime.create_human_conversation("2", "Peer").unwrap()["id"].as_str().unwrap().to_owned();
        let owner = runtime.open_human_conversation_db_owner(&id).unwrap();
        let mut peer = settled_reply_fixture_row(&parent_nonce, "peer with matching nonce", 895101);
        peer["authorId"] = json!("2");
        let mut parent = settled_reply_fixture_row(&parent_nonce, "numeric reply parent", 895001);
        parent["id"] = json!(canonical_parent);
        parent["remoteMessageId"] = remote_id;
        if label == "peer-canonical" { parent["authorId"] = json!("2"); }
        let pending = pending_reply_fixture_row(&child_nonce, "numeric reply child", Some(requested_reply));
        let before = vec![peer.clone(), parent.clone(), pending];
        assert_eq!(owner.append_transcript_entries(&before).unwrap(), 3);
        assert_eq!(owner.get_transcript_entries().unwrap(), before, "{mode}/{label}: all fixture rows remain readable");
        // Each row starts at 1234ms. The leading-zero fixture still sorts as
        // numeric 895001 on read, despite being inadmissible as a reply ID.
        // Padded/signed/fractional/nonpositive/unsafe IDs sort after peer 895101;
        // MAX_SAFE is valid but also follows that peer. Equal fallback keys
        // retain physical parent-before-child order. These are literal fixture
        // expectations, independent of the production ordering implementation.
        let parent_before_peer = matches!(label, "leading-zero" | "canonical-integer" | "peer-canonical");
        let chronological_before = if parent_before_peer {
            vec![before[1].clone(), before[0].clone(), before[2].clone()]
        } else { before.clone() };
        drop(owner); runtime.shutdown(); drop(runtime);
        let recovered = human_message_fixture_runtime(&root, &url);
        let result = dispatch_production_session_gateway_call(&recovered, "sendHumanMessage", &json!({
            "conversationId":id,"text":"numeric reply child","clientNonce":child_nonce,
            "replyToId":requested_reply,"attachments":[],"composedAtMs":1234.0
        })).expect("registered numeric reply send");
        let expected_physical;
        let expected_canonical;
        if valid {
            let settled = result.expect("canonical numeric reply identity is admissible");
            assert_eq!(settled["id"], "human-server-message:895002");
            assert_eq!(settled["replyToId"], canonical_parent);
            assert_eq!(settled["clientNonce"], child_nonce);
            if label == "max-safe-equivalence" {
                // The historical alias first used the canonical string boundary.
                // Replaying the same child through a direct ID must also accept
                // the identical safe integer representation without another POST.
                parent["remoteMessageId"] = json!(9_007_199_254_740_991_u64);
                assert_eq!(recovered.open_human_conversation_db_owner(&id).unwrap()
                    .update_transcript_entry(&canonical_parent, &parent).unwrap(), Some(parent.clone()));
            }
            let replay = dispatch(&recovered, "sendHumanMessage", json!({
                "conversationId":id,"text":"numeric reply child","clientNonce":child_nonce,
                "replyToId":canonical_parent,"attachments":[],"composedAtMs":1234.0
            }));
            assert_eq!(replay, settled, "{mode}/{label}: direct canonical retry is idempotent");
            expected_physical = vec![peer.clone(), parent.clone(), settled.clone()];
            // The real HTTP settlement has an October 2026 timestamp, after
            // both fixture parents. Their explicit relative order stays fixed.
            expected_canonical = if parent_before_peer {
                vec![parent.clone(), peer.clone(), settled]
            } else { expected_physical.clone() };
        } else {
            let error = result.expect_err("noncanonical numeric reply must be refused before dispatch");
            assert!(matches!(error, SessionGatewayError::Internal(message)
                if message.contains("reply") && message.contains("canonical positive safe integer")),
                "{mode}/{label}: the numeric admission guard must reject the complete parent");
            expected_physical = before.clone();
            expected_canonical = chronological_before;
        }
        assert_eq!(recovered.open_human_conversation_db_owner(&id).unwrap()
            .get_transcript_entries().unwrap(), expected_physical,
            "{mode}/{label}: ingress preserves every physical row except an admitted settlement");
        assert_eq!(recovered.read_human_conversation_transcript(&id).unwrap(), expected_canonical,
            "{mode}/{label}: ingress preserves the complete canonical projection");
        let rows = recovered.sync_human_conversation(&id).expect("independent authorized history still progresses");
        assert_eq!(recovered.open_human_conversation_db_owner(&id).unwrap()
            .get_transcript_entries().unwrap(), expected_physical,
            "{mode}/{label}: automatic replay cannot rewrite a refused numeric intent");
        assert_eq!(rows, expected_canonical, "{mode}/{label}: sync keeps the specified canonical order");
        recovered.shutdown(); drop(recovered);
        let reopened = human_message_fixture_runtime(&root, &url);
        assert_eq!(reopened.open_human_conversation_db_owner(&id).unwrap()
            .get_transcript_entries().unwrap(), expected_physical,
            "{mode}/{label}: restart preserves exact durable payloads");
        assert_eq!(reopened.read_human_conversation_transcript(&id).unwrap(), expected_canonical,
            "{mode}/{label}: restart preserves canonical order");
        reopened.shutdown(); drop(reopened);
        let requests = server.join().expect("numeric reply HTTP");
        assert_eq!(requests.len(), if valid { 2 } else { 1 }, "{mode}/{label}");
        assert!(requests.last().unwrap().starts_with("GET /api/social/messages?"));
        let posts = requests.iter().filter(|request| request.starts_with("POST /api/social/messages ")).collect::<Vec<_>>();
        assert_eq!(posts.len(), usize::from(valid), "{mode}/{label}: exactly one admitted send and no retry duplicate");
        if valid {
            assert_eq!(fixture_request_json(posts[0])["replyToMessageId"], canonical_suffix);
            assert_eq!(fixture_request_json(posts[0])["clientRequestId"], child_nonce);
        } else {
            assert!(requests.iter().all(|request| request.starts_with("GET ")),
                "{mode}/{label}: invalid reply produces no POST or resource access");
        }
        fs::remove_dir_all(root).expect("cleanup numeric reply fixture");
    }

}

#[test]
fn shipping_human_direct_reply_targets_reject_corrupt_identity_before_staging() {
    for variant in ["canonical-non-message","canonical-agent","canonical-pending","canonical-no-remote","canonical-conflicting-remote","canonical-missing-author","canonical-empty-author","canonical-corrupt-remote","local-non-message","local-agent","local-peer-author","local-mismatched-nonce","local-empty-nonce","local-has-remote","local-failed","local-missing-author-kind","local-unsupported-delivery"] {
        let root = temp_root(&format!("nmr49-direct-{variant}"));
        let (url, server) = spawn_human_message_fixture(vec![]);
        let runtime = human_message_fixture_runtime(&root, &url);
        let id = runtime.create_human_conversation("2", "Peer").unwrap()["id"].as_str().unwrap().to_owned();
        let nonce = format!("nmr49-parent-{variant}");
        let mut parent = if variant.starts_with("canonical-") {
            settled_reply_fixture_row(&nonce, "parent", 895701)
        } else { pending_reply_fixture_row(&nonce, "parent", None) };
        match variant {
            "canonical-non-message" | "local-non-message" => {
                // A valid non-message row must reach reply admission; an event
                // without event.type would disappear during transcript parsing.
                parent["kind"] = json!("notice");
                parent["text"] = json!("retained non-message reply parent");
            }
            "canonical-agent" | "local-agent" => parent["authorKind"] = json!("agent"),
            "canonical-pending" => parent["delivery"] = json!("pending"),
            "canonical-no-remote" => { parent.as_object_mut().unwrap().remove("remoteMessageId"); }
            "canonical-conflicting-remote" => parent["remoteMessageId"] = json!("895702"),
            "canonical-missing-author" => { parent.as_object_mut().unwrap().remove("authorId"); }
            "canonical-empty-author" => parent["authorId"] = json!(""),
            "canonical-corrupt-remote" => parent["remoteMessageId"] = json!({"unexpected":1}),
            "local-peer-author" => parent["authorId"] = json!("2"),
            "local-mismatched-nonce" => parent["clientNonce"] = json!("different"),
            "local-empty-nonce" => parent["clientNonce"] = json!(""),
            "local-has-remote" => parent["remoteMessageId"] = json!("895701"),
            "local-failed" => parent["delivery"] = json!("failed"),
            "local-missing-author-kind" => { parent.as_object_mut().unwrap().remove("authorKind"); }
            "local-unsupported-delivery" => parent["delivery"] = json!("queued"),
            _ => unreachable!(),
        }
        let reply_id = parent["id"].as_str().unwrap().to_owned();
        let owner = runtime.open_human_conversation_db_owner(&id).unwrap();
        assert!(owner.append_transcript_entry(&parent).unwrap());
        let before = owner.get_transcript_entries().unwrap();
        assert_eq!(before, vec![parent.clone()], "{variant}: the complete fixture reaches reply admission");
        let result = dispatch_production_session_gateway_call(&runtime, "sendHumanMessage", &json!({
            "conversationId":id,"text":"child","clientNonce":format!("child-{variant}"),
            "replyToId":reply_id,"attachments":[],"composedAtMs":1234.0
        })).expect("registered Human gateway");
        assert!(matches!(result, Err(SessionGatewayError::Internal(message)) if message.contains("reply")),
            "{variant}: invalid parent must be rejected");
        assert_eq!(owner.get_transcript_entries().unwrap(), before, "{variant}: no child staged");
        drop(owner); runtime.shutdown(); drop(runtime);
        let reopened = human_message_fixture_runtime(&root, &url);
        assert_eq!(reopened.read_human_conversation_transcript(&id).unwrap(), before);
        reopened.shutdown(); drop(reopened);
        assert!(server.join().expect("unexpected HTTP fixture").is_empty());
        fs::remove_dir_all(root).expect("cleanup direct identity fixture");
    }
}

#[test]
fn shipping_human_reply_identity_transaction_rolls_back_children_and_recovers_after_reopen() {
    let root = temp_root("nmr49-reply-atomicity");
    let mut parent = fixture_remote_human_message(896001, 1, 2, "atomic parent", Some("nmr49-atomic-a"), json!([]));
    let mut child = fixture_remote_human_message(896002, 1, 2, "atomic child", Some("nmr49-atomic-b"), json!([]));
    child["replyToMessageId"] = json!(896001);
    let mut sibling = fixture_remote_human_message(896003, 1, 2, "atomic sibling", Some("nmr49-atomic-c"), json!([]));
    sibling["replyToMessageId"] = json!(896001);
    parent["replyToMessageId"] = serde_json::Value::Null;
    let empty = json!({"success":true,"data":{"messages":[]}});
    let (url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(201, json!({"success":true,"message":parent})),
        HumanMessageFixtureReply::Json(201, json!({"success":true,"message":child})),
        HumanMessageFixtureReply::Json(201, json!({"success":true,"message":sibling})),
        HumanMessageFixtureReply::Json(200, empty.clone()),
        HumanMessageFixtureReply::Json(200, empty),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").unwrap()["id"].as_str().unwrap().to_owned();
    let owner = runtime.open_human_conversation_db_owner(&id).unwrap();
    let old_parent = "human-message:nmr49-atomic-a";
    let parent_row = pending_reply_fixture_row("nmr49-atomic-a", "atomic parent", None);
    let mut child_row = pending_reply_fixture_row("nmr49-atomic-b", "atomic child", Some(old_parent));
    child_row["forwardContext"] = json!({"preserve":"existing ordinary reply metadata"});
    child_row["retainedLocalMetadata"] = json!({"value":17});
    let mut sibling_row = pending_reply_fixture_row("nmr49-atomic-c", "atomic sibling", Some(old_parent));
    sibling_row["delivery"] = json!("dispatching");
    let mut peer_row = pending_reply_fixture_row("nmr49-atomic-peer", "foreign author", Some(old_parent));
    peer_row["authorId"] = json!("2");
    let mut sent_row = settled_reply_fixture_row("nmr49-atomic-sent", "already canonical", 896099);
    sent_row["replyToId"] = json!(old_parent);
    let mut damaged_canonical = pending_reply_fixture_row("nmr49-atomic-damaged", "canonical ID without remote field", Some(old_parent));
    damaged_canonical["id"] = json!("human-server-message:896098");
    let mut non_human = pending_reply_fixture_row("nmr49-atomic-agent", "other author kind", Some(old_parent));
    non_human["authorKind"] = json!("agent");
    let before = vec![parent_row, child_row.clone(), sibling_row.clone(), peer_row.clone(),
        sent_row.clone(), damaged_canonical.clone(), non_human.clone()];
    assert_eq!(owner.append_transcript_entries(&before).unwrap(), 7);
    let events = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let captured = Arc::clone(&events); let event_id = id.clone();
    let subscription = mahayana_host_runtime::transcript_mutation_events::subscribe_transcript_mutations(move |mutation| {
        if mutation.get("agentId").and_then(serde_json::Value::as_str) != Some(event_id.as_str()) { return; }
        let targets = mutation.get("entryId").and_then(serde_json::Value::as_str)
            .is_some_and(|id| id.starts_with("human-message:nmr49-atomic-"))
            || mutation.get("entries").and_then(serde_json::Value::as_array).is_some_and(|entries| {
                entries.iter().any(|entry| entry["clientNonce"].as_str().is_some_and(|nonce| nonce.starts_with("nmr49-atomic-")))
            });
        if targets { captured.lock().unwrap().push(serde_json::Value::Object(mutation.clone())); }
    });
    let db = rusqlite::Connection::open(forward_db_path(&root, &id)).expect("real SQLite fault injector");
    for target in [old_parent, "human-message:nmr49-atomic-c"] {
        for failure in ["RAISE(ABORT, 'NMR-49 identity write failure')", "RAISE(IGNORE)"] {
            db.execute_batch(&format!("CREATE TRIGGER reject_reply_child BEFORE UPDATE OF entry ON transcript_entries
                WHEN OLD.id = '{target}' BEGIN SELECT {failure}; END;"))
                .expect("install real parent/child write rejection");
            assert!(owner.settle_transcript_entry_identity(old_parent,
                &settled_reply_fixture_row("nmr49-atomic-a", "atomic parent", 896001)).is_err(), "{target}: {failure}");
            assert_eq!(owner.get_transcript_entries().unwrap(), before,
                "{target}: {failure}: parent and earlier child updates roll back together");
            assert!(events.lock().unwrap().is_empty(), "failed owner transaction publishes no identity mutation");
            db.execute_batch("DROP TRIGGER reject_reply_child;").expect("remove scoped fault");
        }
    }
    drop(db); drop(owner); runtime.shutdown(); drop(runtime);

    let recovered = human_message_fixture_runtime(&root, &url);
    let recovered_owner = recovered.open_human_conversation_db_owner(&id).unwrap();
    assert_eq!(recovered_owner.get_transcript_entries().unwrap(), before,
        "reopen preserves all seven physical slots after each failed transaction");
    assert_eq!(recovered.read_human_conversation_transcript(&id).unwrap(), vec![
        before[4].clone(), before[0].clone(), before[1].clone(), before[2].clone(),
        before[3].clone(), before[5].clone(), before[6].clone(),
    ], "the existing remote child sorts before equal-time pending rows");
    dispatch(&recovered, "sendHumanMessage", json!({"conversationId":id,"text":"atomic parent",
        "clientNonce":"nmr49-atomic-a","attachments":[],"composedAtMs":1234.0}));
    let repaired = recovered_owner.get_transcript_entries().unwrap();
    child_row["replyToId"] = json!("human-server-message:896001");
    sibling_row["replyToId"] = json!("human-server-message:896001");
    assert_eq!(repaired[1], child_row, "child keeps nonce, content, resources, forwarding and local metadata");
    assert_eq!(repaired[2], sibling_row, "dispatching child keeps every field except the reply edge");
    assert_eq!(repaired[3], peer_row, "remote author is outside the repair scope");
    assert_eq!(repaired[4], sent_row, "canonical child is immutable");
    assert_eq!(repaired[5], damaged_canonical, "a canonical ID is not an unsent local identity even without its remote field");
    assert_eq!(repaired[6], non_human, "another author kind is outside the Human repair scope");
    assert_eq!(recovered.read_human_conversation_transcript(&id).unwrap(), vec![
        repaired[4].clone(), repaired[1].clone(), repaired[2].clone(), repaired[3].clone(),
        repaired[5].clone(), repaired[6].clone(), repaired[0].clone(),
    ], "canonical reads retain every repaired payload while the settled parent acquires its authoritative time");
    assert!(events.lock().unwrap().iter().any(|event| event["entries"].as_array().is_some_and(|entries| {
        entries.len() == 3 && entries[0]["id"] == "human-server-message:896001"
            && entries[1] == child_row && entries[2] == sibling_row
    })), "committed parent and both repaired children reach the mounted projection together");
    let rows = recovered.sync_human_conversation(&id).expect("repaired children replay through shipping HTTP");
    assert_eq!(rows.len(), 7);
    let physical = recovered_owner.get_transcript_entries().unwrap();
    assert_eq!(physical.len(), 7);
    assert_eq!(physical[1]["id"], "human-server-message:896002");
    assert_eq!(physical[1]["forwardContext"], child_row["forwardContext"]);
    assert_eq!(physical[2]["id"], "human-server-message:896003");
    assert_eq!(physical[3], peer_row); assert_eq!(physical[4], sent_row);
    assert_eq!(physical[5], damaged_canonical); assert_eq!(physical[6], non_human);
    assert_eq!(rows, vec![
        physical[4].clone(), physical[3].clone(), physical[5].clone(), physical[6].clone(),
        physical[0].clone(), physical[1].clone(), physical[2].clone(),
    ], "canonical history keeps old equal-time rows followed by the three real HTTP settlements");
    drop(subscription); recovered.shutdown(); drop(recovered_owner); drop(recovered);
    let reopened = human_message_fixture_runtime(&root, &url);
    assert_eq!(reopened.sync_human_conversation(&id).expect("second restart cannot duplicate sends"), rows);
    reopened.shutdown(); drop(reopened);
    let requests = server.join().expect("atomic reply recovery HTTP");
    assert_eq!(requests.len(), 5);
    let posts = requests.iter().filter(|request| request.starts_with("POST /api/social/messages ")).collect::<Vec<_>>();
    assert_eq!(posts.len(), 3);
    assert_eq!(fixture_request_json(posts[1])["replyToMessageId"], "896001");
    assert_eq!(fixture_request_json(posts[2])["replyToMessageId"], "896001");
    fs::remove_dir_all(root).expect("remove atomic reply fixture");
}


// NMR-PAGING-01: the existing Human SQLite owner fixes walk membership,
// while each page reads current canonical payloads after real mutations.
fn fixture_saved_human_message(id: u64, peer: &str, text: &str) -> serde_json::Value {
    json!({
        "id":format!("human-server-message:{id}"),"kind":"message","role":"user",
        "authorKind":"human","authorId":peer,"content":text,"timestampMs":1791162000000.0,
        "delivery":"sent","remoteMessageId":id.to_string(),"remoteCreatedAt":"2026-10-05T01:00:00Z",
        "reactions":[],"remoteReactions":[],"retainedLocalMetadata":{"keep":true}
    })
}

#[test]
fn shipping_human_tail_keeps_physical_anchors_stable_during_backfill_and_equal_time_inserts() {
    let root = temp_root("nmr-human-stable-tail");
    let runtime = human_message_fixture_runtime(&root, "http://127.0.0.1:1");
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("id").to_owned();
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    let mut physical = Vec::new();
    for (remote_id, timestamp) in [(200, 3000.0), (10, 1000.0), (20, 2000.0), (30, 2000.0)] {
        let mut row = fixture_saved_human_message(remote_id, "2", "ordered");
        row["timestampMs"] = json!(timestamp);
        physical.push(row);
    }
    physical.push(json!({"id":"human-message:pending-tail","kind":"message","role":"user",
        "authorKind":"human","authorId":"1","content":"pending","timestampMs":2000.0,"delivery":"pending"}));
    let mut row25 = fixture_saved_human_message(25, "2", "equal time");
    row25["timestampMs"] = json!(2000.0);
    physical.push(row25);
    assert_eq!(owner.append_transcript_entries(&physical).expect("physical rows"), physical.len());
    let first = runtime.read_human_conversation_transcript_tail(&id, HumanTranscriptPageQuery { before_seq:None, before_cursor:None, limit:2, topic_id:None })
        .expect("newest human tail");
    assert_eq!(first.entries.iter().map(|row| row["id"].as_str().unwrap()).collect::<Vec<_>>(),
        vec!["human-message:pending-tail", "human-server-message:200"]);
    assert_eq!(first.next_before_seq, Some(5), "return the real physical seq, not the sorted index");
    let cursor = first.next_before_cursor.clone().expect("immutable history snapshot cursor");
    for (remote_id, timestamp) in [(15, 1500.0), (29, 2000.0)] {
        let mut row = fixture_saved_human_message(remote_id, "2", "backfilled");
        row["timestampMs"] = json!(timestamp);
        assert!(owner.append_transcript_entry(&row).expect("late insertion"));
        physical.push(row);
    }
    let second = runtime.read_human_conversation_transcript_tail(&id, HumanTranscriptPageQuery {
        before_seq:first.next_before_seq,before_cursor:Some(cursor.clone()),limit:3,topic_id:None
    }).expect("older relative to the immutable pending anchor");
    assert_eq!(second.entries.iter().map(|row| row["remoteMessageId"].as_str().unwrap()).collect::<Vec<_>>(), vec!["20","25","30"],
        "late inserts belong to the next snapshot, not the in-progress stable traversal");
    assert_eq!(second.next_before_seq, Some(3));
    let third = runtime.read_human_conversation_transcript_tail(&id, HumanTranscriptPageQuery {
        before_seq:second.next_before_seq,before_cursor:second.next_before_cursor.clone(),limit:3,topic_id:None
    }).expect("oldest relative to ID 20");
    assert_eq!(third.entries.iter().map(|row| row["remoteMessageId"].as_str().unwrap()).collect::<Vec<_>>(), vec!["10"]);
    assert_eq!(third.next_before_seq, None);
    assert_eq!(third.next_before_cursor, None);
    assert_eq!(owner.get_transcript_entries().expect("legacy Agent sequence semantics"), physical);
    let all = runtime.read_human_conversation_transcript(&id).expect("one human ordering");
    assert_eq!(all.iter().map(|row| row["id"].as_str().unwrap()).collect::<Vec<_>>(), vec![
        "human-server-message:10","human-server-message:15","human-server-message:20","human-server-message:25",
        "human-server-message:29","human-server-message:30","human-message:pending-tail","human-server-message:200"
    ]);
    let stable = third.entries.into_iter().chain(second.entries.clone()).chain(first.entries).collect::<Vec<_>>();
    assert_eq!(stable.iter().map(|row| row["id"].as_str().unwrap()).collect::<Vec<_>>(), vec![
        "human-server-message:10","human-server-message:20","human-server-message:25",
        "human-server-message:30","human-message:pending-tail","human-server-message:200"
    ]);
    assert!(runtime.read_human_conversation_transcript_tail(&id, HumanTranscriptPageQuery {
        before_seq:Some(9999),before_cursor:Some(cursor.clone()),limit:2,topic_id:None
    }).is_err(), "a mismatched physical anchor cannot reinterpret the token");
    assert!(runtime.read_human_conversation_transcript_tail(&id, HumanTranscriptPageQuery {
        before_seq:None,before_cursor:Some("not-a-history-token".into()),limit:2,topic_id:None
    }).is_err(), "malformed cursor fails closed");
    let original_participants = owner.get_metadata("participantIds").expect("original participants")
        .expect("Human participants exist");
    assert!(owner.set_metadata("participantIds", json!(["1", "9"])).expect("changed conversation scope"));
    let wrong_scope = runtime.read_human_conversation_transcript_tail(&id, HumanTranscriptPageQuery {
        before_seq:Some(5),before_cursor:Some(cursor.clone()),limit:3,topic_id:None
    });
    assert!(owner.set_metadata("participantIds", original_participants).expect("restore exact scope"));
    assert!(wrong_scope.is_err(), "a token cannot be reinterpreted for changed participants");
    let snapshot_id = cursor.split(':').nth(1).expect("snapshot identity in cursor");
    let anchor_position = cursor.split(':').nth(2).expect("snapshot position in cursor")
        .parse::<usize>().expect("numeric snapshot position");
    let snapshot_key = format!("humanTranscriptSnapshot:v1:{snapshot_id}");
    let snapshot_raw = owner.read_kv(&snapshot_key).expect("read persisted snapshot")
        .expect("snapshot exists until eviction");
    let snapshot: serde_json::Value = serde_json::from_str(&snapshot_raw).expect("snapshot JSON");
    let unread_position = anchor_position + 1;
    let seqs = snapshot["seqs"].as_array().expect("physical sequence index");
    assert!(unread_position + 1 < seqs.len(), "fixture has multiple unread identities");
    let unread_seq = seqs[unread_position].as_i64().expect("physical integer identity");
    for (label, corrupt_seq) in [
        ("duplicate", seqs[unread_position + 1].clone()),
        ("string-alias", json!(unread_seq.to_string())),
        ("float-alias", json!(unread_seq as f64)),
        ("missing-seq", json!(999_999_i64)),
    ] {
        let mut corrupt_snapshot = snapshot.clone();
        corrupt_snapshot["seqs"][unread_position] = corrupt_seq;
        let corrupt_raw = corrupt_snapshot.to_string();
        assert!(owner.write_kv(&snapshot_key, &corrupt_raw).expect("persist corrupt unread index"));
        assert!(runtime.read_human_conversation_transcript_tail(&id, HumanTranscriptPageQuery {
            before_seq:Some(5),before_cursor:Some(cursor.clone()),limit:3,topic_id:None
        }).is_err(), "{label}: corrupt unread identities must not silently repeat or disappear");
        assert_eq!(owner.read_kv(&snapshot_key).expect("corrupt index is retained"), Some(corrupt_raw), "{label}");
        assert!(owner.write_kv(&snapshot_key, &snapshot_raw).expect("restore original snapshot"));
        let restored = runtime.read_human_conversation_transcript_tail(&id, HumanTranscriptPageQuery {
            before_seq:Some(5),before_cursor:Some(cursor.clone()),limit:3,topic_id:None
        }).expect("restored index resumes the same page");
        assert_eq!(restored.entries, second.entries, "{label}: recovery preserves every unread row exactly once");
        assert_eq!(restored.next_before_seq, second.next_before_seq, "{label}");
        assert_eq!(restored.next_before_cursor, second.next_before_cursor, "{label}");
    }
    runtime.shutdown();
    assert!(owner.get_human_transcript_entries().is_err(), "closed Human storage is not an empty history");
    assert!(owner.get_human_transcript_tail(HumanTranscriptPageQuery {
        before_seq:Some(5),before_cursor:Some(cursor.clone()),limit:3,topic_id:None
    }).is_err(), "closed Human storage cannot fabricate a terminal page");
    drop(owner);
    drop(runtime);
    let restarted = human_message_fixture_runtime(&root, "http://127.0.0.1:1");
    assert_eq!(restarted.read_human_conversation_transcript(&id).expect("reopened order"), all);
    let repeated = restarted.read_human_conversation_transcript_tail(&id, HumanTranscriptPageQuery {
        before_seq:Some(5),before_cursor:Some(cursor.clone()),limit:3,topic_id:None
    }).expect("replay exact persisted snapshot after restart");
    assert_eq!(repeated.entries, second.entries);
    let owner = restarted.open_human_conversation_db_owner(&id).expect("snapshot cache owner");
    let mut newest = None;
    for index in 0_u64..16 {
        let mut row = fixture_saved_human_message(1000 + index, "2", "new snapshot input");
        row["timestampMs"] = json!(6000.0 + index as f64);
        assert!(owner.append_transcript_entry(&row).expect("new stable set"));
        newest = Some(restarted.read_human_conversation_transcript_tail(&id, HumanTranscriptPageQuery {
            before_seq:None,before_cursor:None,limit:2,topic_id:None
        }).expect("new snapshot"));
    }
    assert!(restarted.read_human_conversation_transcript_tail(&id, HumanTranscriptPageQuery {
        before_seq:Some(5),before_cursor:Some(cursor),limit:3,topic_id:None
    }).is_err(), "the bounded cache rejects an expired token instead of silently restarting");
    let registry: serde_json::Value = serde_json::from_str(
        &owner.read_kv("humanTranscriptSnapshots:v1").expect("bounded registry")
            .expect("registry exists")
    ).expect("registry JSON");
    assert_eq!(registry["snapshots"].as_array().expect("snapshot identities").len(), 16);
    assert!(owner.read_kv(&snapshot_key).expect("evicted original index").is_none());
    let newest = newest.expect("latest snapshot");
    assert!(!restarted.read_human_conversation_transcript_tail(&id, HumanTranscriptPageQuery {
        before_seq:newest.next_before_seq,before_cursor:newest.next_before_cursor,limit:2,topic_id:None
    }).expect("most recent token survives eviction").entries.is_empty());
    restarted.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_snapshot_cursor_survives_pending_settlement_and_authoritative_anchor_refresh() {
    let root = temp_root("nmr-mutable-human-tail-anchor");
    let sent = fixture_remote_human_message(3, 1, 2, "pending anchor", Some("cursor-pending"), json!([]));
    let mut refreshed = Vec::new();
    for remote_id in [1_u64, 2, 5] {
        let mut row = fixture_remote_human_message(remote_id, 2, 1, &format!("remote-{remote_id}"), None, json!([]));
        row["createdAt"] = json!(format!("1970-01-01T00:00:{remote_id:02}Z"));
        refreshed.push(row);
    }
    refreshed.push(sent.clone());
    let mut new_anchor = fixture_remote_human_message(4, 2, 1, "remote-4", None, json!([]));
    new_anchor["createdAt"] = json!("2026-10-05T01:00:10Z");
    refreshed.push(new_anchor);
    refreshed.sort_by_key(|row| row["id"].as_u64().expect("positive service identity"));
    let (url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(201, json!({"success":true,"deduplicated":false,"message":sent})),
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{
            "messages":refreshed,"historyCursor":{"mode":"after-id","afterId":0},"nextAfterId":5
        }})),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("id").to_owned();
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    for remote_id in 1_u64..=5 {
        let row = if remote_id == 3 {
            json!({"id":"human-message:cursor-pending","kind":"message","role":"user",
                "authorKind":"human","authorId":"1","clientNonce":"cursor-pending",
                "content":"pending anchor","composedAtMs":3000.0,"timestampMs":3000.0,"delivery":"pending"})
        } else {
            let mut row = fixture_saved_human_message(remote_id, "2", &format!("remote-{remote_id}"));
            row["timestampMs"] = json!(remote_id as f64 * 1000.0);
            row["remoteCreatedAt"] = json!(format!("1970-01-01T00:00:{remote_id:02}Z"));
            row
        };
        assert!(owner.append_transcript_entry(&row).expect("original chronological set"));
    }
    let first = dispatch(&runtime, "getHumanConversationTranscriptTail", json!({"conversationId":id,"limit":2}));
    assert_eq!(first["entries"].as_array().expect("first page").iter()
        .map(|row| row["remoteMessageId"].as_str().unwrap()).collect::<Vec<_>>(), vec!["4","5"]);
    assert_eq!(first["nextBeforeSeq"], 4);
    assert!(first["nextBeforeCursor"].as_str().is_some());
    let settled = runtime.append_human_message(&id, "pending anchor", "cursor-pending", Some(3000.0), None, &[])
        .expect("real native settlement moves the pending row's presentation timestamp");
    assert_eq!(settled["id"], "human-server-message:3");
    let fresh = runtime.sync_human_conversation(&id).expect("authoritative refresh moves the old page anchor's timestamp");
    assert_eq!(fresh.iter().map(|row| row["remoteMessageId"].as_str().unwrap()).collect::<Vec<_>>(), vec!["1","2","5","3","4"]);
    let continuation_args = json!({"conversationId":id,"limit":3,
        "beforeSeq":first["nextBeforeSeq"],"beforeCursor":first["nextBeforeCursor"]});
    let older = dispatch(&runtime, "getHumanConversationTranscriptTail", continuation_args.clone());
    assert_eq!(older["entries"].as_array().expect("stable older page").iter()
        .map(|row| row["remoteMessageId"].as_str().unwrap()).collect::<Vec<_>>(), vec!["1","2","3"],
        "a moved timestamp cannot repeat already-returned 4/5 or lose the now-settled former pending row");
    assert_eq!(older["entries"][2], settled);
    assert_eq!(older["nextBeforeCursor"], serde_json::Value::Null);
    assert_eq!(older["nextBeforeSeq"], serde_json::Value::Null);
    assert_eq!(owner.get_transcript_entries().expect("physical identities retained").iter()
        .map(|row| row["remoteMessageId"].as_str().unwrap()).collect::<Vec<_>>(), vec!["1","2","3","4","5"]);
    runtime.shutdown();
    drop(owner);
    drop(runtime);
    let restarted = human_message_fixture_runtime(&root, &url);
    assert_eq!(dispatch(&restarted, "getHumanConversationTranscriptTail", continuation_args), older,
        "stable continuation and refreshed payload survive restart");
    restarted.shutdown();
    let requests = server.join().expect("settlement and refresh fixture");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("POST /api/social/messages "));
    assert!(requests[1].starts_with("GET /api/social/messages?contactId=2&limit=200&afterId=0 "));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_get_conversation_transcript_tail_topic_support() {
    let root = temp_root("transcript-tail-topic-support");
    let runtime = human_message_fixture_runtime(&root, "http://127.0.0.1:1");
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("id").to_owned();
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");

    let mut msg_main = fixture_saved_human_message(1, "2", "main message");
    msg_main["timestampMs"] = json!(1000.0);
    assert!(owner.append_transcript_entry(&msg_main).expect("append main"));

    let mut msg_topic = fixture_saved_human_message(2, "2", "topic message");
    msg_topic["timestampMs"] = json!(2000.0);
    msg_topic["topicId"] = json!("topic-abc");
    assert!(owner.append_transcript_entry(&msg_topic).expect("append topic"));

    // Tail without topicId -> returns only msg_main
    let main_tail = dispatch(&runtime, "getHumanConversationTranscriptTail", json!({
        "conversationId": id,
        "limit": 10
    }));
    let main_entries = main_tail["entries"].as_array().expect("main entries");
    assert_eq!(main_entries.len(), 1);
    assert_eq!(main_entries[0]["remoteMessageId"], "1");

    // Tail with topicId -> returns only msg_topic
    let topic_tail = dispatch(&runtime, "getHumanConversationTranscriptTail", json!({
        "conversationId": id,
        "topicId": "topic-abc",
        "limit": 10
    }));
    let topic_entries = topic_tail["entries"].as_array().expect("topic entries");
    assert_eq!(topic_entries.len(), 1);
    assert_eq!(topic_entries[0]["remoteMessageId"], "2");

    // Tail with empty topicId -> rejected with bad request
    let err_empty = dispatch_production_session_gateway_call(
        &runtime,
        "getHumanConversationTranscriptTail",
        &json!({
            "conversationId": id,
            "topicId": "",
            "limit": 10
        }),
    ).expect("handled");
    assert!(err_empty.is_err(), "empty topicId must be rejected");

    // Tail with whitespace topicId -> rejected with bad request
    let err_ws = dispatch_production_session_gateway_call(
        &runtime,
        "getHumanConversationTranscriptTail",
        &json!({
            "conversationId": id,
            "topicId": "   ",
            "limit": 10
        }),
    ).expect("handled");
    assert!(err_ws.is_err(), "whitespace topicId must be rejected");

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_agent_handoff_and_history_download_keep_activity_monotonic() {
    let root = temp_root("nmr-agent-handoff-history-activity");
    let remote = fixture_remote_human_message(71, 2, 1, "older resource message", None, json!([{
        "resourceId":"handoff-race-resource","name":"history.txt","contentType":"text/plain",
        "size":4,"createdAt":"2026-10-05T01:00:00Z"
    }]));
    let (resource_seen, resource_wait) = std::sync::mpsc::channel();
    let (resource_release, resource_resume) = std::sync::mpsc::channel();
    let (url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{
            "messages":[remote],"historyCursor":{"mode":"after-id","afterId":0},"nextAfterId":71
        }})),
        HumanMessageFixtureReply::Deferred(
            Box::new(HumanMessageFixtureReply::Bytes(200, vec![1,2,3,4])),
            resource_seen, resource_resume,
        ),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("id").to_owned();
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    assert!(owner.set_metadata("lastActivityAt", json!(1000.0)).expect("historic activity baseline"));
    let worker = runtime.clone();
    let conversation = id.clone();
    let sync = thread::spawn(move || worker.sync_human_conversation(&conversation));
    resource_wait.recv_timeout(std::time::Duration::from_secs(10))
        .expect("materializer is waiting for resource bytes");
    let worker = runtime.clone();
    let conversation = id.clone();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (completed_tx, completed_rx) = std::sync::mpsc::channel();
    let handoff = thread::spawn(move || {
        started_tx.send(()).expect("Agent handoff started");
        let result = worker.append_human_agent_message(
            &conversation, "fixture-agent", "history-race-stream", None, "new Agent result", 1900000000000.0,
        );
        completed_tx.send(result).expect("Agent handoff result");
    });
    started_rx.recv_timeout(std::time::Duration::from_secs(10)).expect("Agent handoff caller entered");
    let handoff_waited = matches!(completed_rx.recv_timeout(std::time::Duration::from_millis(200)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout));
    // Release the held HTTP body before asserting the concurrency result, so
    // an implementation regression cannot leave either worker stuck in CI.
    resource_release.send(()).expect("release delayed resource");
    let sync_result = sync.join().expect("history worker");
    let handoff_result = if handoff_waited {
        Some(completed_rx.recv_timeout(std::time::Duration::from_secs(10)).expect("handoff after history"))
    } else {
        None
    };
    handoff.join().expect("Agent handoff worker");
    sync_result.expect("history finishes");
    assert!(handoff_waited,
        "Agent handoff uses the same conversation mutation boundary while history materializes");
    let agent_entry = handoff_result.expect("handoff observed after release").expect("Agent handoff succeeds");
    assert_eq!(agent_entry["authorKind"], "agent");
    assert_eq!(owner.get_metadata("lastActivityAt").expect("new activity"), Some(json!(1900000000000.0)));
    runtime.append_human_agent_message(&id, "fixture-agent", "older-completion-stream", None, "late older Agent result", 1800000000000.0)
        .expect("older completed stream still materializes");
    assert_eq!(owner.get_metadata("lastActivityAt").expect("monotonic older completion"), Some(json!(1900000000000.0)));
    let rows = runtime.read_human_conversation_transcript(&id).expect("Human and Agent rows share owner");
    assert_eq!(rows.len(), 3);
    for timestamp in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(runtime.append_human_agent_message(&id, "fixture-agent", "invalid-clock", None, "invalid clock", timestamp).is_err(),
            "non-finite activity must fail before persistence");
    }
    assert_eq!(runtime.read_human_conversation_transcript(&id).expect("unchanged after invalid clocks"), rows);
    let saved_remote = rows.iter().find(|row| row["remoteMessageId"] == "71").expect("downloaded remote row");
    assert_eq!(fs::read(saved_remote["attachments"][0]["path"].as_str().expect("download path"))
        .expect("materialized bytes"), vec![1,2,3,4]);
    runtime.shutdown(); drop(owner); drop(runtime);
    let restarted = human_message_fixture_runtime(&root, &url);
    assert_eq!(restarted.read_human_conversation_transcript(&id).expect("restart shared transcript"), rows);
    let owner = restarted.open_human_conversation_db_owner(&id).expect("reopened owner");
    assert_eq!(owner.get_metadata("lastActivityAt").expect("reopened activity"), Some(json!(1900000000000.0)));
    assert_eq!(restarted.append_human_agent_message(&id, "fixture-agent", "history-race-stream", None, "new Agent result", 0.0)
        .expect("idempotent Agent replay retains original completion"), agent_entry);
    assert_eq!(owner.get_metadata("lastActivityAt").expect("replay cannot regress activity"), Some(json!(1900000000000.0)));
    restarted.shutdown(); drop(owner); drop(restarted);
    let requests = server.join().expect("history and resource fixture");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /api/social/messages?contactId=2&limit=200&afterId=0 "));
    assert!(requests[1].starts_with("GET /api/social/message-resources/handoff-race-resource "));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_activity_write_failure_keeps_committed_rows_and_repairs_after_restart() {
    let root = temp_root("nmr-activity-metadata-recovery");
    let remote = fixture_remote_human_message(78, 2, 1, "durable before activity", None, json!([]));
    let page = json!({"success":true,"data":{
        "messages":[remote],"historyCursor":{"mode":"after-id","afterId":0},"nextAfterId":78
    }});
    let (url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(200, page.clone()),
        HumanMessageFixtureReply::Json(200, page),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("id").to_owned();
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    assert_eq!(owner.get_metadata("lastActivityAt").expect("initial activity"), Some(json!(0.0)));
    let db = rusqlite::Connection::open(forward_db_path(&root, &id)).expect("independent SQLite fault injector");
    db.busy_timeout(std::time::Duration::from_secs(5)).expect("bounded SQLite fixture");
    let reject_activity = "CREATE TRIGGER reject_human_activity BEFORE UPDATE OF value ON kv
        WHEN NEW.key = 'metadata' BEGIN SELECT RAISE(ABORT, 'NMR-ACTIVITY injected persistence failure'); END;";
    db.execute_batch(reject_activity).expect("reject real metadata persistence");
    let error = runtime.sync_human_conversation(&id).expect_err("history cannot acknowledge failed activity persistence");
    assert!(error.contains("NMR-ACTIVITY injected persistence failure"), "{error}");
    let committed = owner.get_transcript_entries().expect("row commit precedes failed metadata commit");
    assert_eq!(committed.len(), 1);
    assert_eq!(committed[0]["remoteMessageId"], "78");
    assert_eq!(owner.get_metadata("lastActivityAt").expect("failed activity retained"), Some(json!(0.0)));
    assert!(owner.get_metadata("humanHistoryCoverage").expect("no premature coverage checkpoint").is_none());
    db.execute_batch("DROP TRIGGER reject_human_activity;").expect("remove precise injected fault");
    drop(db); runtime.shutdown(); drop(owner); drop(runtime);

    let restarted = human_message_fixture_runtime(&root, &url);
    assert_eq!(restarted.read_human_conversation_transcript(&id).expect("row survived pure reopen"), committed);
    assert_eq!(restarted.sync_human_conversation(&id).expect("known-row replay repairs activity"), committed);
    let owner = restarted.open_human_conversation_db_owner(&id).expect("reopened owner");
    let remote_timestamp = committed[0]["timestampMs"].clone();
    assert_eq!(owner.get_metadata("lastActivityAt").expect("repaired native activity"), Some(remote_timestamp.clone()));
    assert_eq!(owner.get_metadata("humanHistoryCoverage").expect("checkpoint only after repair"),
        Some(json!({"version":1,"afterId":78,"complete":true})));
    let db = rusqlite::Connection::open(forward_db_path(&root, &id)).expect("Agent activity fault injector");
    db.busy_timeout(std::time::Duration::from_secs(5)).expect("bounded second SQLite fixture");
    db.execute_batch(reject_activity).expect("reject Agent activity persistence");
    let error = restarted.append_human_agent_message(
        &id, "fixture-agent", "durable-agent-stream", None, "durable Agent result", 1900000000000.0,
    ).expect_err("Agent handoff must report its failed metadata commit");
    assert!(error.contains("NMR-ACTIVITY injected persistence failure"), "{error}");
    let both = owner.get_transcript_entries().expect("both durable rows");
    assert_eq!(both.len(), 2);
    assert_eq!(both[0], committed[0]);
    assert_eq!(both[1]["id"], "human-agent-message:durable-agent-stream");
    assert_eq!(owner.get_metadata("lastActivityAt").expect("Agent metadata write failed"), Some(remote_timestamp));
    db.execute_batch("DROP TRIGGER reject_human_activity;").expect("restore metadata writes");
    drop(db); restarted.shutdown(); drop(owner); drop(restarted);

    let recovered = human_message_fixture_runtime(&root, &url);
    assert_eq!(recovered.append_human_agent_message(
        &id, "fixture-agent", "durable-agent-stream", None, "durable Agent result", 1900000001000.0,
    ).expect("idempotent Agent replay repairs the interrupted metadata write"), both[1]);
    assert_eq!(recovered.read_human_conversation_transcript(&id).expect("no duplicated replay row"), both);
    let owner = recovered.open_human_conversation_db_owner(&id).expect("recovered owner");
    assert_eq!(owner.get_metadata("lastActivityAt").expect("original committed completion time repaired"),
        Some(json!(1900000000000.0)));
    recovered.shutdown(); drop(owner); drop(recovered);
    let requests = server.join().expect("two actual history attempts");
    assert_eq!(requests.len(), 2, "Agent replay cannot dispatch another HTTP message");
    assert!(requests.iter().all(|request| request.starts_with(
        "GET /api/social/messages?contactId=2&limit=200&afterId=0 ")));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_forward_pending_ownership_survives_missing_versions_and_rejects_unreadable_sources() {
    let root = temp_root("forward-pending-ownership");
    let mut server = ForwardHttpFixture::start(true);
    let args = prepare_forward_process(&root, &server.url);
    let mut child = ForwardProcess::start(&root, &server.url);
    server.wait_for_first_post(); child.terminate(); server.release_first_post();
    let runtime = human_message_fixture_runtime(&root, &server.url);
    let source = args["sourceConversationId"].as_str().unwrap();
    let first = args["destinationConversationIds"][0].as_str().unwrap();
    let source_owner = runtime.open_human_conversation_db_owner(source).unwrap();
    let target_owner = runtime.open_human_conversation_db_owner(first).unwrap();
    let original = target_owner.get_transcript_entries().unwrap().remove(0);
    assert_eq!(original["delivery"], "dispatching");
    assert_eq!(original["forwardBatchOwned"], true, "ownership must be durable before the first POST");
    let original_id = original["id"].as_str().unwrap();
    let admitted = source_owner.get_metadata("humanForwardBatches").unwrap().unwrap();
    assert!(source_owner.set_metadata("participantIds", json!(["2", "9"])).unwrap());
    // The current source scan includes revoked sources and persists their
    // policy denial. Establish that legitimate journal transition first;
    // the damaged destination markers must not mutate this denied intent.
    let denied = forward_gateway_result(&runtime, &args).expect("persist revoked-source policy denial");
    assert!(denied["destinations"].as_array().unwrap().iter().all(|target|
        target["status"] == "failed" && target["error"].as_str().unwrap().contains("participant")));
    let journal = source_owner.get_metadata("humanForwardBatches").unwrap().unwrap();
    for field in ["version", "accountId", "issuer", "requestId", "sourceConversationId",
        "sourceEntryId", "text", "attachments", "context"] {
        assert_eq!(journal[0][field], admitted[0][field], "denial retains immutable {field}");
    }
    for (before, after) in admitted[0]["destinations"].as_array().unwrap().iter()
        .zip(journal[0]["destinations"].as_array().unwrap()) {
        assert_eq!(after["conversationId"], before["conversationId"]);
        assert_eq!(after["clientNonce"], before["clientNonce"]);
        assert_eq!(after["completed"], false);
        assert!(after["error"].as_str().unwrap().contains("participant"));
    }
    assert_eq!(server.post_count(), 1);
    for case in ["future-version", "missing-version", "changed-request", "marker-only",
        "missing-context", "unrelated-context"] {
        let mut pending = original.clone();
        if case == "marker-only" {
            pending.as_object_mut().unwrap().remove("forwardContext");
        } else {
            // Model persisted v1 rows written before the extra ownership marker
            // existed, so the journal lookup must stand on its own.
            pending.as_object_mut().unwrap().remove("forwardBatchOwned");
            if case == "missing-context" {
                pending.as_object_mut().unwrap().remove("forwardContext");
            } else if case == "unrelated-context" {
                pending["forwardContext"] = json!({"preserve":"unrelated retained metadata"});
            } else {
                let context = pending["forwardContext"].as_object_mut().unwrap();
                if case == "future-version" {
                    context.insert("batchVersion".into(), json!(2));
                } else {
                    context.remove("batchVersion");
                    if case == "changed-request" {
                        context.insert("requestId".into(), json!("corrupted-request-marker"));
                    }
                }
            }
        }
        target_owner.update_transcript_entry(original_id, &pending).unwrap().unwrap();
        runtime.sync_human_conversation(first).expect("owned pending row cannot starve independent history");
        assert_eq!(server.post_count(), 1, "{case}: source ownership cannot be bypassed by generic replay");
        let rows = target_owner.get_transcript_entries().unwrap();
        assert_eq!(rows.len(), 1, "{case}");
        let mut preserved = rows[0].clone();
        // Adoption may add an ownership marker, but cannot alter the original
        // payload, nonce, context, or pending settlement state.
        preserved.as_object_mut().unwrap().remove("forwardBatchOwned");
        pending.as_object_mut().unwrap().remove("forwardBatchOwned");
        assert_eq!(preserved, pending, "{case}: retain the original pending intent");
        assert_eq!(source_owner.get_metadata("humanForwardBatches").unwrap(), Some(journal.clone()));
    }
    assert_eq!(server.history_count(), 6);
    let mut markerless = original.clone();
    markerless.as_object_mut().unwrap().remove("forwardBatchOwned");
    markerless["forwardContext"].as_object_mut().unwrap().remove("batchVersion");
    target_owner.update_transcript_entry(original_id, &markerless).unwrap().unwrap();
    assert!(source_owner.set_metadata("humanForwardBatches", json!({"invalid":"not a journal array"})).unwrap());
    assert!(runtime.sync_human_conversation(first).is_err(), "malformed source journal is not proof of legacy ownership");
    assert_eq!(server.post_count(), 1);
    assert_eq!(target_owner.get_transcript_entries().unwrap(), vec![markerless.clone()]);
    assert!(source_owner.set_metadata("humanForwardBatches", journal.clone()).unwrap());
    let source_path = forward_db_path(&root, source);
    let unavailable_path = source_path.with_extension("temporarily-unavailable");
    for case in ["source-context", "missing-context", "unrelated-context"] {
        let mut pending = markerless.clone();
        if case == "missing-context" {
            pending.as_object_mut().unwrap().remove("forwardContext");
        } else if case == "unrelated-context" {
            pending["forwardContext"] = json!({"preserve":"unrelated retained metadata"});
        }
        target_owner.update_transcript_entry(original_id, &pending).unwrap().unwrap();
        // The directory scan may skip an absent SQLite file. Neither a lost
        // source pointer nor unrelated metadata establishes legacy ownership
        // of this pre-marker forwarding nonce when its source is unreadable.
        fs::rename(&source_path, &unavailable_path).expect("make the exact source store unavailable");
        let unavailable = runtime.sync_human_conversation(first);
        fs::rename(&unavailable_path, &source_path).expect("restore owned source store");
        assert!(unavailable.is_err(), "{case}: unreadable source cannot authorize markerless generic replay");
        assert_eq!(server.post_count(), 1, "{case}: preserve the single previously accepted POST");
        assert_eq!(target_owner.get_transcript_entries().unwrap(), vec![pending],
            "{case}: failed source lookup cannot mutate the retained pending intent");
        assert_eq!(source_owner.get_metadata("humanForwardBatches").unwrap(), Some(journal.clone()),
            "{case}: the exact denied source journal remains recoverable");
    }
    runtime.shutdown(); drop(target_owner); drop(source_owner); drop(runtime); drop(child);
    server.finish(); fs::remove_dir_all(root).expect("remove ownership negative fixture");

    // Genuine pre-journal intent uses a readable source with no source-owned
    // batch. Preserve this compatibility path without manufacturing a missing
    // journal for the v1 batch exercised above.
    let legacy_root = temp_root("forward-genuine-legacy");
    let legacy_server = ForwardHttpFixture::start(false);
    let legacy_args = prepare_forward_process(&legacy_root, &legacy_server.url);
    let legacy_runtime = human_message_fixture_runtime(&legacy_root, &legacy_server.url);
    let legacy_source = legacy_args["sourceConversationId"].as_str().unwrap();
    let legacy_target = legacy_args["destinationConversationIds"][0].as_str().unwrap();
    let legacy_source_entry_id = legacy_args["sourceEntryId"].as_str().unwrap();
    let legacy_owner = legacy_runtime.open_human_conversation_db_owner(legacy_source).unwrap();
    let legacy_target_owner = legacy_runtime.open_human_conversation_db_owner(legacy_target).unwrap();
    assert!(legacy_owner.get_metadata("humanForwardBatches").unwrap().is_none());
    let legacy_canonical_metadata = legacy_owner.read_kv("metadata").unwrap().unwrap();
    // Reproduce the actual pre-journal production nonce: SHA256 of the
    // request ID, one NUL byte, then this destination conversation ID.
    use sha2::Digest;
    let legacy_request_id = "pre-journal-request";
    let mut legacy_digest = sha2::Sha256::new();
    legacy_digest.update(legacy_request_id.as_bytes());
    legacy_digest.update([0u8]);
    legacy_digest.update(legacy_target.as_bytes());
    let legacy_nonce = format!("human-forward-{:x}", legacy_digest.finalize());
    let context = json!({
        "requestId":legacy_request_id,"sourceConversationId":legacy_source,
        "sourceEntryId":legacy_args["sourceEntryId"],
        "privacy":{"dropSenderNames":false,"dropCaptions":false}
    });
    let legacy_pending = json!({
        "id":format!("human-message:{legacy_nonce}"),"kind":"message","role":"user",
        "authorKind":"human","authorId":"1","content":"legacy forward content",
        "clientNonce":legacy_nonce.as_str(),"timestampMs":1000.0,
        "delivery":"pending","forwardContext":context.clone()
    });
    assert!(legacy_target_owner.append_transcript_entry(&legacy_pending).unwrap());
    assert!(legacy_owner.set_metadata("participantIds", json!(["2", "9"])).unwrap());
    // Genuine legacy intent has no batch journal to carry a policy denial.
    // Persist both the old outbox and revoked membership, then use fresh owners
    // to prove restart cannot turn that readable source into permission to send.
    legacy_runtime.shutdown(); drop(legacy_target_owner); drop(legacy_owner); drop(legacy_runtime);
    let legacy_runtime = human_message_fixture_runtime(&legacy_root, &legacy_server.url);
    assert!(legacy_runtime.open_human_conversation_db_owner(legacy_source).is_err(),
        "ordinary cold admission refuses the revoked legacy source");
    let legacy_target_owner = legacy_runtime.open_human_conversation_db_owner(legacy_target).unwrap();
    assert_eq!(legacy_target_owner.get_transcript_entries().unwrap(), vec![legacy_pending.clone()]);
    let legacy_revoked_metadata = read_forward_metadata_from_disk(&legacy_root, legacy_source);
    assert_eq!(legacy_revoked_metadata["participantIds"], json!(["2", "9"]));
    let denied = legacy_runtime.sync_human_conversation(legacy_target)
        .expect_err("reopened genuine legacy intent must reauthorize source membership");
    assert!(denied.contains("participant"), "{denied}");
    assert_eq!(legacy_server.post_count(), 0);
    assert_eq!(legacy_target_owner.get_transcript_entries().unwrap(), vec![legacy_pending.clone()]);
    assert!(legacy_revoked_metadata.get("humanForwardBatches").is_none());
    assert_eq!(read_forward_metadata_from_disk(&legacy_root, legacy_source), legacy_revoked_metadata,
        "legacy refusal preserves the complete revoked source metadata");
    assert!(legacy_runtime.open_human_conversation_db_owner(legacy_source).is_err(),
        "retained journal inspection never grants ordinary source authority");

    // Only after the refusal oracles, explicitly restore this test fixture's
    // original metadata. Production recovery does not repair participant pairs.
    let repair = rusqlite::Connection::open(forward_db_path(&legacy_root, legacy_source)).unwrap();
    assert_eq!(repair.execute("UPDATE kv SET value = ?1 WHERE key = 'metadata'",
        [legacy_canonical_metadata.as_str()]).unwrap(), 1);
    drop(repair);
    let legacy_owner = legacy_runtime.open_human_conversation_db_owner(legacy_source)
        .expect("ordinary source authority returns only after explicit pair restoration");
    assert_eq!(legacy_owner.read_kv("metadata").unwrap().as_deref(), Some(legacy_canonical_metadata.as_str()));
    let mut changed_source = legacy_owner.get_transcript_entries().unwrap().into_iter()
        .find(|entry| entry["id"].as_str() == Some(legacy_source_entry_id)).unwrap();
    changed_source["content"] = json!("source edited after the legacy intent was persisted");
    changed_source["protectedContent"] = json!(true);
    legacy_owner.update_transcript_entry(legacy_source_entry_id, &changed_source).unwrap().unwrap();
    let denied = legacy_runtime.sync_human_conversation(legacy_target)
        .expect_err("genuine legacy intent must reauthorize the current source protection");
    assert!(denied.contains("protected"), "{denied}");
    assert_eq!(legacy_server.post_count(), 0);
    assert_eq!(legacy_target_owner.get_transcript_entries().unwrap(), vec![legacy_pending.clone()]);
    assert!(legacy_owner.get_metadata("humanForwardBatches").unwrap().is_none());

    changed_source.as_object_mut().unwrap().remove("protectedContent");
    legacy_owner.update_transcript_entry(legacy_source_entry_id, &changed_source).unwrap().unwrap();
    let legacy_rows = legacy_runtime.sync_human_conversation(legacy_target)
        .expect("restored source permission resumes the same genuine legacy intent");
    assert_eq!(legacy_server.post_count(), 1);
    assert_eq!(legacy_rows.len(), 1);
    assert_eq!(legacy_rows[0]["delivery"], "sent");
    assert_eq!(legacy_rows[0]["clientNonce"].as_str(), Some(legacy_nonce.as_str()));
    assert_eq!(legacy_rows[0]["forwardContext"], context);
    assert_eq!(legacy_rows[0]["content"], legacy_pending["content"]);
    assert_ne!(legacy_rows[0]["content"], changed_source["content"]);
    let submitted = {
        let state = legacy_server.state.lock().expect("legacy submitted snapshot");
        fixture_request_json(state.requests.iter()
            .find(|request| request.starts_with("POST /api/social/messages ")).unwrap())
    };
    assert_eq!(submitted["text"], legacy_pending["content"], "permission restoration cannot replace the saved payload");
    assert!(legacy_owner.get_metadata("humanForwardBatches").unwrap().is_none());
    legacy_runtime.shutdown(); drop(legacy_target_owner); drop(legacy_owner); drop(legacy_runtime);
    legacy_server.finish(); fs::remove_dir_all(legacy_root).expect("remove genuine legacy fixture");
}

fn human_history_message(id: u64) -> serde_json::Value {
    let mut row = fixture_remote_human_message(id, 2, 1, &format!("history-{id}"), None, json!([]));
    row["createdAt"] = json!("1970-01-01T00:00:01Z");
    row
}

fn human_history_reply(rows: Vec<serde_json::Value>) -> HumanMessageFixtureReply {
    HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":rows}}))
}

fn human_local_history_message(id: u64) -> serde_json::Value {
    json!({
        "id":format!("human-server-message:{id}"),"kind":"message","role":"user",
        "authorKind":"human","authorId":"2","authorName":"peer",
        "content":format!("history-{id}"),"timestampMs":1000.0,"delivery":"sent",
        "remoteMessageId":id.to_string(),"remoteCreatedAt":"1970-01-01T00:00:01Z",
        "remoteReactions":[],"reactions":[]
    })
}

fn human_row_ids(rows: &[serde_json::Value]) -> Vec<String> {
    rows.iter().map(|row| row["id"].as_str().expect("canonical row id").to_owned()).collect()
}

fn record_human_target_events(
    conversation_id: &str,
    entry_id: &str,
) -> (
    Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    mahayana_host_runtime::transcript_mutation_events::TranscriptMutationSubscription,
) {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = Arc::clone(&events);
    let conversation_id = conversation_id.to_owned();
    let entry_id = entry_id.to_owned();
    let subscription = mahayana_host_runtime::transcript_mutation_events::subscribe_transcript_mutations(
        move |mutation| {
            if mutation.get("agentId").and_then(serde_json::Value::as_str) != Some(conversation_id.as_str()) {
                return;
            }
            let matches = mutation.get("entryId").and_then(serde_json::Value::as_str) == Some(entry_id.as_str())
                || mutation.get("entries").and_then(serde_json::Value::as_array)
                    .is_some_and(|rows| rows.iter().any(|row| row["id"].as_str() == Some(entry_id.as_str())));
            if matches {
                captured.lock().expect("capture target mutation").push(serde_json::Value::Object(mutation.clone()));
            }
        },
    );
    (events, subscription)
}

fn spawn_human_no_request_fixture() -> (
    String,
    std::sync::mpsc::Sender<()>,
    thread::JoinHandle<Vec<String>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind active admission fixture");
    listener.set_nonblocking(true).expect("nonblocking admission fixture");
    let url = format!("http://{}", listener.local_addr().expect("admission fixture address"));
    let (stop, stopped) = std::sync::mpsc::channel();
    let (ready, active) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let mut requests = Vec::new();
        ready.send(()).expect("report live admission listener");
        loop {
            assert!(std::time::Instant::now() < deadline, "admission fixture was not stopped");
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).expect("blocking unexpected request");
                    stream.set_write_timeout(Some(std::time::Duration::from_secs(15)))
                        .expect("bound unexpected response");
                    requests.push(read_fixture_http_request_before(&mut stream, Some(deadline)));
                    let body = json!({"success":false,"error":"unexpected HTTP during Human reaction admission"}).to_string();
                    write!(stream,
                        "HTTP/1.1 503 Unexpected\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(), body,
                    ).expect("answer unexpected admission request");
                    stream.flush().expect("flush unexpected response");
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    match stopped.recv_timeout(std::time::Duration::from_millis(5)) {
                        Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
                Err(error) => panic!("admission fixture accept: {error}"),
            }
        }
        requests
    });
    active.recv_timeout(std::time::Duration::from_secs(15)).expect("listener is active before admission");
    (url, stop, server)
}

#[test]
fn shipping_human_short_32nd_page_defers_latest_refresh_to_next_sync() {
    const BASE: u64 = 1_000_000;
    for (last_id, expected_len) in [(BASE + 6200, 6200_usize), (BASE + 6201, 6201_usize)] {
        let root = temp_root(&format!("human-short-32nd-page-{last_id}"));
        let mut replies = (0..31_u64).map(|page| human_history_reply(
            (BASE + page * 200 + 1..=BASE + (page + 1) * 200).map(human_history_message).collect(),
        )).collect::<Vec<_>>();
        let final_page = if last_id == BASE + 6200 { vec![] } else { vec![human_history_message(last_id)] };
        replies.push(human_history_reply(final_page));
        let (url, server) = spawn_human_message_fixture(replies);
        let runtime = human_message_fixture_runtime(&root, &url);
        let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
            .as_str().expect("id").to_owned();
        let rows = runtime.sync_human_conversation(&id).expect("short or empty final page stays within budget");
        assert_eq!(rows.len(), expected_len);
        for (index, row) in rows.iter().enumerate() {
            assert_eq!(row, &human_local_history_message(BASE + index as u64 + 1));
        }
        let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
        assert_eq!(owner.get_metadata("humanHistoryCoverage").expect("coverage"),
            Some(json!({"version":1,"afterId":last_id,"complete":true})));
        let requests = server.join().expect("no 33rd refresh");
        assert_eq!(requests.len(), 32);
        for (index, request) in requests.iter().enumerate() {
            let after = if index == 0 { 0 } else { BASE + index as u64 * 200 };
            assert!(request.starts_with(&format!("GET /api/social/messages?contactId=2&limit=200&afterId={after} ")));
        }
        runtime.shutdown(); drop(owner); drop(runtime);

        let mut updated = human_history_message(last_id);
        updated["reactions"] = json!([{"emoji":"👍","count":1,"reactedByMe":false}]);
        let (url, server) = spawn_human_message_fixture(vec![
            human_history_reply(vec![]), human_history_reply(vec![updated]),
        ]);
        let restarted = human_message_fixture_runtime(&root, &url);
        let refreshed = restarted.sync_human_conversation(&id).expect("deferred latest snapshot");
        assert_eq!(human_row_ids(&refreshed), human_row_ids(&rows));
        let last_index = expected_len - 1;
        assert_eq!(&refreshed[..last_index], &rows[..last_index]);
        let mut expected_last = rows[last_index].clone();
        expected_last["remoteReactions"] = json!([{"emoji":"👍","count":1,"reactedByMe":false}]);
        expected_last["reactions"] = json!([{"emoji":"👍","by":"2"}]);
        assert_eq!(refreshed[last_index], expected_last);
        let owner = restarted.open_human_conversation_db_owner(&id).expect("reopened owner");
        assert_eq!(owner.get_metadata("humanHistoryCoverage").expect("retained completion"),
            Some(json!({"version":1,"afterId":last_id,"complete":true})));
        let requests = server.join().expect("deferred refresh fixture");
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with(&format!("GET /api/social/messages?contactId=2&limit=200&afterId={last_id} ")));
        assert!(requests[1].starts_with("GET /api/social/messages?contactId=2&limit=200 "));
        restarted.shutdown(); drop(owner); drop(restarted);
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn shipping_human_midpage_resource_failure_replays_durable_prefix_after_restart() {
    let root = temp_root("human-midpage-prefix-restart");
    let mut resource_row = human_history_message(2_000_202);
    resource_row["attachments"] = json!([{
        "resourceId":"midpage-retry-resource","name":"retry.bin","contentType":"application/octet-stream",
        "size":4,"createdAt":"1970-01-01T00:00:01Z"
    }]);
    let interrupted_page = vec![human_history_message(2_000_201), resource_row, human_history_message(2_000_203)];
    let (url, server) = spawn_human_message_fixture(vec![
        human_history_reply((2_000_001..=2_000_200).map(human_history_message).collect()),
        human_history_reply(interrupted_page.clone()),
        HumanMessageFixtureReply::Json(503, json!({"success":false,"error":"midpage resource disconnect"})),
        human_history_reply(interrupted_page.clone()),
        HumanMessageFixtureReply::Bytes(200, vec![21, 22, 23, 24]),
        human_history_reply(interrupted_page),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("id").to_owned();
    assert!(runtime.sync_human_conversation(&id).expect_err("resource failure leaves a gap").contains("resource"));
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    let prefix = owner.get_transcript_entries().expect("durable page plus prefix");
    assert_eq!(prefix.len(), 201);
    for (index, row) in prefix.iter().enumerate() {
        assert_eq!(row, &human_local_history_message(2_000_001 + index as u64));
    }
    assert_eq!(owner.get_metadata("humanHistoryCoverage").expect("prior complete page only"),
        Some(json!({"version":1,"afterId":2_000_200,"complete":false})));
    runtime.shutdown(); drop(owner); drop(runtime);

    let restarted = human_message_fixture_runtime(&root, &url);
    let rows = restarted.sync_human_conversation(&id).expect("resume partial page");
    assert_eq!(&rows[..201], prefix.as_slice());
    assert_eq!(rows.len(), 203);
    assert_eq!(rows[202], human_local_history_message(2_000_203));
    assert_eq!(rows[201]["remoteMessageId"], "2000202");
    assert_eq!(fs::read(rows[201]["attachments"][0]["path"].as_str().expect("download path"))
        .expect("resource bytes"), vec![21, 22, 23, 24]);
    let owner = restarted.open_human_conversation_db_owner(&id).expect("owner");
    assert_eq!(owner.get_metadata("humanHistoryCoverage").expect("full page committed"),
        Some(json!({"version":1,"afterId":2_000_203,"complete":true})));
    let requests = server.join().expect("midpage fixture");
    assert_eq!(requests.len(), 6, "prefix replay and latest refresh reuse the committed resource");
    assert!(requests[0].starts_with("GET /api/social/messages?contactId=2&limit=200&afterId=0 "));
    assert!(requests[1].starts_with("GET /api/social/messages?contactId=2&limit=200&afterId=2000200 "));
    assert!(requests[2].starts_with("GET /api/social/message-resources/midpage-retry-resource "));
    assert!(requests[3].starts_with("GET /api/social/messages?contactId=2&limit=200&afterId=2000200 "));
    assert!(requests[4].starts_with("GET /api/social/message-resources/midpage-retry-resource "));
    assert!(requests[5].starts_with("GET /api/social/messages?contactId=2&limit=200 "));
    restarted.shutdown(); drop(owner); drop(restarted);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_equal_timestamp_pagination_keeps_cursor_through_refresh_reaction_and_restart() {
    let root = temp_root("human-stable-sequence-pages");
    let initial = (3_000_001..=3_000_005).map(human_history_message).collect::<Vec<_>>();
    let mut refreshed = (3_000_001..=3_000_006).map(human_history_message).collect::<Vec<_>>();
    refreshed[1]["text"] = json!("refreshed older content");
    refreshed[1]["createdAt"] = json!("1969-12-31T23:59:59Z");
    let (url, server) = spawn_human_message_fixture(vec![
        human_history_reply(initial.clone()), human_history_reply(initial),
        human_history_reply(vec![human_history_message(3_000_006)]), human_history_reply(refreshed),
        HumanMessageFixtureReply::Json(200, json!({
            "success":true,"messageId":3_000_002,"reactions":[{"emoji":"👍","count":1,"reactedByMe":true}]
        })),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("id").to_owned();
    runtime.sync_human_conversation(&id).expect("equal timestamp history");
    let tail = dispatch(&runtime, "getHumanConversationTranscriptTail", json!({"conversationId":id,"limit":2}));
    assert_eq!(human_row_ids(tail["entries"].as_array().expect("tail")),
        vec!["human-server-message:3000004", "human-server-message:3000005"]);
    let cursor = tail["nextBeforeSeq"].as_i64().expect("stable cursor");
    let walk_cursor = tail["nextBeforeCursor"].as_str().expect("stable walk cursor").to_owned();
    runtime.sync_human_conversation(&id).expect("new message and known refresh");
    let reacted = runtime.toggle_human_message_reaction(&id, "human-server-message:3000002", "👍")
        .expect("react to older page");
    assert_eq!(reacted["content"], "refreshed older content");
    assert_eq!(reacted["timestampMs"], -1000.0);
    let older = dispatch(&runtime, "getHumanConversationTranscriptTail",
        json!({"conversationId":id,"beforeSeq":cursor,"beforeCursor":walk_cursor,"limit":2}));
    assert_eq!(human_row_ids(older["entries"].as_array().expect("older")),
        vec!["human-server-message:3000002", "human-server-message:3000003"]);
    assert_eq!(older["entries"][0], reacted);
    let oldest_cursor = older["nextBeforeSeq"].as_i64().expect("oldest cursor");
    let oldest_walk_cursor = older["nextBeforeCursor"].as_str().expect("oldest walk cursor").to_owned();
    runtime.shutdown(); drop(runtime);

    let restarted = human_message_fixture_runtime(&root, &url);
    let reopened = dispatch(&restarted, "getHumanConversationTranscriptTail",
        json!({"conversationId":id,"beforeSeq":cursor,"beforeCursor":walk_cursor,"limit":2}));
    assert_eq!(reopened, older, "the persisted cursor retains the complete refreshed rows");
    let oldest = dispatch(&restarted, "getHumanConversationTranscriptTail",
        json!({"conversationId":id,"beforeSeq":oldest_cursor,"beforeCursor":oldest_walk_cursor,"limit":2}));
    assert_eq!(oldest["entries"], json!([human_local_history_message(3_000_001)]));
    assert!(oldest["nextBeforeSeq"].is_null());
    assert!(oldest["nextBeforeCursor"].is_null());
    let all = restarted.read_human_conversation_transcript(&id).expect("all rows");
    assert_eq!(human_row_ids(&all), [3_000_002, 3_000_001, 3_000_003, 3_000_004, 3_000_005, 3_000_006]
        .into_iter()
        .map(|remote_id| format!("human-server-message:{remote_id}")).collect::<Vec<_>>());
    assert_eq!(all[0], reacted, "fresh chronological reads retain the current reaction payload");
    let fresh = dispatch(&restarted, "getHumanConversationTranscriptTail",
        json!({"conversationId":id,"limit":6}));
    assert_eq!(fresh["entries"], json!(all), "a new walk uses the current canonical sort");
    assert!(fresh["nextBeforeSeq"].is_null());
    assert!(fresh["nextBeforeCursor"].is_null());
    let requests = server.join().expect("pagination fixture");
    assert_eq!(requests.len(), 5, "paging and pure restart reads remain local");
    assert!(requests[0].contains("&afterId=0 "));
    assert!(requests[2].contains("&afterId=3000005 "));
    assert_eq!(fixture_request_json(&requests[4]), json!({"emoji":"👍","active":true}));
    restarted.shutdown(); drop(restarted);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_reaction_restart_replays_already_applied_intent_without_retoggling() {
    let root = temp_root("human-reaction-crash-after-apply");
    let target_id = "human-server-message:4000777";
    let summary = json!([{"emoji":"👍","count":1,"reactedByMe":true}]);
    let mut remote = human_history_message(4_000_777);
    remote["reactions"] = summary.clone();
    let (url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(200, json!({"success":true,"messageId":4_000_777,"reactions":summary.clone()})),
        human_history_reply(vec![]), human_history_reply(vec![remote]),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("id").to_owned();
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    let mut applied = human_local_history_message(4_000_777);
    applied["remoteReactions"] = summary;
    applied["reactions"] = json!([{"emoji":"👍","by":"me"}]);
    applied["retainedLocalMetadata"] = json!({"crash":"after row commit, before intent acknowledgement"});
    assert!(owner.append_transcript_entry(&applied).expect("already applied row"));
    assert!(owner.set_metadata("humanReactionIntent", json!({
        "entryId":target_id,"remoteMessageId":"4000777","emoji":"👍","active":true,"localHumanId":"1"
    })).expect("uncleared intent"));
    assert!(owner.set_metadata("humanHistoryCoverage", json!({"version":1,"afterId":4_000_777,"complete":false}))
        .expect("covered history"));
    runtime.shutdown(); drop(owner); drop(runtime);

    let (events, subscription) = record_human_target_events(&id, target_id);
    let restarted = human_message_fixture_runtime(&root, &url);
    assert_eq!(restarted.sync_human_conversation(&id).expect("sync automatically resumes intent"), vec![applied.clone()]);
    let owner = restarted.open_human_conversation_db_owner(&id).expect("owner");
    assert_eq!(owner.get_metadata("humanReactionIntent").expect("durable ack"), Some(json!(null)));
    assert_eq!(owner.get_metadata("humanHistoryCoverage").expect("complete"),
        Some(json!({"version":1,"afterId":4_000_777,"complete":true})));
    assert!(events.lock().expect("events").iter().all(|event| event["entries"] == json!([applied.clone()])),
        "recovery never publishes an inactive or partial intermediate row");
    restarted.shutdown(); drop(owner); drop(restarted);
    let reopened = human_message_fixture_runtime(&root, &url);
    assert_eq!(reopened.read_human_conversation_transcript(&id).expect("pure reopen"), vec![applied]);
    subscription.unsubscribe();
    let requests = server.join().expect("already-applied intent fixture");
    assert_eq!(requests.len(), 3);
    assert!(requests[0].starts_with("POST /api/social/messages/4000777/reactions "));
    assert_eq!(fixture_request_json(&requests[0]), json!({"emoji":"👍","active":true}));
    assert!(requests[1].starts_with("GET /api/social/messages?contactId=2&limit=200&afterId=4000777 "));
    assert!(requests[2].starts_with("GET /api/social/messages?contactId=2&limit=200 "));
    reopened.shutdown(); drop(reopened);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_invalid_persisted_reaction_intents_are_retained_without_http_after_restart() {
    let cases = [
        "malformed-intent", "missing-entry", "missing-active", "foreign-account",
        "empty-emoji", "padded-emoji", "long-emoji", "missing-target", "changed-remote",
        "foreign-author", "wrong-kind", "wrong-author-kind",
        "zero-remote", "unsafe-remote", "leading-zero-remote", "padded-remote",
        "zero-target", "unsafe-target", "leading-zero-target", "padded-target", "missing-remote-target",
        "wrong-canonical-id", "local-outbox-id", "padded-canonical-id",
        "pending-delivery", "dispatching-delivery", "failed-delivery", "missing-delivery",
        "null-delivery", "numeric-delivery",
    ];
    for (index, label) in cases.into_iter().enumerate() {
        let root = temp_root(&format!("human-invalid-intent-{label}"));
        let remote_id = 4_100_000 + index as u64;
        let target_id = match label {
            "wrong-canonical-id" => format!("human-server-message:{}", remote_id + 10_000),
            "local-outbox-id" => format!("human-message:reaction-outbox-{remote_id}"),
            "padded-canonical-id" => format!("human-server-message: {remote_id}"),
            _ => format!("human-server-message:{remote_id}"),
        };
        let (url, stop, server) = spawn_human_no_request_fixture();
        let runtime = human_message_fixture_runtime(&root, &url);
        let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
            .as_str().expect("id").to_owned();
        let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
        let mut original = human_local_history_message(remote_id);
        // Bind the durable intent to the actual damaged row, not a missing ID.
        // ID-only negatives retain valid remote identity, author and sent state.
        original["id"] = json!(target_id);
        original["retainedLocalMetadata"] = json!({"case":label,"keep":"complete row"});
        let mut intent = json!({
            "entryId":target_id,"remoteMessageId":remote_id.to_string(),"emoji":"👍","active":true,"localHumanId":"1"
        });
        let expected = match label {
            "malformed-intent" => { intent = json!(["not an intention object"]); "persisted Human reaction intent lacks entryId" }
            "missing-entry" => { intent.as_object_mut().expect("intent").remove("entryId"); "persisted Human reaction intent lacks entryId" }
            "missing-active" => { intent.as_object_mut().expect("intent").remove("active"); "persisted Human reaction intent lacks desired state" }
            "foreign-account" => { intent["localHumanId"] = json!("3"); "persisted Human reaction intent belongs to a different account" }
            "empty-emoji" => { intent["emoji"] = json!(" "); "persisted Human reaction intent contains invalid emoji" }
            "padded-emoji" => { intent["emoji"] = json!(" 👍"); "persisted Human reaction intent contains invalid emoji" }
            "long-emoji" => { intent["emoji"] = json!("x".repeat(33)); "persisted Human reaction intent contains invalid emoji" }
            "missing-target" => { intent["entryId"] = json!("human-server-message:missing"); "persisted Human reaction target disappeared" }
            "changed-remote" => { intent["remoteMessageId"] = json!("9999999"); "persisted Human reaction remote identity changed" }
            "foreign-author" => { original["authorId"] = json!("3"); "persisted Human reaction target is outside the selected Human conversation" }
            "wrong-kind" => { original["kind"] = json!("notice"); original["text"] = json!("valid persisted notice, not a Human message"); "persisted Human reaction target is outside the selected Human conversation" }
            "wrong-author-kind" => { original["authorKind"] = json!("agent"); "persisted Human reaction target is outside the selected Human conversation" }
            "zero-remote" => { intent["remoteMessageId"] = json!("0"); "persisted Human reaction intent contains invalid remote identity" }
            "unsafe-remote" => { intent["remoteMessageId"] = json!("9007199254740992"); "persisted Human reaction intent contains invalid remote identity" }
            "leading-zero-remote" => { intent["remoteMessageId"] = json!(format!("0{remote_id}")); "persisted Human reaction intent contains invalid remote identity" }
            "padded-remote" => { intent["remoteMessageId"] = json!(format!(" {remote_id} ")); "persisted Human reaction intent contains invalid remote identity" }
            "zero-target" => { original["remoteMessageId"] = json!(0); "Human reaction target has an invalid canonical remote message identity" }
            "unsafe-target" => { original["remoteMessageId"] = json!(9_007_199_254_740_992_u64); "Human reaction target has an invalid canonical remote message identity" }
            "leading-zero-target" => { original["remoteMessageId"] = json!(format!("0{remote_id}")); "Human reaction target has an invalid canonical remote message identity" }
            "padded-target" => { original["remoteMessageId"] = json!(format!(" {remote_id} ")); "Human reaction target has an invalid canonical remote message identity" }
            "missing-remote-target" => { original.as_object_mut().expect("target").remove("remoteMessageId"); "Human reaction target has no canonical remote message identity" }
            "wrong-canonical-id" | "local-outbox-id" | "padded-canonical-id" => "Human reaction target is not a settled canonical message",
            "pending-delivery" => { original["delivery"] = json!("pending"); "Human reaction target is not a settled canonical message" }
            "dispatching-delivery" => { original["delivery"] = json!("dispatching"); "Human reaction target is not a settled canonical message" }
            "failed-delivery" => { original["delivery"] = json!("failed"); "Human reaction target is not a settled canonical message" }
            "missing-delivery" => { original.as_object_mut().expect("target").remove("delivery"); "Human reaction target is not a settled canonical message" }
            "null-delivery" => { original["delivery"] = json!(null); "Human reaction target is not a settled canonical message" }
            "numeric-delivery" => { original["delivery"] = json!(1); "Human reaction target is not a settled canonical message" }
            _ => unreachable!(),
        };
        assert!(owner.append_transcript_entry(&original).expect("seed target"));
        assert!(owner.set_metadata("humanReactionIntent", intent.clone()).expect("seed durable intent"));
        runtime.shutdown(); drop(owner); drop(runtime);

        let (events, subscription) = record_human_target_events(&id, &target_id);
        let restarted = human_message_fixture_runtime(&root, &url);
        let result = restarted.sync_human_conversation(&id);
        let owner = restarted.open_human_conversation_db_owner(&id).expect("reopened owner");
        let observed_rows = owner.get_transcript_entries();
        let observed_intent = owner.get_metadata("humanReactionIntent");
        let observed_events = events.lock().expect("events").clone();
        subscription.unsubscribe();
        restarted.shutdown(); drop(owner); drop(restarted);
        stop.send(()).expect("stop active admission listener");
        let requests = server.join().expect("live no-HTTP fixture");

        assert!(requests.is_empty(), "rejected intent made no HTTP request: {label}");
        assert_eq!(result.expect_err("invalid persisted intent"), expected, "{label}");
        assert_eq!(observed_rows.expect("preserved complete row"), vec![original.clone()], "{label}");
        assert_eq!(observed_intent.expect("preserved intent"), Some(intent.clone()), "{label}");
        assert!(observed_events.is_empty(), "no rejected-intent event: {label}");
        let reopened = human_message_fixture_runtime(&root, &url);
        let owner = reopened.open_human_conversation_db_owner(&id).expect("pure reopen owner");
        assert_eq!(owner.get_transcript_entries().expect("reopen preserved row"), vec![original], "{label}");
        assert_eq!(owner.get_metadata("humanReactionIntent").expect("reopen preserved intent"), Some(intent), "{label}");
        reopened.shutdown(); drop(owner); drop(reopened);
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn shipping_human_reaction_rechecks_current_target_after_http_without_overwriting_newer_fields() {
    for (index, label) in [
        "newer-metadata", "changed-remote", "wrong-kind", "foreign-author", "wrong-author-kind",
        "zero-remote", "unsafe-remote", "leading-zero-remote", "padded-remote", "missing-remote",
        "wrong-canonical-id", "local-outbox-id", "padded-canonical-id",
        "pending-delivery", "dispatching-delivery", "failed-delivery", "missing-delivery",
        "null-delivery", "numeric-delivery",
    ].into_iter().enumerate()
    {
        let root = temp_root(&format!("human-reaction-current-target-{label}"));
        let remote_id = 4_200_000 + index as u64;
        let target_id = format!("human-server-message:{remote_id}");
        let updated_target_id = match label {
            "wrong-canonical-id" => format!("human-server-message:{}", remote_id + 10_000),
            "local-outbox-id" => format!("human-message:reaction-outbox-{remote_id}"),
            "padded-canonical-id" => format!("human-server-message: {remote_id}"),
            _ => target_id.clone(),
        };
        let (seen, accepted) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let summary = json!([{"emoji":"👍","count":1,"reactedByMe":true}]);
        let (url, server) = spawn_human_message_fixture(vec![
            HumanMessageFixtureReply::Deferred(Box::new(HumanMessageFixtureReply::Json(200, json!({
                "success":true,"messageId":remote_id,"reactions":summary.clone()
            }))), seen, resume),
        ]);
        let runtime = human_message_fixture_runtime(&root, &url);
        let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
            .as_str().expect("id").to_owned();
        let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
        let original = human_local_history_message(remote_id);
        assert!(owner.append_transcript_entry(&original).expect("seed target"));
        // Updating the JSON payload does not rekey the physical SQLite row.
        // Watch its changed payload ID so an erroneous extra settlement event
        // cannot evade the event oracle, without capturing parallel fixtures.
        let (events, subscription) = record_human_target_events(&id, &updated_target_id);
        let reacting = {
            let runtime = Arc::clone(&runtime);
            let id = id.clone();
            let target_id = target_id.clone();
            thread::spawn(move || runtime.toggle_human_message_reaction(&id, &target_id, "👍"))
        };
        let observed_request = accepted.recv_timeout(std::time::Duration::from_secs(15));
        let staged = owner.get_metadata("humanReactionIntent");
        let desired = json!({
            "entryId":target_id,"remoteMessageId":remote_id.to_string(),"emoji":"👍","active":true,"localHumanId":"1"
        });
        let mut current = original;
        current["retainedLocalMetadata"] = json!({"written":"while the reaction response is held"});
        current["readAt"] = json!("2026-10-05T01:05:00Z");
        let expected_error = match label {
            "newer-metadata" => None,
            "changed-remote" => { current["remoteMessageId"] = json!((remote_id + 100).to_string()); Some("Human reaction target is not a settled canonical message") }
            "wrong-kind" => { current["kind"] = json!("notice"); current["text"] = json!("valid notice written while reaction waits"); Some("persisted Human reaction target is outside the selected Human conversation") }
            "foreign-author" => { current["authorId"] = json!("3"); Some("persisted Human reaction target is outside the selected Human conversation") }
            "wrong-author-kind" => { current["authorKind"] = json!("agent"); Some("persisted Human reaction target is outside the selected Human conversation") }
            "zero-remote" => { current["remoteMessageId"] = json!(0); Some("Human reaction target has an invalid canonical remote message identity") }
            "unsafe-remote" => { current["remoteMessageId"] = json!(9_007_199_254_740_992_u64); Some("Human reaction target has an invalid canonical remote message identity") }
            "leading-zero-remote" => { current["remoteMessageId"] = json!(format!("0{remote_id}")); Some("Human reaction target has an invalid canonical remote message identity") }
            "padded-remote" => { current["remoteMessageId"] = json!(format!(" {remote_id} ")); Some("Human reaction target has an invalid canonical remote message identity") }
            "missing-remote" => { current.as_object_mut().expect("target").remove("remoteMessageId"); Some("Human reaction target has no canonical remote message identity") }
            "wrong-canonical-id" | "local-outbox-id" | "padded-canonical-id" => {
                current["id"] = json!(updated_target_id);
                Some("Human reaction target is not a settled canonical message")
            }
            "pending-delivery" => { current["delivery"] = json!("pending"); Some("Human reaction target is not a settled canonical message") }
            "dispatching-delivery" => { current["delivery"] = json!("dispatching"); Some("Human reaction target is not a settled canonical message") }
            "failed-delivery" => { current["delivery"] = json!("failed"); Some("Human reaction target is not a settled canonical message") }
            "missing-delivery" => { current.as_object_mut().expect("target").remove("delivery"); Some("Human reaction target is not a settled canonical message") }
            "null-delivery" => { current["delivery"] = json!(null); Some("Human reaction target is not a settled canonical message") }
            "numeric-delivery" => { current["delivery"] = json!(1); Some("Human reaction target is not a settled canonical message") }
            _ => unreachable!(),
        };
        let independent_update = owner.update_transcript_entry(&target_id, &current);
        let before_settlement = events.lock().expect("events before settlement").clone();
        // Release before testing any observed outcome, so a failing oracle does
        // not strand either the HTTP writer or the production reaction worker.
        let released = release.send(());
        let result = reacting.join().expect("reaction worker");
        let requests = server.join().expect("held reaction fixture");
        observed_request.expect("reaction POST is held before independent SQLite update");
        released.expect("release reaction response");
        assert_eq!(staged.expect("staged intention"), Some(desired.clone()), "{label}");
        assert_eq!(independent_update.expect("independent durable update"), Some(current.clone()), "{label}");
        assert_eq!(before_settlement.len(), 1, "observe the independent update before settlement: {label}");
        assert_eq!(before_settlement[0]["entries"], json!([current.clone()]), "{label}");

        let expected = if let Some(error) = expected_error {
            assert_eq!(result.expect_err("changed target cannot be settled"), error, "{label}");
            assert_eq!(owner.get_metadata("humanReactionIntent").expect("retained intent"), Some(desired), "{label}");
            assert_eq!(*events.lock().expect("events after rejection"), before_settlement,
                "target rejection publishes no settlement event: {label}");
            current
        } else {
            let mut expected = current;
            expected["remoteReactions"] = summary;
            expected["reactions"] = json!([{"emoji":"👍","by":"me"}]);
            assert_eq!(result.expect("current target settles"), expected, "{label}");
            assert_eq!(owner.get_metadata("humanReactionIntent").expect("acknowledged intent"), Some(json!(null)));
            let published = events.lock().expect("settled events").clone();
            assert_eq!(published.len(), before_settlement.len() + 1);
            assert_eq!(published.last().expect("reaction event")["entries"], json!([expected.clone()]));
            expected
        };
        assert_eq!(owner.get_transcript_entries().expect("current complete row"), vec![expected.clone()], "{label}");
        assert_eq!(requests.len(), 1, "exactly one dispatched reaction: {label}");
        assert!(requests[0].starts_with(&format!("POST /api/social/messages/{remote_id}/reactions ")));
        assert_eq!(fixture_request_json(&requests[0]), json!({"emoji":"👍","active":true}));
        let final_intent = owner.get_metadata("humanReactionIntent").expect("final durable intent");
        subscription.unsubscribe();
        runtime.shutdown(); drop(owner); drop(runtime);
        let reopened = human_message_fixture_runtime(&root, &url);
        let owner = reopened.open_human_conversation_db_owner(&id).expect("pure reopen owner");
        assert_eq!(owner.get_transcript_entries().expect("pure reopen row"), vec![expected], "{label}");
        assert_eq!(owner.get_metadata("humanReactionIntent").expect("pure reopen intent"), final_intent, "{label}");
        reopened.shutdown(); drop(owner); drop(reopened);
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn shipping_human_reaction_gateway_rejects_invalid_targets_and_emoji_before_staging_intent() {
    let cases = [
        "wrong-kind", "wrong-author-kind", "foreign-author", "zero-number", "zero-string",
        "unsafe-number", "unsafe-string", "leading-zero-remote", "padded-remote", "missing-remote",
        "padded-emoji-leading", "padded-emoji-trailing",
        "wrong-canonical-id", "local-outbox-id", "padded-canonical-id",
        "pending-delivery", "dispatching-delivery", "failed-delivery", "missing-delivery",
        "null-delivery", "numeric-delivery",
    ];
    for (index, label) in cases.into_iter().enumerate() {
        let root = temp_root(&format!("human-reaction-gateway-admission-{label}"));
        let remote_id = 4_300_000 + index as u64;
        let target_id = match label {
            "wrong-canonical-id" => format!("human-server-message:{}", remote_id + 10_000),
            "local-outbox-id" => format!("human-message:reaction-outbox-{remote_id}"),
            // Internal padding survives required_string's trim, so the gateway
            // still looks up this row instead of failing on a missing target.
            "padded-canonical-id" => format!("human-server-message: {remote_id}"),
            _ => format!("human-server-message:{remote_id}"),
        };
        let (url, stop, server) = spawn_human_no_request_fixture();
        let runtime = human_message_fixture_runtime(&root, &url);
        let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
            .as_str().expect("id").to_owned();
        let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
        let mut original = human_local_history_message(remote_id);
        original["id"] = json!(target_id);
        original["retainedLocalMetadata"] = json!({"case":label,"keep":"complete unstaged target"});
        let mut emoji = "👍";
        let expected_error = match label {
            "wrong-kind" => { original["kind"] = json!("notice"); original["text"] = json!("valid notice is not a reaction target"); "persisted Human reaction target is outside the selected Human conversation" }
            "wrong-author-kind" => { original["authorKind"] = json!("agent"); "persisted Human reaction target is outside the selected Human conversation" }
            "foreign-author" => { original["authorId"] = json!("3"); "persisted Human reaction target is outside the selected Human conversation" }
            "zero-number" => { original["remoteMessageId"] = json!(0); "Human reaction target has an invalid canonical remote message identity" }
            "zero-string" => { original["remoteMessageId"] = json!("0"); "Human reaction target has an invalid canonical remote message identity" }
            "unsafe-number" => { original["remoteMessageId"] = json!(9_007_199_254_740_992_u64); "Human reaction target has an invalid canonical remote message identity" }
            "unsafe-string" => { original["remoteMessageId"] = json!("9007199254740992"); "Human reaction target has an invalid canonical remote message identity" }
            "leading-zero-remote" => { original["remoteMessageId"] = json!(format!("0{remote_id}")); "Human reaction target has an invalid canonical remote message identity" }
            "padded-remote" => { original["remoteMessageId"] = json!(format!(" {remote_id} ")); "Human reaction target has an invalid canonical remote message identity" }
            "missing-remote" => { original.as_object_mut().expect("target").remove("remoteMessageId"); "Human reaction target has no canonical remote message identity" }
            "padded-emoji-leading" => { emoji = " 👍"; "Fabushi Human reaction emoji must not contain surrounding whitespace" }
            "padded-emoji-trailing" => { emoji = "👍 "; "Fabushi Human reaction emoji must not contain surrounding whitespace" }
            "wrong-canonical-id" | "local-outbox-id" | "padded-canonical-id" => "Human reaction target is not a settled canonical message",
            "pending-delivery" => { original["delivery"] = json!("pending"); "Human reaction target is not a settled canonical message" }
            "dispatching-delivery" => { original["delivery"] = json!("dispatching"); "Human reaction target is not a settled canonical message" }
            "failed-delivery" => { original["delivery"] = json!("failed"); "Human reaction target is not a settled canonical message" }
            "missing-delivery" => { original.as_object_mut().expect("target").remove("delivery"); "Human reaction target is not a settled canonical message" }
            "null-delivery" => { original["delivery"] = json!(null); "Human reaction target is not a settled canonical message" }
            "numeric-delivery" => { original["delivery"] = json!(1); "Human reaction target is not a settled canonical message" }
            _ => unreachable!(),
        };
        assert!(owner.append_transcript_entry(&original).expect("seed invalid target"));
        assert_eq!(owner.get_metadata("humanReactionIntent").expect("no existing intent"), None);
        let (events, subscription) = record_human_target_events(&id, &target_id);
        // Exercise the production RPC carrier too: trimming emoji here would
        // bypass the worker's exact-identity admission before staging.
        let result = dispatch_production_session_gateway_call(&runtime, "reactHumanMessage", &json!({
            "conversationId":id,"entryId":target_id,"emoji":emoji
        }));
        let observed_rows = owner.get_transcript_entries();
        let observed_intent = owner.get_metadata("humanReactionIntent");
        let observed_events = events.lock().expect("admission events").clone();
        subscription.unsubscribe();
        runtime.shutdown(); drop(owner); drop(runtime);
        stop.send(()).expect("stop active gateway listener");
        let requests = server.join().expect("live gateway admission fixture");

        assert!(requests.is_empty(), "gateway admission made no HTTP request: {label}");
        assert_eq!(result.expect("handled reaction gateway request").expect_err("invalid new reaction"),
            SessionGatewayError::Internal(expected_error.to_owned()), "{label}");
        assert_eq!(observed_rows.expect("unchanged whole target"), vec![original.clone()], "{label}");
        assert_eq!(observed_intent.expect("no intention staged"), None, "{label}");
        assert!(observed_events.is_empty(), "new reaction rejection publishes no mutation: {label}");
        let reopened = human_message_fixture_runtime(&root, &url);
        let owner = reopened.open_human_conversation_db_owner(&id).expect("pure reopen owner");
        assert_eq!(owner.get_transcript_entries().expect("reopen unchanged target"), vec![original], "{label}");
        assert_eq!(owner.get_metadata("humanReactionIntent").expect("reopen has no intention"), None, "{label}");
        reopened.shutdown(); drop(owner); drop(reopened);
        let _ = fs::remove_dir_all(root);
    }

    // Existing numeric remote identities remain valid through the same public
    // gateway, while the outgoing route and durable intention use canonical text.
    let root = temp_root("human-reaction-gateway-canonical-number");
    let remote_id = 4_400_999_u64;
    let target_id = format!("human-server-message:{remote_id}");
    let summary = json!([{"emoji":"👍","count":1,"reactedByMe":true}]);
    let (url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(200, json!({"success":true,"messageId":remote_id,"reactions":summary.clone()})),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("id").to_owned();
    let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
    let mut original = human_local_history_message(remote_id);
    original["remoteMessageId"] = json!(remote_id);
    original["retainedLocalMetadata"] = json!({"keep":"numeric compatibility"});
    assert!(owner.append_transcript_entry(&original).expect("seed canonical numeric target"));
    let (events, subscription) = record_human_target_events(&id, &target_id);
    let result = dispatch_production_session_gateway_call(&runtime, "reactHumanMessage", &json!({
        "conversationId":id,"entryId":target_id,"emoji":"👍"
    }));
    let requests = server.join().expect("canonical numeric positive fixture");
    let mut expected = original;
    expected["remoteReactions"] = summary;
    expected["reactions"] = json!([{"emoji":"👍","by":"me"}]);
    assert_eq!(result.expect("handled canonical numeric reaction").expect("numeric target admitted"), expected);
    assert_eq!(owner.get_transcript_entries().expect("complete numeric target"), vec![expected.clone()]);
    assert_eq!(owner.get_metadata("humanReactionIntent").expect("numeric intention acknowledged"), Some(json!(null)));
    assert_eq!(requests.len(), 1, "one positive-control reaction POST");
    assert!(requests[0].starts_with("POST /api/social/messages/4400999/reactions "));
    assert_eq!(fixture_request_json(&requests[0]), json!({"emoji":"👍","active":true}));
    let published = events.lock().expect("numeric target events").clone();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0]["entries"], json!([expected.clone()]));
    subscription.unsubscribe();
    runtime.shutdown(); drop(owner); drop(runtime);
    let reopened = human_message_fixture_runtime(&root, &url);
    let owner = reopened.open_human_conversation_db_owner(&id).expect("positive pure reopen owner");
    assert_eq!(owner.get_transcript_entries().expect("positive reopen row"), vec![expected]);
    assert_eq!(owner.get_metadata("humanReactionIntent").expect("positive reopen acknowledgement"), Some(json!(null)));
    reopened.shutdown(); drop(owner); drop(reopened);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_malformed_history_ids_preserve_page_checkpoint_and_recover_after_restart() {
    let cases = [
        "leading-zero-tail", "padded-tail", "signed-tail", "noncanonical-middle", "zero-middle",
        "unsafe-middle", "negative-middle", "fractional-middle", "duplicate", "backwards", "nonadvancing",
        "max-safe-leading-zero",
    ];
    for (index, label) in cases.into_iter().enumerate() {
        let root = temp_root(&format!("human-history-id-admission-{label}"));
        let after = if label == "max-safe-leading-zero" { 9_007_199_254_740_990_u64 }
            else { 5_000_000 + index as u64 * 100 };
        let invalid_id = "Fabushi Human message identity is not a canonical positive safe integer";
        let invalid_order = "Fabushi Human history afterId did not advance monotonically";
        let (ids, next_after, expected_error) = match label {
            "leading-zero-tail" => (vec![json!(after + 1), json!(format!("0{}", after + 2))], after + 2, invalid_id),
            "padded-tail" => (vec![json!(after + 1), json!(format!(" {} ", after + 2))], after + 2, invalid_id),
            "signed-tail" => (vec![json!(after + 1), json!(format!("+{}", after + 2))], after + 2, invalid_id),
            "noncanonical-middle" => (vec![json!(after + 1), json!(format!("0{}", after + 2)), json!(after + 3)], after + 3, invalid_id),
            "zero-middle" => (vec![json!(after + 1), json!(0), json!(after + 3)], after + 3, invalid_id),
            "unsafe-middle" => (vec![json!(after + 1), json!(9_007_199_254_740_992_u64), json!(after + 3)], after + 3, invalid_id),
            "negative-middle" => (vec![json!(after + 1), json!(-1), json!(after + 3)], after + 3, invalid_id),
            "fractional-middle" => (vec![json!(after + 1), json!(after as f64 + 1.5), json!(after + 3)], after + 3, invalid_id),
            "duplicate" => (vec![json!(after + 1), json!(after + 1)], after + 1, invalid_order),
            "backwards" => (vec![json!(after + 2), json!(after + 1)], after + 1, invalid_order),
            "nonadvancing" => (vec![json!(after)], after, invalid_order),
            "max-safe-leading-zero" => (vec![json!("09007199254740991")], 9_007_199_254_740_991, invalid_id),
            _ => unreachable!(),
        };
        let marker = format!("invalid-history-page-{after}-");
        let rows = ids.into_iter().enumerate().map(|(position, remote_id)| {
            let mut row = human_history_message(after + 1);
            row["id"] = remote_id;
            row["text"] = json!(format!("{marker}{position}"));
            row["attachments"] = json!([{
                "resourceId":format!("must-not-read-page-{after}"),"name":"forbidden.bin",
                "contentType":"application/octet-stream","size":4,"createdAt":"1970-01-01T00:00:01Z"
            }]);
            row
        }).collect::<Vec<_>>();
        // Supply the cursor fields explicitly. In particular, a numeric next
        // cursor must not disguise a noncanonical text ID in the response.
        let bad_page = json!({"success":true,"data":{
            "messages":rows,"historyCursor":{"mode":"after-id","afterId":after},"nextAfterId":next_after
        }});
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind history admission fixture");
        listener.set_nonblocking(true).expect("nonblocking history admission fixture");
        let url = format!("http://{}", listener.local_addr().expect("history admission address"));
        let (stop, stopped) = std::sync::mpsc::channel();
        let (ready, active) = std::sync::mpsc::channel();
        let server = thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            let mut first_reply = Some(bad_page);
            let mut requests = Vec::new();
            ready.send(()).expect("report active history admission listener");
            loop {
                assert!(std::time::Instant::now() < deadline, "history admission fixture was not stopped");
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).expect("blocking history request");
                        stream.set_write_timeout(Some(std::time::Duration::from_secs(15)))
                            .expect("bounded history admission response");
                        requests.push(read_fixture_http_request_before(&mut stream, Some(deadline)));
                        let (status, body) = match first_reply.take() {
                            Some(body) => (200, body),
                            None => (503, json!({"success":false,"error":"unexpected request after rejected history page"})),
                        };
                        let body = body.to_string();
                        write!(stream,
                            "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(), body,
                        ).expect("write history admission response");
                        stream.flush().expect("flush history admission response");
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        match stopped.recv_timeout(std::time::Duration::from_millis(5)) {
                            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        }
                    }
                    Err(error) => panic!("history admission fixture accept: {error}"),
                }
            }
            requests
        });
        active.recv_timeout(std::time::Duration::from_secs(15)).expect("active listener before history sync");
        let runtime = human_message_fixture_runtime(&root, &url);
        let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
            .as_str().expect("id").to_owned();
        let owner = runtime.open_human_conversation_db_owner(&id).expect("owner");
        let mut original = human_local_history_message(after);
        original["retainedLocalMetadata"] = json!({"case":label,"keep":"established complete row"});
        let coverage = json!({"version":1,"afterId":after,"complete":false});
        assert!(owner.append_transcript_entry(&original).expect("seed established anchor"));
        assert!(owner.set_metadata("humanHistoryCoverage", coverage.clone()).expect("seed coverage"));
        let events = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
        let captured = Arc::clone(&events);
        let event_conversation = id.clone();
        let anchor_id = format!("human-server-message:{after}");
        let next_id = format!("human-server-message:{}", after + 1);
        let subscription = mahayana_host_runtime::transcript_mutation_events::subscribe_transcript_mutations(
            move |mutation| {
                if mutation.get("agentId").and_then(serde_json::Value::as_str) != Some(event_conversation.as_str()) {
                    return;
                }
                let targets_entry = mutation.get("entryId").and_then(serde_json::Value::as_str)
                    .is_some_and(|entry| entry == anchor_id.as_str() || entry == next_id.as_str())
                    || mutation.get("entries").and_then(serde_json::Value::as_array).is_some_and(|entries| {
                        entries.iter().any(|entry| {
                            entry.get("id").and_then(serde_json::Value::as_str)
                                .is_some_and(|entry| entry == anchor_id.as_str() || entry == next_id.as_str())
                                || entry.get("content").and_then(serde_json::Value::as_str)
                                    .is_some_and(|content| content.starts_with(&marker))
                        })
                    });
                if targets_entry {
                    captured.lock().expect("record page mutation").push(serde_json::Value::Object(mutation.clone()));
                }
            },
        );
        let result = runtime.sync_human_conversation(&id);
        let rejected_rows = owner.get_transcript_entries();
        let rejected_coverage = owner.get_metadata("humanHistoryCoverage");
        let rejected_events = events.lock().expect("rejected page events").clone();
        let touched_resources = root.join("agents/human-message-resources").exists();
        runtime.shutdown(); drop(owner); drop(runtime);
        stop.send(()).expect("stop history admission listener");
        let requests = server.join().expect("active history admission fixture");

        assert_eq!(result.expect_err("invalid page cannot be accepted"), expected_error, "{label}");
        assert_eq!(rejected_rows.expect("unchanged complete rows"), vec![original.clone()], "{label}");
        assert_eq!(rejected_coverage.expect("unchanged checkpoint"), Some(coverage), "{label}");
        assert!(rejected_events.is_empty(), "no rejected-page event: {label}");
        assert!(!touched_resources, "whole-page validation precedes any resource directory: {label}");
        assert_eq!(requests.len(), 1, "no resource or later-page request after rejection: {label}");
        assert!(requests[0].starts_with(&format!("GET /api/social/messages?contactId=2&limit=200&afterId={after} ")));

        let numeric_row = human_history_message(after + 1);
        let mut string_row = numeric_row.clone();
        string_row["id"] = json!((after + 1).to_string());
        let (resume_url, resume_server) = spawn_human_message_fixture(vec![
            HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{
                "messages":[string_row],"historyCursor":{"mode":"after-id","afterId":after},"nextAfterId":after + 1
            }})),
            // Latest snapshots are not required to use ascending afterId order.
            // The numeric echo of the accepted string ID must refresh one row.
            human_history_reply(vec![numeric_row, human_history_message(after)]),
        ]);
        let restarted = human_message_fixture_runtime(&root, &resume_url);
        let recovered = restarted.sync_human_conversation(&id).expect("valid page resumes the retained checkpoint");
        let expected_rows = vec![original, human_local_history_message(after + 1)];
        assert_eq!(recovered, expected_rows, "numeric and string IDs share one canonical row: {label}");
        let owner = restarted.open_human_conversation_db_owner(&id).expect("recovery owner");
        assert_eq!(owner.get_transcript_entries().expect("recovered complete rows"), expected_rows, "{label}");
        assert_eq!(owner.get_metadata("humanHistoryCoverage").expect("recovered coverage"),
            Some(json!({"version":1,"afterId":after + 1,"complete":true})), "{label}");
        let requests = resume_server.join().expect("valid recovery fixture");
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with(&format!("GET /api/social/messages?contactId=2&limit=200&afterId={after} ")));
        assert!(requests[1].starts_with("GET /api/social/messages?contactId=2&limit=200 "));
        assert!(!root.join("agents/human-message-resources").exists(), "recovery never reads rejected resources: {label}");
        let published = events.lock().expect("recovery events").clone();
        assert_eq!(published.len(), 1, "only the new canonical row publishes: {label}");
        assert_eq!(published[0]["entries"], json!([human_local_history_message(after + 1)]));
        restarted.shutdown(); drop(owner); drop(restarted);
        let reopened = human_message_fixture_runtime(&root, &resume_url);
        assert_eq!(reopened.read_human_conversation_transcript(&id).expect("pure reopen recovered history"), expected_rows, "{label}");
        assert_eq!(events.lock().expect("events after pure reopen").len(), 1);
        subscription.unsubscribe();
        reopened.shutdown(); drop(reopened);
        let _ = fs::remove_dir_all(root);
    }
}

// NMR-HUMAN-RETRY-01: the public retry carrier contains identities only. The
// production Session must recover payload and forwarding authority from SQLite.
#[test]
fn shipping_human_retry_reopens_attachment_reply_and_coalesces_same_nonce_without_reupload() {
    const NONCE: &str = "explicit-retry-attachment-reply";
    const TEXT: &str = "retry the durable attachment and reply";
    const BYTES: &[u8] = b"durable retry attachment\r\n";
    let root = temp_root("human-explicit-retry-attachment-reply");
    fs::create_dir_all(&root).expect("retry fixture root");
    let path = root.join("retry.txt");
    fs::write(&path, BYTES).expect("original attachment");
    let resource = json!({
        "resourceId":"retry-resource-1","name":"retry.txt","contentType":"text/plain",
        "size":BYTES.len(),"createdAt":"2026-10-05T01:00:00Z"
    });
    let mut remote = fixture_remote_human_message(4_500_002, 1, 2, TEXT, Some(NONCE),
        json!([resource.clone()]));
    remote["replyToMessageId"] = json!(4_500_001);
    let (seen, accepted) = std::sync::mpsc::channel();
    let (release, resume) = std::sync::mpsc::channel();
    let (url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(201, json!({"success":true,"resource":resource})),
        HumanMessageFixtureReply::Disconnect,
        HumanMessageFixtureReply::Deferred(Box::new(HumanMessageFixtureReply::Json(200,
            json!({"success":true,"deduplicated":true,"message":remote}))), seen, resume),
        HumanMessageFixtureReply::Json(200, json!({
            "success":true,"messageId":4_500_001,
            "reactions":[{"emoji":"👍","count":1,"reactedByMe":true}]
        })),
        HumanMessageFixtureReply::Bytes(200, BYTES.to_vec()),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("conversation id").to_owned();
    let owner = runtime.open_human_conversation_db_owner(&id).expect("SQLite owner");
    let anchor = human_local_history_message(4_500_001);
    assert!(owner.append_transcript_entry(&anchor).expect("canonical reply anchor"));
    let failed = dispatch_production_session_gateway_call(&runtime, "sendHumanMessage", &json!({
        "conversationId":id,"senderId":"1","clientNonce":NONCE,"text":TEXT,
        "composedAtMs":1234.0,"replyToId":"human-server-message:4500001",
        "attachments":[{"path":path.to_string_lossy(),"name":"retry.txt"}]
    })).expect("handled original send").expect_err("accepted POST lost its response");
    assert!(matches!(failed, SessionGatewayError::Internal(_)));
    let pending = owner.get_transcript_entries().expect("durable ambiguous intent");
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0], anchor);
    assert_eq!(pending[1]["id"], format!("human-message:{NONCE}"));
    assert_eq!(pending[1]["delivery"], "pending");
    assert_eq!(pending[1]["clientNonce"], NONCE);
    assert_eq!(pending[1]["content"], TEXT);
    assert_eq!(pending[1]["composedAtMs"], 1234.0);
    assert_eq!(pending[1]["replyToId"], "human-server-message:4500001");
    assert_eq!(pending[1]["remoteDispatchAttachments"][0]["resourceId"], "retry-resource-1");
    assert!(pending[1]["lastDispatchError"].as_str().is_some_and(|error| !error.is_empty()));
    assert!(pending[1]["lastDispatchFailedAtMs"].is_number());
    runtime.shutdown(); drop(owner); drop(runtime);
    fs::remove_file(&path).expect("committed upload no longer depends on composer staging");

    let restarted = human_message_fixture_runtime(&root, &url);
    assert_eq!(restarted.read_human_conversation_transcript(&id).expect("pure failed reopen"), pending);
    let retry_args = json!({"conversationId":id,"clientNonce":NONCE});
    let retrying = {
        let runtime = Arc::clone(&restarted); let args = retry_args.clone();
        thread::spawn(move || dispatch(&runtime, "retryHumanMessage", args))
    };
    accepted.recv_timeout(std::time::Duration::from_secs(15)).expect("explicit retry POST in flight");
    let owner = restarted.open_human_conversation_db_owner(&id).expect("reopened owner");
    let in_flight = owner.get_transcript_entries().expect("retry dispatching intent");
    let mut expected_in_flight = pending.clone();
    expected_in_flight[1]["delivery"] = json!("dispatching");
    expected_in_flight[1].as_object_mut().unwrap().remove("lastDispatchError");
    expected_in_flight[1].as_object_mut().unwrap().remove("lastDispatchFailedAtMs");
    assert_eq!(in_flight, expected_in_flight, "retry stages the original complete intent");
    let (second_started, second_wait) = std::sync::mpsc::channel();
    let second_retry = {
        let runtime = Arc::clone(&restarted); let args = retry_args.clone();
        thread::spawn(move || {
            second_started.send(()).expect("start concurrent same-target retry");
            dispatch(&runtime, "retryHumanMessage", args)
        })
    };
    second_wait.recv_timeout(std::time::Duration::from_secs(15)).expect("second caller started");
    // A real mutation on this conversation must finish before the held retry
    // response is released. A retry lane may not hold the conversation lane.
    let (reaction_done, reaction_wait) = std::sync::mpsc::channel();
    let reacting = {
        let runtime = Arc::clone(&restarted); let id = id.clone();
        thread::spawn(move || {
            let result = dispatch_production_session_gateway_call(&runtime, "reactHumanMessage", &json!({
                "conversationId":id,"entryId":"human-server-message:4500001","emoji":"👍"
            })).expect("handled reaction during retry");
            reaction_done.send(result).expect("report independent reaction settlement");
        })
    };
    let reacted = reaction_wait.recv_timeout(std::time::Duration::from_secs(15));
    release.send(()).expect("release retry after independent conversation mutation");
    let reacted = reacted.expect("conversation lane is free during retry HTTP").expect("reaction settled");
    reacting.join().expect("reaction thread");
    let settled = retrying.join().expect("first explicit retry");
    assert_eq!(second_retry.join().expect("coalesced explicit retry"), settled);
    assert_eq!(settled["id"], "human-server-message:4500002");
    assert_eq!(settled["delivery"], "sent");
    assert_eq!(settled["clientNonce"], NONCE);
    assert_eq!(settled["content"], TEXT);
    assert_eq!(settled["replyToId"], "human-server-message:4500001");
    assert_eq!(settled["remoteReplyToMessageId"], "4500001");
    assert_eq!(settled["attachments"][0]["resourceId"], "retry-resource-1");
    assert_eq!(fs::read(settled["attachments"][0]["path"].as_str().unwrap()).unwrap(), BYTES);
    for key in ["lastDispatchError", "lastDispatchFailedAtMs", "remoteDispatchAttachments"] {
        assert!(settled.get(key).is_none(), "settlement clears {key}");
    }
    let rows = vec![reacted, settled.clone()];
    assert_eq!(owner.get_transcript_entries().expect("one canonical retry row"), rows);
    assert!(owner.get_transcript_entry_by_id(&format!("human-message:{NONCE}")).unwrap().is_none());
    for _ in 0..2 {
        assert_eq!(dispatch(&restarted, "retryHumanMessage", retry_args.clone()), settled);
    }
    restarted.shutdown(); drop(owner); drop(restarted);
    let reopened = human_message_fixture_runtime(&root, &url);
    assert_eq!(reopened.read_human_conversation_transcript(&id).expect("canonical reopen"), rows);
    assert_eq!(dispatch(&reopened, "retryHumanMessage", retry_args), settled);
    reopened.shutdown(); drop(reopened);

    let requests = server.join().expect("explicit retry HTTP fixture");
    assert_eq!(requests.len(), 5, "one upload, two same-nonce message POSTs, one reaction and one download");
    assert!(requests[0].starts_with("POST /api/social/message-resources "));
    assert!(requests[1].starts_with("POST /api/social/messages "));
    assert!(requests[2].starts_with("POST /api/social/messages "));
    assert!(requests[3].starts_with("POST /api/social/messages/4500001/reactions "));
    assert!(requests[4].starts_with("GET /api/social/message-resources/retry-resource-1 "));
    let submitted = fixture_request_json(&requests[1]);
    assert_eq!(fixture_request_json(&requests[2]), submitted, "same durable request after restart");
    assert_eq!(submitted["clientRequestId"], NONCE);
    assert_eq!(submitted["targetUserId"], "2");
    assert_eq!(submitted["text"], TEXT);
    assert_eq!(submitted["replyToMessageId"], "4500001");
    assert_eq!(submitted["attachments"], json!([{"resourceId":"retry-resource-1"}]));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_retry_rejects_invalid_identity_and_payload_without_http_or_mutation() {
    let cases = [
        "missing-row", "missing-conversation", "foreign-author", "wrong-kind", "wrong-role",
        "wrong-author-kind", "missing-author-kind",
        "wrong-local-id", "mismatched-nonce", "missing-nonce", "numeric-nonce", "missing-content",
        "nonstring-content", "invalid-attachments", "invalid-dispatch-resources", "conflicting-dispatch-resources",
        "dispatch-resource-parent-path", "dispatch-resource-backslash", "dispatch-resource-overlength",
        "dispatch-resource-empty-size", "dispatch-resource-oversized",
        "missing-delivery", "unsupported-delivery", "local-with-remote",
        "canonical-missing-remote", "canonical-wrong-remote", "canonical-padded-remote",
        "canonical-pending", "duplicate-pending", "duplicate-canonical", "foreign-participants",
        "participants-more-than-two", "participants-duplicate-local", "participants-rebound-peer",
        "participants-empty-peer", "participants-padded-peer",
        "args-null", "args-array", "args-missing-conversation", "args-missing-nonce",
        "args-empty-conversation", "args-leading-conversation", "args-trailing-conversation",
        "args-nonstring-conversation", "args-empty-nonce", "args-leading-nonce", "args-trailing-nonce",
        "args-nonstring-nonce", "args-long-nonce", "args-long-utf8", "extra-text", "extra-senderId",
        "extra-attachments", "extra-replyToId", "extra-composedAtMs", "extra-forwardContext",
        "extra-forwardBatchOwned", "extra-sourceConversationId", "extra-sourceEntryId",
        "extra-destinationConversationIds", "extra-remoteDispatchAttachments",
        "canonical-positive", "canonical-numeric-positive", "canonical-utf8-boundary",
    ];
    for label in cases {
        let root = temp_root(&format!("human-retry-admission-{label}"));
        let (url, stop, server) = spawn_human_no_request_fixture();
        let runtime = human_message_fixture_runtime(&root, &url);
        let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
            .as_str().expect("conversation id").to_owned();
        let nonce = if label == "canonical-utf8-boundary" { "é".repeat(100) } else { "retry-admission".to_owned() };
        let canonical = label.starts_with("canonical-");
        let positive = matches!(label, "canonical-positive" | "canonical-numeric-positive" | "canonical-utf8-boundary");
        let mut row = if canonical {
            settled_reply_fixture_row(&nonce, "retained retry content", 4_500_100)
        } else { pending_reply_fixture_row(&nonce, "retained retry content", None) };
        if canonical { row["remoteCreatedAt"] = json!("1970-01-01T00:00:01.234Z"); }
        row["retainedLocalMetadata"] = json!({"case":label,"keep":"unchanged whole durable row"});
        row["lastDispatchError"] = json!("retained historical dispatch failure");
        row["lastDispatchFailedAtMs"] = json!(1230.0);
        let mut args = json!({"conversationId":id,"clientNonce":nonce});
        match label {
            "missing-conversation" => args["conversationId"] = json!("human-unmaterialized-retry-target"),
            "foreign-author" => row["authorId"] = json!("2"),
            "wrong-kind" => { row["kind"] = json!("notice"); row["text"] = json!("not a Human message"); }
            "wrong-role" => row["role"] = json!("assistant"),
            "wrong-author-kind" => row["authorKind"] = json!("agent"),
            "missing-author-kind" => { row.as_object_mut().unwrap().remove("authorKind"); }
            "wrong-local-id" => row["id"] = json!("human-message:another-nonce"),
            "mismatched-nonce" => row["clientNonce"] = json!("another-nonce"),
            "missing-nonce" => { row.as_object_mut().unwrap().remove("clientNonce"); }
            "numeric-nonce" => row["clientNonce"] = json!(7),
            "missing-content" => { row.as_object_mut().unwrap().remove("content"); }
            "nonstring-content" => row["content"] = json!({"invalid":"content"}),
            "invalid-attachments" => row["attachments"] = json!({"invalid":"not an attachment array"}),
            "invalid-dispatch-resources" => row["remoteDispatchAttachments"] = json!({"invalid":"not a resource array"}),
            "conflicting-dispatch-resources" => {
                row["attachments"] = json!([{"path":root.join("retained-upload.txt"),"name":"original.txt"}]);
                row["remoteDispatchAttachments"] = json!([{
                    "resourceId":"retained-upload","name":"different.txt","contentType":"text/plain",
                    "size":4,"createdAt":"2026-10-05T01:00:00Z"
                }]);
            }
            _ if label.starts_with("dispatch-resource-") => {
                row["attachments"] = json!([{"path":root.join("retained-upload.txt"),"name":"original.txt"}]);
                let mut resource = json!({
                    "resourceId":"retained-upload","name":"original.txt","contentType":"text/plain",
                    "size":4,"createdAt":"2026-10-05T01:00:00Z"
                });
                match label {
                    "dispatch-resource-parent-path" => resource["resourceId"] = json!("../retained-upload"),
                    "dispatch-resource-backslash" => resource["resourceId"] = json!("retained\\upload"),
                    "dispatch-resource-overlength" => resource["resourceId"] = json!("r".repeat(129)),
                    "dispatch-resource-empty-size" => resource["size"] = json!(0),
                    "dispatch-resource-oversized" => resource["size"] = json!(32_u64 * 1024 * 1024 + 1),
                    _ => unreachable!(),
                }
                row["remoteDispatchAttachments"] = json!([resource]);
            }
            "missing-delivery" => { row.as_object_mut().unwrap().remove("delivery"); }
            "unsupported-delivery" => row["delivery"] = json!("failed"),
            "local-with-remote" => row["remoteMessageId"] = json!("4500100"),
            "canonical-missing-remote" => { row.as_object_mut().unwrap().remove("remoteMessageId"); }
            "canonical-wrong-remote" => row["remoteMessageId"] = json!("4500101"),
            "canonical-padded-remote" => row["remoteMessageId"] = json!(" 4500100 "),
            "canonical-pending" => row["delivery"] = json!("pending"),
            "canonical-numeric-positive" => row["remoteMessageId"] = json!(4_500_100),
            "args-null" => args = json!(null),
            "args-array" => args = json!([id, nonce]),
            "args-missing-conversation" => { args.as_object_mut().unwrap().remove("conversationId"); }
            "args-missing-nonce" => { args.as_object_mut().unwrap().remove("clientNonce"); }
            "args-empty-conversation" => args["conversationId"] = json!(""),
            "args-leading-conversation" => args["conversationId"] = json!(format!(" {id}")),
            "args-trailing-conversation" => args["conversationId"] = json!(format!("{id} ")),
            "args-nonstring-conversation" => args["conversationId"] = json!(3),
            "args-empty-nonce" => args["clientNonce"] = json!(""),
            "args-leading-nonce" => args["clientNonce"] = json!(format!(" {nonce}")),
            "args-trailing-nonce" => args["clientNonce"] = json!(format!("{nonce} ")),
            "args-nonstring-nonce" => args["clientNonce"] = json!(7),
            "args-long-nonce" => args["clientNonce"] = json!("a".repeat(201)),
            "args-long-utf8" => args["clientNonce"] = json!("é".repeat(101)),
            _ if label.starts_with("extra-") => {
                args[label.strip_prefix("extra-").unwrap()] = json!(null);
            }
            _ => {}
        }
        let owner = runtime.open_human_conversation_db_owner(&id).expect("admission owner");
        if label != "missing-row" {
            assert!(owner.append_transcript_entry(&row).expect("seed retained retry row"));
        }
        if matches!(label, "duplicate-pending" | "duplicate-canonical") {
            let duplicate = if label == "duplicate-canonical" {
                settled_reply_fixture_row(&nonce, "retained retry content", 4_500_100)
            } else {
                let mut duplicate = row.clone();
                duplicate["id"] = json!("human-message:conflicting-local-identity");
                duplicate
            };
            assert!(owner.append_transcript_entry(&duplicate).expect("seed conflicting nonce row"));
        }
        let original_participants = owner.read_metadata().expect("original metadata")
            .get("participantIds").cloned().expect("original participant pair");
        let canonical_metadata = owner.read_metadata().expect("original canonical conversation metadata");
        let canonical_raw_metadata = owner.read_kv("metadata").expect("original metadata bytes")
            .expect("persisted canonical metadata");
        let participants = match label {
            "foreign-participants" => Some(json!(["2", "3"])),
            "participants-more-than-two" => Some(json!(["1", "2", "3"])),
            "participants-duplicate-local" => Some(json!(["1", "1"])),
            "participants-rebound-peer" => Some(json!(["1", "3"])),
            "participants-empty-peer" => Some(json!(["1", ""])),
            "participants-padded-peer" => Some(json!(["1", " 2 "])),
            _ => None,
        };
        if let Some(participants) = participants {
            assert!(owner.set_metadata("participantIds", participants).unwrap());
        }
        if label == "participants-rebound-peer" {
            let rebound_metadata = owner.read_metadata().expect("rebound metadata");
            assert!(runtime.open_human_conversation_db_owner(&id).is_err(),
                "a rebound peer must not retain authority through the original canonical directory");
            assert_eq!(owner.read_metadata().expect("identity refusal is read-only"), rebound_metadata);
        }
        let before = owner.get_transcript_entries().expect("original complete rows");
        let metadata = owner.read_metadata().expect("original conversation metadata");
        let db = rusqlite::Connection::open_with_flags(forward_db_path(&root, &id),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).expect("independent retry state observation");
        let raw_rows = || {
            let mut statement = db.prepare("SELECT seq, id, entry FROM transcript_entries ORDER BY seq")
                .expect("read exact retained transcript bytes");
            let rows = statement.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?,
                row.get::<_, String>(2)?))).expect("raw transcript query")
                .collect::<Result<Vec<_>, _>>().expect("raw transcript rows");
            rows
        };
        let raw_before = raw_rows();
        let raw_metadata = || db.query_row("SELECT value FROM kv WHERE key = 'metadata'", [],
            |row| row.get::<_, String>(0)).expect("read exact retained metadata bytes");
        let raw_metadata_before = raw_metadata();
        let (events, subscription) = record_human_target_events(&id, row["id"].as_str().unwrap());
        for _ in 0..2 {
            let result = dispatch_production_session_gateway_call(&runtime, "retryHumanMessage", &args)
                .expect("registered explicit Human retry");
            if positive {
                assert_eq!(result.expect("valid canonical retry returns its existing row"), row, "{label}");
            } else if label.starts_with("args-") || label.starts_with("extra-") {
                assert!(matches!(result, Err(SessionGatewayError::BadRequest(_))), "invalid retry carrier: {label}");
            } else {
                assert!(result.is_err(), "invalid durable retry target: {label}");
            }
            assert_eq!(owner.get_transcript_entries().expect("unchanged complete rows"), before, "{label}");
            assert_eq!(owner.read_metadata().expect("unchanged metadata"), metadata, "{label}");
            assert_eq!(raw_rows(), raw_before, "invalid stored payload is preserved byte for byte: {label}");
            assert_eq!(raw_metadata(), raw_metadata_before, "retained metadata bytes are unchanged: {label}");
        }
        assert!(events.lock().expect("admission mutation events").is_empty(), "no retry mutation: {label}");
        subscription.unsubscribe();
        runtime.shutdown(); drop(owner); drop(runtime);
        let reopened = human_message_fixture_runtime(&root, &url);
        if matches!(label, "foreign-participants" | "participants-rebound-peer"
            | "participants-more-than-two" | "participants-duplicate-local"
            | "participants-empty-peer" | "participants-padded-peer") {
            assert!(reopened.open_human_conversation_db_owner(&id).is_err(),
                "cold admission rejects invalid canonical participant pairs: {label}");
        } else {
            let owner = reopened.open_human_conversation_db_owner(&id).expect("pure reopen owner");
            assert_eq!(owner.get_transcript_entries().expect("reopened unchanged rows"), before, "{label}");
            assert_eq!(owner.read_metadata().expect("reopened unchanged metadata"), metadata, "{label}");
        }
        assert_eq!(raw_rows(), raw_before, "reopen preserves the same physical identities: {label}");
        assert_eq!(raw_metadata(), raw_metadata_before, "reopen preserves retained metadata bytes: {label}");
        if matches!(label, "foreign-participants" | "participants-rebound-peer") {
            // The negative admission/retry/reopen checks above retain every
            // original byte. This explicit fixture repair is a separate control.
            let repair = rusqlite::Connection::open(forward_db_path(&root, &id))
                .expect("explicit test-only participant restoration");
            assert_eq!(repair.execute(
                "UPDATE kv SET value = ?1 WHERE key = 'metadata' AND value = ?2",
                rusqlite::params![canonical_raw_metadata, raw_metadata_before],
            ).expect("restore the original metadata bytes"), 1);
            drop(repair);
            let restored = reopened.open_human_conversation_db_owner(&id)
                .expect("ordinary reopen succeeds only after explicit canonical pair restoration");
            assert_eq!(restored.read_metadata().unwrap(), canonical_metadata, "{label}");
            assert_eq!(restored.get_metadata("participantIds").unwrap(), Some(original_participants), "{label}");
            assert_eq!(restored.get_transcript_entries().unwrap(), before, "{label}");
            assert_eq!(raw_rows(), raw_before, "restoration does not adopt or rewrite messages: {label}");
            assert_eq!(raw_metadata(), canonical_raw_metadata, "restore exact original metadata: {label}");
        }
        reopened.shutdown(); drop(reopened);
        drop(db);
        stop.send(()).expect("stop live retry admission listener");
        assert!(server.join().expect("retry admission HTTP fixture").is_empty(),
            "both invalid and already canonical retries produce zero HTTP: {label}");
        assert!(!root.join("agents/human-message-resources").exists(), "no retry resources: {label}");
        assert!(!forward_db_path(&root, "human-unmaterialized-retry-target").exists(),
            "retry cannot materialize a replacement conversation: {label}");
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn shipping_human_retry_preserves_forward_source_policy_and_refuses_generic_ownership_bypass() {
    let root = temp_root("human-retry-forward-ownership");
    let files = forward_attachment_files(&root, 1);
    let mut server = ForwardHttpFixture::start(true);
    let args = prepare_forward_attachment_process(&root, &server.url, &files);
    let source = args["sourceConversationId"].as_str().unwrap();
    let first = args["destinationConversationIds"][0].as_str().unwrap();
    let second = args["destinationConversationIds"][1].as_str().unwrap();
    server.observe_snapshot_staging(&root, source);
    let mut child = ForwardProcess::start(&root, &server.url);
    server.wait_for_first_post(); child.terminate(); server.release_first_post(); drop(child);
    let pins = assert_forward_staged_bytes(&root, &args, &files, &server);
    let runtime = human_message_fixture_runtime(&root, &server.url);
    let source_owner = runtime.open_human_conversation_db_owner(source).expect("source owner");
    let target_owner = runtime.open_human_conversation_db_owner(first).expect("target owner");
    let original = target_owner.get_transcript_entries().unwrap().remove(0);
    let original_id = original["id"].as_str().unwrap();
    let nonce = original["clientNonce"].as_str().unwrap();
    assert_eq!(original["delivery"], "dispatching");
    assert_eq!(original["forwardBatchOwned"], true);
    assert_eq!(original["remoteDispatchAttachments"][0]["resourceId"], "forward-resource-1");
    let original_source = source_owner.get_transcript_entries().unwrap().remove(0);
    let admitted = source_owner.get_metadata("humanForwardBatches").unwrap().unwrap();
    let mut protected = original_source.clone(); protected["protectedContent"] = json!(true);
    source_owner.update_transcript_entry(args["sourceEntryId"].as_str().unwrap(), &protected).unwrap().unwrap();
    let retry_args = json!({"conversationId":first,"clientNonce":nonce});
    let denied = dispatch_production_session_gateway_call(&runtime, "retryHumanMessage", &retry_args)
        .expect("registered forward-owned retry").expect_err("source policy rejects retry");
    assert!(matches!(denied, SessionGatewayError::Internal(ref error) if error.contains("protected")));
    assert_eq!(server.post_count(), 1);
    assert_eq!(server.resource_post_count(), 1);
    assert_eq!(server.history_count(), 0);
    let journal = source_owner.get_metadata("humanForwardBatches").unwrap().unwrap();
    assert_forward_intent_unchanged(&admitted[0], &journal[0]);
    assert_eq!(journal[0]["destinations"][0]["completed"], false);
    assert!(journal[0]["destinations"][0]["error"].as_str().unwrap().contains("protected"));
    let denied_target = target_owner.get_transcript_entries().unwrap().remove(0);
    assert_eq!(denied_target["delivery"], "pending");
    assert!(denied_target["lastDispatchError"].as_str().unwrap().contains("protected"));
    assert!(denied_target["lastDispatchFailedAtMs"].is_number());
    let mut preserved = denied_target.clone();
    let mut expected = original.clone();
    for key in ["delivery", "lastDispatchError", "lastDispatchFailedAtMs"] {
        preserved.as_object_mut().unwrap().remove(key);
        expected.as_object_mut().unwrap().remove(key);
    }
    assert_eq!(preserved, expected, "source denial retains all original target intent");
    runtime.shutdown(); drop(target_owner); drop(source_owner); drop(runtime);
    let runtime = human_message_fixture_runtime(&root, &server.url);
    assert_eq!(runtime.read_human_conversation_transcript(first).unwrap(), vec![denied_target.clone()],
        "source-policy failure remains visible after reopen");
    let source_owner = runtime.open_human_conversation_db_owner(source).expect("reopened source owner");
    let target_owner = runtime.open_human_conversation_db_owner(first).expect("reopened target owner");
    assert_eq!(source_owner.get_metadata("humanForwardBatches").unwrap(), Some(journal.clone()));

    for label in ["missing-context", "unrelated-context", "future-version", "missing-version",
        "changed-request", "changed-source", "changed-entry", "markerless-lost-context"] {
        let mut damaged = denied_target.clone();
        match label {
            "missing-context" => { damaged.as_object_mut().unwrap().remove("forwardContext"); }
            "unrelated-context" => damaged["forwardContext"] = json!({"retained":"unknown context"}),
            "future-version" => damaged["forwardContext"]["batchVersion"] = json!(2),
            "missing-version" => { damaged["forwardContext"].as_object_mut().unwrap().remove("batchVersion"); }
            "changed-request" => damaged["forwardContext"]["requestId"] = json!("changed-forward-request"),
            "changed-source" => damaged["forwardContext"]["sourceConversationId"] = json!(second),
            "changed-entry" => damaged["forwardContext"]["sourceEntryId"] = json!("human-server-message:999"),
            "markerless-lost-context" => {
                damaged.as_object_mut().unwrap().remove("forwardContext");
                damaged.as_object_mut().unwrap().remove("forwardBatchOwned");
            }
            _ => unreachable!(),
        }
        target_owner.update_transcript_entry(original_id, &damaged).unwrap().unwrap();
        assert!(dispatch_production_session_gateway_call(&runtime, "retryHumanMessage", &retry_args)
            .expect("handled damaged forward retry").is_err(), "unresolved forward retry: {label}");
        assert!(dispatch_production_session_gateway_call(&runtime, "sendHumanMessage", &json!({
            "conversationId":first,"senderId":"1","clientNonce":nonce,
            "text":damaged["content"],"attachments":damaged["attachments"]
        })).expect("handled ordinary send").is_err(), "normal send cannot drop forward ownership: {label}");
        assert_eq!(target_owner.get_transcript_entries().unwrap(), vec![damaged], "retain damaged target: {label}");
        assert_eq!(source_owner.get_metadata("humanForwardBatches").unwrap(), Some(journal.clone()),
            "malformed target cannot rewrite source authorization: {label}");
        assert_eq!(server.post_count(), 1, "no generic message bypass: {label}");
        assert_eq!(server.resource_post_count(), 1, "no generic resource bypass: {label}");
        assert_eq!(server.history_count(), 0, "explicit retry does not replace itself with sync: {label}");
    }
    let mut unresolved = denied_target.clone();
    unresolved.as_object_mut().unwrap().remove("forwardContext");
    unresolved.as_object_mut().unwrap().remove("forwardBatchOwned");
    target_owner.update_transcript_entry(original_id, &unresolved).unwrap().unwrap();
    let source_path = forward_db_path(&root, source);
    let unavailable_path = source_path.with_extension("retry-unavailable");
    fs::rename(&source_path, &unavailable_path).expect("make only the original source store unavailable");
    let unresolved_retry = dispatch_production_session_gateway_call(&runtime, "retryHumanMessage", &retry_args);
    let unresolved_send = dispatch_production_session_gateway_call(&runtime, "sendHumanMessage", &json!({
        "conversationId":first,"senderId":"1","clientNonce":nonce,
        "text":unresolved["content"],"attachments":unresolved["attachments"]
    }));
    fs::rename(&unavailable_path, &source_path).expect("restore the exact retained source store");
    assert!(unresolved_retry.expect("handled unresolved retry").is_err());
    assert!(unresolved_send.expect("handled unresolved ordinary send").is_err());
    assert_eq!(target_owner.get_transcript_entries().unwrap(), vec![unresolved]);
    assert_eq!(source_owner.get_metadata("humanForwardBatches").unwrap(), Some(journal.clone()));
    assert_eq!(server.post_count(), 1); assert_eq!(server.resource_post_count(), 1);
    assert_eq!(server.history_count(), 0);
    target_owner.update_transcript_entry(original_id, &denied_target).unwrap().unwrap();
    let credential_path = root.join("host-credential.json");
    let credential = fs::read_to_string(&credential_path).unwrap();
    for (key, value) in [("userId", json!(9)), ("issuer", json!("http://127.0.0.1:1"))] {
        let mut changed: serde_json::Value = serde_json::from_str(&credential).unwrap();
        changed[key] = value;
        fs::write(&credential_path, changed.to_string()).unwrap();
        assert!(dispatch_production_session_gateway_call(&runtime, "retryHumanMessage", &retry_args)
            .expect("handled changed-scope retry").is_err(), "forward retry rejects changed {key}");
        assert_eq!(target_owner.get_transcript_entries().unwrap(), vec![denied_target.clone()]);
        assert_eq!(server.post_count(), 1); assert_eq!(server.resource_post_count(), 1);
    }
    fs::write(&credential_path, credential).unwrap();
    let mut edited_source = original_source;
    edited_source["content"] = json!("edited source after the original accepted batch");
    source_owner.update_transcript_entry(args["sourceEntryId"].as_str().unwrap(), &edited_source).unwrap().unwrap();
    fs::write(&files[0].1, b"changed mutable source cache").unwrap();
    assert_eq!(fs::read(&pins[0]).unwrap(), files[0].2, "original pinned bytes remain authoritative");
    let settled = dispatch(&runtime, "retryHumanMessage", retry_args.clone());
    assert_eq!(settled["delivery"], "sent");
    assert_eq!(settled["clientNonce"], nonce);
    assert_eq!(settled["content"], "immutable forwarded snapshot");
    assert_eq!(settled["forwardContext"], original["forwardContext"]);
    assert!(settled.get("lastDispatchError").is_none());
    assert!(settled.get("lastDispatchFailedAtMs").is_none());
    assert_eq!(settled["attachments"][0]["resourceId"], "forward-resource-1");
    assert_eq!(fs::read(settled["attachments"][0]["path"].as_str().unwrap()).unwrap(), files[0].2);
    assert_eq!(target_owner.get_transcript_entries().unwrap(), vec![settled.clone()]);
    assert!(runtime.read_human_conversation_transcript(second).unwrap().is_empty(),
        "one target retry cannot dispatch another original batch destination");
    let completed = source_owner.get_metadata("humanForwardBatches").unwrap().unwrap();
    assert_forward_intent_unchanged(&admitted[0], &completed[0]);
    assert_eq!(completed[0]["destinations"][0]["completed"], true);
    assert_eq!(completed[0]["destinations"][1]["completed"], false);
    for _ in 0..2 { assert_eq!(dispatch(&runtime, "retryHumanMessage", retry_args.clone()), settled); }
    runtime.shutdown(); drop(target_owner); drop(source_owner); drop(runtime);
    let reopened = human_message_fixture_runtime(&root, &server.url);
    assert_eq!(dispatch(&reopened, "retryHumanMessage", retry_args), settled);
    assert_eq!(reopened.read_human_conversation_transcript(first).unwrap(), vec![settled]);
    assert!(reopened.read_human_conversation_transcript(second).unwrap().is_empty());
    reopened.shutdown(); drop(reopened);
    {
        let state = server.state.lock().expect("forward retry HTTP oracle");
        assert_eq!(state.requests.len(), 4);
        assert!(state.requests[0].starts_with("POST /api/social/message-resources "));
        assert!(state.requests[1].starts_with("POST /api/social/messages "));
        assert!(state.requests[2].starts_with("POST /api/social/messages "));
        assert!(state.requests[3].starts_with("GET /api/social/message-resources/forward-resource-1 "));
        assert_eq!(fixture_request_json(&state.requests[1]), fixture_request_json(&state.requests[2]));
        assert_eq!(state.uploaded_files, vec![(files[0].0.clone(), files[0].2.clone())]);
        assert_eq!(state.accepted.len(), 1, "only the selected original nonce has one server message");
    }
    server.finish(); fs::remove_dir_all(root).expect("remove forward retry fixture");
}

#[test]
fn shipping_human_retry_fences_account_and_issuer_changes_and_restores_durable_failure() {
    for (field, replacement) in [("userId", json!(9)), ("issuer", json!("http://127.0.0.1:1"))] {
        let root = temp_root(&format!("human-retry-credential-{field}"));
        let nonce = format!("retry-credential-{field}");
        let remote = fixture_remote_human_message(4_500_200, 1, 2, "credential-bound retry", Some(&nonce), json!([]));
        let (seen, accepted) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let (url, server) = spawn_human_message_fixture(vec![
            HumanMessageFixtureReply::Deferred(Box::new(HumanMessageFixtureReply::Json(201,
                json!({"success":true,"message":remote.clone()}))), seen, resume),
            HumanMessageFixtureReply::Json(200, json!({"success":true,"deduplicated":true,"message":remote})),
        ]);
        let runtime = human_message_fixture_runtime(&root, &url);
        let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
            .as_str().expect("id").to_owned();
        let owner = runtime.open_human_conversation_db_owner(&id).expect("credential retry owner");
        let mut original = pending_reply_fixture_row(&nonce, "credential-bound retry", None);
        original["lastDispatchError"] = json!("earlier retained failure");
        original["lastDispatchFailedAtMs"] = json!(1200.0);
        assert!(owner.append_transcript_entry(&original).unwrap());
        let args = json!({"conversationId":id,"clientNonce":nonce});
        let credential_path = root.join("host-credential.json");
        let credential = fs::read_to_string(&credential_path).unwrap();
        let mut changed: serde_json::Value = serde_json::from_str(&credential).unwrap();
        changed[field] = replacement;
        fs::write(&credential_path, changed.to_string()).unwrap();
        assert!(dispatch_production_session_gateway_call(&runtime, "retryHumanMessage", &args)
            .expect("handled pre-dispatch credential rejection").is_err());
        assert!(matches!(accepted.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)),
            "changed {field} cannot reach the live HTTP fixture");
        assert_eq!(owner.get_transcript_entries().unwrap(), vec![original.clone()],
            "pre-dispatch scope rejection preserves existing failure: {field}");
        fs::write(&credential_path, &credential).unwrap();
        let retrying = {
            let runtime = Arc::clone(&runtime); let args = args.clone();
            thread::spawn(move || dispatch_production_session_gateway_call(&runtime, "retryHumanMessage", &args)
                .expect("handled admitted retry"))
        };
        accepted.recv_timeout(std::time::Duration::from_secs(15)).expect("retry request admitted under original account");
        fs::write(&credential_path, changed.to_string()).unwrap();
        release.send(()).expect("release response after credential replacement");
        let failure = retrying.join().expect("late retry response").expect_err("stale credential cannot settle retry");
        let error = match failure {
            SessionGatewayError::Internal(error) => error,
            other => panic!("unexpected admitted retry error: {other:?}"),
        };
        assert!(!error.is_empty());
        let failed = owner.get_transcript_entries().expect("durable rejected retry");
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0]["delivery"], "pending");
        assert_eq!(failed[0]["lastDispatchError"], error);
        assert!(failed[0]["lastDispatchFailedAtMs"].is_number());
        assert!(failed[0].get("remoteMessageId").is_none());
        let mut expected = original;
        expected["lastDispatchError"] = failed[0]["lastDispatchError"].clone();
        expected["lastDispatchFailedAtMs"] = failed[0]["lastDispatchFailedAtMs"].clone();
        assert_eq!(failed, vec![expected], "late response retains the complete durable intent: {field}");
        runtime.shutdown(); drop(owner); drop(runtime);

        let restarted = human_message_fixture_runtime(&root, &url);
        assert_eq!(restarted.read_human_conversation_transcript(&id).expect("restored visible failure data"), failed);
        let settled = dispatch(&restarted, "retryHumanMessage", args.clone());
        assert_eq!(settled["id"], "human-server-message:4500200");
        assert_eq!(settled["delivery"], "sent");
        assert_eq!(settled["clientNonce"], nonce);
        assert!(settled.get("lastDispatchError").is_none());
        assert!(settled.get("lastDispatchFailedAtMs").is_none());
        assert_eq!(dispatch(&restarted, "retryHumanMessage", args), settled);
        restarted.shutdown(); drop(restarted);
        let reopened = human_message_fixture_runtime(&root, &url);
        assert_eq!(reopened.read_human_conversation_transcript(&id).expect("final credential recovery reopen"), vec![settled]);
        reopened.shutdown(); drop(reopened);
        let requests = server.join().expect("credential-fenced retry fixture");
        assert_eq!(requests.len(), 2, "pre-admission refusal and canonical repeat add no HTTP: {field}");
        assert!(requests.iter().all(|request| request.starts_with("POST /api/social/messages ")));
        assert_eq!(fixture_request_json(&requests[0]), fixture_request_json(&requests[1]),
            "restored account retries the original request: {field}");
        assert_eq!(fixture_request_json(&requests[1])["clientRequestId"], nonce);
        let _ = fs::remove_dir_all(root);
    }
}

fn retained_admission_files(
    root: &std::path::Path,
) -> std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>> {
    fn visit(
        root: &std::path::Path,
        directory: &std::path::Path,
        files: &mut std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>>,
    ) {
        for entry in fs::read_dir(directory).expect("retained fixture directory") {
            let entry = entry.expect("retained fixture entry");
            let path = entry.path();
            let relative = path.strip_prefix(root).expect("fixture path is scoped").to_path_buf();
            if entry.file_type().expect("fixture file type").is_dir() {
                files.insert(relative, None);
                visit(root, &path, files);
            } else {
                files.insert(relative, Some(fs::read(path).expect("retained fixture bytes")));
            }
        }
    }
    let mut files = std::collections::BTreeMap::new();
    visit(root, root, &mut files);
    files
}

#[test]
fn shipping_human_forward_recovery_reports_storage_failures_and_preserves_account_policy() {
    let root = temp_root("forward-strict-recovery");
    let mut server = ForwardHttpFixture::start(true);
    let args = prepare_forward_process(&root, &server.url);
    let mut child = ForwardProcess::start(&root, &server.url);
    server.wait_for_first_post(); child.terminate(); server.release_first_post();
    let runtime = human_message_fixture_runtime(&root, &server.url);
    let source = args["sourceConversationId"].as_str().unwrap();
    let second = args["destinationConversationIds"][1].as_str().unwrap();
    let source_owner = runtime.open_human_conversation_db_owner(source).unwrap();
    assert!(source_owner.set_metadata("participantIds", json!(["2", "9"])).unwrap());

    // A real second-account owner writes a valid incomplete batch through the
    // same Session API. Its journal must remain untouched by account 1 recovery.
    let agents = root.join("agents");
    let other = ProductionSessionWorkers::with_agents_root_identity_and_dependencies(
        agents.clone(), 500, Some("9".into()), Arc::new(|| None),
        Arc::new(mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(agents)),
    );
    let other_source = other.create_human_conversation("8", "Other account source").unwrap()["id"].as_str().unwrap().to_string();
    let other_entry = other.append_human_message(&other_source, "other account intent", "other-source", None, None, &[]).unwrap();
    let other_result = other.forward_human_message(
        &other_source, other_entry["id"].as_str().unwrap(),
        &["human-direct-uncreated-target".to_string()], "other-account-forward", false, false,
    ).unwrap();
    assert_eq!(other_result["destinations"][0]["status"], "failed");
    let other_metadata = read_forward_metadata_from_disk(&root, &other_source);
    other.shutdown(); drop(other);

    runtime.sync_human_conversation(second).expect("revoked source must not starve unrelated history");
    assert_eq!(server.history_count(), 1);
    assert_eq!(server.post_count(), 1, "revoked source and another account never dispatch");
    let denied = source_owner.get_metadata("humanForwardBatches").unwrap().unwrap();
    assert!(denied[0]["destinations"].as_array().unwrap().iter().all(|destination|
        destination["completed"] == false
            && destination["error"].as_str().is_some_and(|error| error.contains("participant"))
    ), "ordinary sync durably records denial even though source membership was removed");
    assert_eq!(read_forward_metadata_from_disk(&root, &other_source), other_metadata);
    let candidate = runtime.create_human_conversation("5", "Another valid source").unwrap()["id"]
        .as_str().unwrap().to_string();
    let candidate_owner = runtime.open_human_conversation_db_owner(&candidate).unwrap();
    assert!(candidate_owner.append_transcript_entry(&json!({
        "id":"human-server-message:52","kind":"message","role":"user",
        "authorKind":"human","authorId":"5","content":"different source payload",
        "delivery":"sent","timestampMs":1000.0,"remoteMessageId":"52",
        "remoteCreatedAt":"1970-01-01T00:00:01Z"
    })).unwrap());
    let candidate_metadata = candidate_owner.read_metadata().unwrap();
    assert!(candidate_metadata.get("humanForwardBatches").is_none());
    assert!(runtime.search_human_recipients(&candidate, "human-server-message:52", "", 50)
        .expect("candidate source is eligible under the real permission path")
        .iter().any(|conversation| conversation["id"] == second));
    let mut rebound = args.clone();
    rebound["sourceConversationId"] = json!(candidate);
    rebound["sourceEntryId"] = json!("human-server-message:52");
    assert!(forward_gateway_result(&runtime, &rebound).is_err(), "existing identity cannot bind another source");
    let roster = runtime.list_human_conversations().unwrap();
    assert!(roster.iter().any(|conversation| conversation["id"] == second));
    assert!(!roster.iter().any(|conversation| conversation["id"] == source || conversation["id"] == other_source));

    // A cached Arc is not an exemption from persisted identity checks. Keep
    // the same live owner and restore its original value after each refusal.
    let source_metadata = source_owner.read_metadata().unwrap();
    let canonical_raw = source_owner.read_kv("metadata").unwrap().unwrap();
    let mut invalid_participants = source_metadata.clone();
    invalid_participants["participantIds"] = json!(["2", "2"]);
    let invalid_pair_hex = invalid_participants.to_string().as_bytes().iter()
        .map(|byte| format!("{byte:02x}")).collect::<String>();
    for invalid_raw in [Some("🙂".to_string()), Some(invalid_pair_hex), None] {
        match invalid_raw {
            Some(raw) => assert!(source_owner.write_kv("metadata", &raw).unwrap()),
            None => assert!(source_owner.delete_kv("metadata").unwrap()),
        }
        let retained = source_owner.read_kv("metadata").unwrap();
        let history_before = server.history_count();
        assert!(runtime.open_human_conversation_db_owner(source).is_err());
        assert!(runtime.sync_human_conversation(second).is_err());
        assert!(forward_gateway_result(&runtime, &rebound).is_err());
        assert_eq!(server.history_count(), history_before);
        assert_eq!(server.post_count(), 1);
        assert_eq!(source_owner.read_kv("metadata").unwrap(), retained);
        assert!(!source_owner.is_closed(), "failed validation must retain the cached handle");
        assert!(source_owner.write_kv("metadata", &canonical_raw).unwrap());
        assert!(runtime.open_human_conversation_db_owner(source).is_err(),
            "restored structural metadata still has a revoked canonical pair");
        assert!(!source_owner.is_closed(), "ordinary refusal preserves the retained recovery handle");
        assert_eq!(source_owner.read_metadata().unwrap(), source_metadata);
    }
    runtime.shutdown(); drop(source_owner); drop(candidate_owner); drop(runtime); drop(child);

    let path = forward_db_path(&root, source);
    let directory = path.parent().unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    drop(db);
    let healthy = fs::read(&path).unwrap();
    for failure in ["invalid-metadata", "non-ascii-metadata", "missing-metadata", "missing-db", "nonregular-db", "corrupt-sqlite"] {
        match failure {
            "invalid-metadata" | "non-ascii-metadata" | "missing-metadata" => {
                let db = rusqlite::Connection::open(&path).unwrap();
                if failure == "missing-metadata" {
                    db.execute("DELETE FROM kv WHERE key = 'metadata'", []).unwrap();
                } else {
                    let raw = if failure == "invalid-metadata" { "7b" } else { "🙂" };
                    db.execute("UPDATE kv SET value = ?1 WHERE key = 'metadata'", [raw]).unwrap();
                }
                drop(db);
            }
            "missing-db" => { fs::rename(&path, directory.join("store.db.retained")).unwrap(); }
            "nonregular-db" => {
                fs::rename(&path, directory.join("store.db.retained")).unwrap();
                fs::create_dir(&path).unwrap();
            }
            _ => { fs::write(&path, b"retained corrupt source bytes").unwrap(); }
        }
        // This bounded oracle does not feed arbitrary retained WAL/journal
        // bytes to SQLite. The inherited Human opener probes this cold main
        // file before configuring it, with corruption recovery disabled.
        for suffix in ["-wal", "-shm", "-journal"] {
            assert!(!directory.join(format!("store.db{suffix}")).exists(),
                "the {failure} fixture is closed and has no retained sidecar");
        }
        let before = retained_admission_files(directory);
        let history_before = server.history_count();
        let runtime = human_message_fixture_runtime(&root, &server.url);
        assert!(runtime.list_human_conversations().unwrap().iter().any(|conversation| conversation["id"] == second),
            "ordinary roster remains compatible when one source is unreadable");
        assert!(runtime.sync_human_conversation(second).is_err(), "recovery must report {failure}");
        assert!(forward_gateway_result(&runtime, &rebound).is_err(),
            "{failure} must not free the original requestId for a different valid source");
        assert_eq!(read_forward_metadata_from_disk(&root, &candidate), candidate_metadata,
            "failed identity discovery cannot stage a replacement source journal");
        assert_eq!(server.history_count(), history_before, "no later history after failed recovery enumeration");
        assert_eq!(server.post_count(), 1, "no dispatch after failed recovery enumeration");
        runtime.shutdown(); drop(runtime);
        assert_eq!(retained_admission_files(directory), before, "failed recovery preserves {failure}");
        if failure == "nonregular-db" {
            fs::remove_dir(&path).unwrap();
        }
        if failure == "missing-db" || failure == "nonregular-db" {
            fs::rename(directory.join("store.db.retained"), &path).unwrap();
        } else {
            fs::write(&path, &healthy).unwrap();
        }
        let resumed = human_message_fixture_runtime(&root, &server.url);
        resumed.sync_human_conversation(second).expect("retry after restoring retained source");
        assert_eq!(server.history_count(), history_before + 1);
        assert_eq!(server.post_count(), 1);
        assert_eq!(read_forward_metadata_from_disk(&root, &other_source), other_metadata);
        resumed.shutdown(); drop(resumed);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        for directory_alias in [true, false] {
            let original = if directory_alias { directory.to_path_buf() } else { path.clone() };
            // The retained directory must be outside the discovery root so a
            // second real directory cannot mask a skipped source alias.
            let retained = if directory_alias {
                root.join("retained-forward-source")
            } else {
                directory.join("store.db.retained")
            };
            fs::rename(&original, &retained).unwrap();
            symlink(&retained, &original).unwrap();
            let snapshot_root = if directory_alias { retained.as_path() } else { directory };
            let before = retained_admission_files(snapshot_root);
            let history_before = server.history_count();
            let runtime = human_message_fixture_runtime(&root, &server.url);
            assert!(runtime.open_human_conversation_db_owner(source).is_err());
            assert!(runtime.sync_human_conversation(second).is_err(),
                "source aliases must be rejected by complete recovery discovery");
            assert!(forward_gateway_result(&runtime, &rebound).is_err(),
                "an aliased source cannot free its durable request identity");
            assert_eq!(read_forward_metadata_from_disk(&root, &candidate), candidate_metadata);
            assert_eq!(server.history_count(), history_before);
            assert_eq!(server.post_count(), 1);
            runtime.shutdown(); drop(runtime);
            assert!(fs::symlink_metadata(&original).unwrap().file_type().is_symlink());
            assert_eq!(fs::read_link(&original).unwrap(), retained);
            assert_eq!(retained_admission_files(snapshot_root), before);
            fs::remove_file(&original).unwrap();
            fs::rename(&retained, &original).unwrap();
            let resumed = human_message_fixture_runtime(&root, &server.url);
            resumed.sync_human_conversation(second).expect("restored real source resumes recovery");
            assert_eq!(server.history_count(), history_before + 1);
            assert_eq!(server.post_count(), 1);
            assert_eq!(read_forward_metadata_from_disk(&root, &other_source), other_metadata);
            resumed.shutdown(); drop(resumed);
        }
    }
    server.finish(); fs::remove_dir_all(root).unwrap();
}

// NMR-50a/b: the real Host owns recovery; this loopback fixture only supplies
// controlled service responses and observes the actual HTTP request stream.
struct Nmr50ForwardRecoveryFixture {
    url: String,
    requests: Arc<std::sync::Mutex<Vec<String>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    first_post: std::sync::mpsc::Receiver<()>,
    release: Option<std::sync::mpsc::Sender<()>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Nmr50ForwardRecoveryFixture {
    fn start(hold_first_post: bool, mut rejected_posts: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("NMR50 listener");
        listener.set_nonblocking(true).expect("bounded NMR50 listener");
        let url = format!("http://{}", listener.local_addr().expect("NMR50 address"));
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (seen, first_post) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let observed = Arc::clone(&requests);
        let stopped = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let mut held_release = hold_first_post.then_some(released);
            let mut held_writer = None;
            let mut accepted: Option<serde_json::Value> = None;
            while !stopped.load(std::sync::atomic::Ordering::Acquire) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(std::time::Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("NMR50 accept: {error}"),
                };
                stream.set_nonblocking(false).expect("blocking NMR50 stream");
                stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))
                    .expect("bounded NMR50 response");
                let request = read_fixture_http_request_before(&mut stream,
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(10)));
                assert!(request.to_ascii_lowercase().contains("authorization: bearer fixture-token"));
                observed.lock().expect("NMR50 requests").push(request.clone());
                let path = request.split_whitespace().nth(1).expect("NMR50 request path");
                let is_post = request.starts_with("POST /api/social/messages ");
                let (status, body) = if is_post {
                    let submitted = fixture_request_json(&request);
                    assert_eq!(submitted["targetUserId"], "3");
                    if rejected_posts > 0 {
                        rejected_posts -= 1;
                        (503, json!({"success":false,"error":"NMR50 temporary delivery rejection"}))
                    } else {
                        let nonce = submitted["clientRequestId"].as_str().expect("durable forward nonce");
                        let text = submitted["text"].as_str().expect("durable forward text");
                        let message = accepted.get_or_insert_with(||
                            fixture_remote_human_message(70, 1, 3, text, Some(nonce), json!([]))
                        ).clone();
                        assert_eq!(message["clientRequestId"], nonce);
                        assert_eq!(message["text"], text);
                        (200, json!({"success":true,"message":message}))
                    }
                } else if request.starts_with("GET /api/social/friends ") {
                    (200, json!({"success":true,"data":{"friends":[
                        {"id":3,"userId":3,"username":"target","displayName":"Target","status":"accepted","avatarUrl":null},
                        {"id":4,"userId":4,"username":"unrelated-four","displayName":"Four","status":"accepted","avatarUrl":null},
                        {"id":5,"userId":5,"username":"unrelated-five","displayName":"Five","status":"accepted","avatarUrl":null}
                    ]}}))
                } else if request.starts_with("GET /api/social/calls?limit=200 ") {
                    (200, json!({"success":true,"calls":[]}))
                } else {
                    assert!(request.starts_with("GET /api/social/messages?"), "{request}");
                    let query = path.split_once('?').expect("history query").1;
                    let field = |name: &str| query.split('&').find_map(|part| {
                        let (key, value) = part.split_once('=')?;
                        (key == name).then_some(value)
                    }).expect("required history query field");
                    let peer = field("contactId").parse::<u64>().expect("numeric history peer");
                    let after = field("afterId").parse::<u64>().expect("coverage cursor");
                    let message = if peer == 3 {
                        accepted.clone()
                    } else {
                        Some(fixture_remote_human_message(10 + peer, peer, 1,
                            &format!("independent history {peer}"), None, json!([])))
                    };
                    let messages = message.into_iter().filter(|message|
                        message["id"].as_u64().expect("numeric message id") > after
                    ).collect::<Vec<_>>();
                    let next = messages.last().map(|message| message["id"].clone()).unwrap_or(json!(after));
                    (200, json!({"success":true,"data":{"messages":messages,
                        "historyCursor":{"mode":"after-id","afterId":after},"nextAfterId":next}}))
                };
                let mut write_response = move || {
                    let body = body.to_string();
                    write!(stream,
                        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()).expect("NMR50 headers");
                    stream.write_all(body.as_bytes()).expect("NMR50 response body");
                    stream.flush().expect("NMR50 response flush");
                };
                if is_post {
                    if let Some(released) = held_release.take() {
                        seen.send(()).expect("observe held NMR50 POST");
                        // Keep accepting history while only this response waits.
                        held_writer = Some(thread::spawn(move || {
                            released.recv_timeout(std::time::Duration::from_secs(30))
                                .expect("release the owned NMR50 response barrier");
                            write_response();
                        }));
                        continue;
                    }
                }
                write_response();
            }
            if let Some(writer) = held_writer {
                writer.join().expect("owned NMR50 response writer");
            }
        });
        Self { url, requests, stop, first_post, release: Some(release), worker: Some(worker) }
    }

    fn release_first_post(&mut self) {
        if let Some(release) = self.release.take() { let _ = release.send(()); }
    }

    fn request_count(&self, prefix: &str) -> usize {
        self.requests.lock().expect("NMR50 observed requests").iter()
            .filter(|request| request.starts_with(prefix)).count()
    }

    fn finish(mut self) {
        self.release_first_post();
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        self.worker.take().expect("owned NMR50 fixture worker").join().expect("NMR50 fixture");
    }
}

impl Drop for Nmr50ForwardRecoveryFixture {
    fn drop(&mut self) {
        self.release_first_post();
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
    }
}

#[test]
fn shipping_human_forward_active_batch_does_not_block_destination_or_unrelated_history() {
    let root = temp_root("nmr50-active-forward-history");
    let mut server = Nmr50ForwardRecoveryFixture::start(true, 0);
    let mut args = prepare_forward_process(&root, &server.url);
    let source = args["sourceConversationId"].as_str().unwrap().to_string();
    let destination = args["destinationConversationIds"][0].as_str().unwrap().to_string();
    let unrelated = args["destinationConversationIds"][1].as_str().unwrap().to_string();
    args["destinationConversationIds"] = json!([destination.clone()]);
    let runtime = human_message_fixture_runtime(&root, &server.url);
    let source_owner = runtime.open_human_conversation_db_owner(&source).expect("source owner");
    let forwarding = {
        let runtime = Arc::clone(&runtime);
        let args = args.clone();
        thread::spawn(move || forward_gateway_result(&runtime, &args))
    };
    if let Err(error) = server.first_post.recv_timeout(std::time::Duration::from_secs(15)) {
        server.release_first_post();
        let result = forwarding.join();
        panic!("forward never reached its response barrier: {error:?}; {result:?}");
    }
    let journal_before = source_owner.get_metadata("humanForwardBatches");
    let start_history = |id: String| {
        let runtime = Arc::clone(&runtime);
        let (done, result) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            let result = runtime.sync_human_conversation(&id);
            let _ = done.send(result);
        });
        (worker, result)
    };
    let (unrelated_worker, unrelated_done) = start_history(unrelated.clone());
    let (destination_worker, destination_done) = start_history(destination.clone());
    let unrelated_before_release = unrelated_done.recv_timeout(std::time::Duration::from_secs(8));
    let destination_before_release = destination_done.recv_timeout(std::time::Duration::from_secs(8));
    let forward_still_waiting = !forwarding.is_finished();
    let journal_after = source_owner.get_metadata("humanForwardBatches");
    let posts_before_release = server.request_count("POST /api/social/messages ");
    let histories_before_release = server.request_count("GET /api/social/messages?");
    // Release before any assertion or join, including the pre-fix timeout path.
    server.release_first_post();
    let forwarded = forwarding.join().expect("explicit forward thread");
    unrelated_worker.join().expect("unrelated history thread");
    destination_worker.join().expect("destination history thread");

    let unrelated_rows = unrelated_before_release.expect("unrelated history must finish before the POST response")
        .expect("unrelated history succeeds");
    let destination_rows = destination_before_release.expect("destination history must finish before the POST response")
        .expect("destination history succeeds");
    assert!(forward_still_waiting, "history cannot rely on the original HTTP request ending first");
    assert_eq!(posts_before_release, 1, "automatic recovery skips the active batch without another POST");
    assert_eq!(histories_before_release, 2, "both histories make their actual service request");
    let journal_before = journal_before.expect("initial durable journal").expect("accepted batch");
    assert_eq!(journal_before[0]["destinations"][0]["completed"], false);
    assert_eq!(journal_after.expect("journal while the response is held"), Some(journal_before.clone()),
        "skipping the busy batch never mutates its progress");
    assert_eq!(unrelated_rows.len(), 1);
    assert_eq!(unrelated_rows[0]["content"], "independent history 4");
    assert_eq!(destination_rows.len(), 1);
    assert_eq!(destination_rows[0]["id"], "human-server-message:70");
    assert_eq!(destination_rows[0]["delivery"], "sent");
    assert_eq!(destination_rows[0]["clientNonce"], journal_before[0]["destinations"][0]["clientNonce"]);
    assert_eq!(forwarded.expect("late acknowledgement uses the canonical settlement")["destinations"][0]["status"], "sent");
    assert_forward_settled(&runtime, &args);
    assert_eq!(runtime.read_human_conversation_transcript(&destination).unwrap(), destination_rows);
    runtime.shutdown(); drop(source_owner); drop(runtime);

    let restarted = human_message_fixture_runtime(&root, &server.url);
    assert_eq!(restarted.sync_human_conversation(&destination).expect("restart retains canonical history"), destination_rows);
    assert_forward_settled(&restarted, &args);
    assert_eq!(server.request_count("POST /api/social/messages "), 1, "completed restart never resends");
    restarted.shutdown(); drop(restarted);
    server.finish(); fs::remove_dir_all(root).expect("remove NMR50 active history fixture");
}

#[test]
fn shipping_human_roster_recovery_attempts_failed_forward_once_per_invocation_and_reopens() {
    let root = temp_root("nmr50-roster-forward-once");
    let server = Nmr50ForwardRecoveryFixture::start(false, 2);
    let mut args = prepare_forward_process(&root, &server.url);
    let destination = args["destinationConversationIds"][0].clone();
    args["destinationConversationIds"] = json!([destination]);
    let source = args["sourceConversationId"].as_str().unwrap().to_string();
    let runtime = human_message_fixture_runtime(&root, &server.url);
    let first = forward_gateway_result(&runtime, &args).expect("initial durable failure");
    assert_eq!(first["destinations"][0]["status"], "failed");
    assert_eq!(server.request_count("POST /api/social/messages "), 1);
    let owner = runtime.open_human_conversation_db_owner(&source).expect("source owner");
    let before = owner.get_metadata("humanForwardBatches").unwrap().unwrap();

    let conversations = runtime.sync_human_conversations().expect("failed forward must not starve the roster");
    assert_eq!(server.request_count("POST /api/social/messages "), 2,
        "one top-level roster call retries once, regardless of its three friends");
    assert_eq!(server.request_count("GET /api/social/friends "), 1);
    assert_eq!(server.request_count("GET /api/social/messages?"), 3);
    assert_eq!(server.request_count("GET /api/social/calls?limit=200 "), 1);
    let pending = owner.get_metadata("humanForwardBatches").unwrap().unwrap();
    for field in ["requestId", "text", "attachments", "context"] {
        assert_eq!(pending[0][field], before[0][field], "failed recovery retains {field}");
    }
    assert_eq!(pending[0]["destinations"][0]["completed"], false);
    assert_eq!(pending[0]["destinations"][0]["clientNonce"], before[0]["destinations"][0]["clientNonce"]);
    assert!(pending[0]["destinations"][0]["error"].as_str().unwrap().contains("NMR50 temporary delivery rejection"));
    for peer in ["4", "5"] {
        let id = conversations.iter().find(|item| item["participantIds"].as_array().unwrap()
            .iter().any(|participant| participant.as_str() == Some(peer))).expect("unrelated friend discovered")["id"]
            .as_str().unwrap();
        let rows = runtime.read_human_conversation_transcript(id).expect("unrelated history advanced");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["content"], format!("independent history {peer}"));
    }
    runtime.shutdown(); drop(owner); drop(runtime);

    let restarted = human_message_fixture_runtime(&root, &server.url);
    let owner = restarted.open_human_conversation_db_owner(&source).expect("reopen failed journal");
    assert_eq!(owner.get_metadata("humanForwardBatches").unwrap(), Some(pending));
    restarted.sync_human_conversations().expect("next invocation recovers the durable target");
    assert_eq!(server.request_count("POST /api/social/messages "), 3, "next invocation adds exactly one recovery POST");
    assert_forward_settled(&restarted, &args);
    let requests = server.requests.lock().unwrap();
    let posts = requests.iter().filter(|request| request.starts_with("POST /api/social/messages "))
        .map(|request| fixture_request_json(request)).collect::<Vec<_>>();
    assert_eq!(posts.len(), 3);
    assert!(posts.iter().all(|post| post == &posts[0]), "every retry uses the original destination nonce and payload");
    drop(requests);
    restarted.shutdown(); drop(owner); drop(restarted);
    let reopened = human_message_fixture_runtime(&root, &server.url);
    assert_forward_settled(&reopened, &args);
    assert_eq!(server.request_count("POST /api/social/messages "), 3, "pure reopen never dispatches");
    reopened.shutdown(); drop(reopened);
    server.finish(); fs::remove_dir_all(root).expect("remove NMR50 roster recovery fixture");
}

fn spawn_streaming_human_roster_fixture() -> (
    String,
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::Sender<()>,
    std::sync::mpsc::Sender<()>,
    thread::JoinHandle<Vec<String>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind streaming roster fixture");
    listener.set_nonblocking(true).expect("nonblocking roster fixture");
    let url = format!("http://{}", listener.local_addr().expect("roster fixture address"));
    let (body_started, wait_for_body) = std::sync::mpsc::channel();
    let (release_body, wait_for_release) = std::sync::mpsc::channel();
    let (stop, stopped) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let mut requests = Vec::new();
        loop {
            assert!(std::time::Instant::now() < deadline, "streaming roster fixture was not stopped");
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).expect("blocking roster response");
                    stream.set_write_timeout(Some(std::time::Duration::from_secs(15)))
                        .expect("bound roster response write");
                    requests.push(read_fixture_http_request_before(&mut stream, Some(deadline)));
                    if requests.len() == 1 {
                        let body = json!({"success":true,"data":{"friends":[{
                            "id":3,"userId":3,"username":"obsolete-peer",
                            "displayName":"Obsolete roster","avatarUrl":null,"status":"accepted"
                        }]}}).to_string();
                        write!(stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len(),
                        ).expect("stream roster headers");
                        stream.write_all(&body.as_bytes()[..1]).expect("first roster body byte");
                        stream.flush().expect("flush incomplete roster body");
                        body_started.send(()).expect("report incomplete roster response");
                        wait_for_release.recv_timeout(std::time::Duration::from_secs(15))
                            .expect("release roster body after credential rotation");
                        if let Err(error) = stream.write_all(&body.as_bytes()[1..]).and_then(|_| stream.flush()) {
                            assert!(matches!(error.kind(),
                                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                            ), "unexpected roster fixture write failure: {error}");
                        }
                    } else {
                        // Keep the endpoint live until the Host returns, so a
                        // downstream history/call attempt is recorded, not hidden
                        // by a dropped listener or a connection-refused error.
                        let body = json!({"success":false,"error":"unexpected downstream roster request"}).to_string();
                        write!(stream,
                            "HTTP/1.1 503 Unexpected\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(), body,
                        ).expect("answer unexpected downstream request");
                        stream.flush().expect("flush unexpected downstream response");
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    match stopped.recv_timeout(std::time::Duration::from_millis(5)) {
                        Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
                Err(error) => panic!("streaming roster accept: {error}"),
            }
        }
        requests
    });
    (url, wait_for_body, release_body, stop, server)
}

#[test]
fn shipping_human_roster_binds_active_user_before_http_and_preserves_compatible_api() {
    let root = temp_root("human-roster-bound-user");
    let (url, stop, server) = spawn_human_no_request_fixture();
    let runtime = human_message_fixture_runtime(&root, &url);
    let retained = runtime.create_human_conversation("2", "Retained peer").expect("retained conversation");
    let id = retained["id"].as_str().expect("retained id");
    let owner = runtime.open_human_conversation_db_owner(id).expect("retained owner");
    let metadata = owner.read_metadata().expect("metadata before refusal");
    let before = runtime.list_human_conversations().expect("roster before refusal");
    let credential_path = root.join("host-credential.json");
    let original = fs::read_to_string(&credential_path).expect("original credential");
    let mut changed: serde_json::Value = serde_json::from_str(&original).expect("credential JSON");
    changed["userId"] = json!(9);
    fs::write(&credential_path, changed.to_string()).expect("replace active credential");
    let client = FabushiNativeMessagingClient::new(&url, &credential_path).expect("bound roster client");
    let bound_error = client.list_friends_for_user("1").expect_err("wrong user before GET");
    let host_error = runtime.sync_human_conversations().expect_err("wrong user before roster effects");
    stop.send(()).expect("stop live no-request listener");
    let requests = server.join().expect("no-request roster fixture");
    assert!(bound_error.contains("changed Human account"), "{bound_error}");
    assert!(host_error.contains("does not match active Human identity"), "{host_error}");
    assert!(requests.is_empty(), "wrong-user admission must issue no HTTP");
    assert_eq!(runtime.list_human_conversations().unwrap(), before);
    assert_eq!(owner.read_metadata().unwrap(), metadata);
    assert!(owner.get_transcript_entries().unwrap().is_empty());
    runtime.shutdown(); drop(owner); drop(runtime);

    let (restored_url, restored_server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"friends":[]}})),
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"friends":[]}})),
    ]);
    let reopened = human_message_fixture_runtime(&root, &restored_url);
    let restored_client = FabushiNativeMessagingClient::new(&restored_url, &credential_path)
        .expect("restored roster client");
    assert!(restored_client.list_friends().expect("compatible existing API").is_empty());
    assert!(restored_client.list_friends_for_user("1").expect("bound current account").is_empty());
    assert_eq!(reopened.list_human_conversations().unwrap(), before);
    let requests = restored_server.join().expect("both compatible roster calls");
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| request.starts_with("GET /api/social/friends ")));
    reopened.shutdown(); drop(reopened);
    fs::remove_dir_all(root).expect("remove bound roster fixture");
}

#[test]
fn shipping_human_roster_rejects_six_streaming_credential_rotations_before_effects_and_recovers() {
    for (index, (field, replacement)) in [
        ("userId", json!(9)),
        ("deviceId", json!("rotated-roster-device")),
        ("accessToken", json!("rotated-roster-token")),
        ("sessionId", json!("rotated-roster-session")),
        ("expiresAtMs", json!(12345)),
        ("issuer", json!("http://127.0.0.1:1")),
    ].into_iter().enumerate() {
        let root = temp_root(&format!("human-roster-streaming-{field}"));
        let (url, wait_for_body, release_body, stop, server) = spawn_streaming_human_roster_fixture();
        let runtime = human_message_fixture_runtime(&root, &url);
        let id = runtime.create_human_conversation("2", "Retained peer").expect("retained conversation")["id"]
            .as_str().expect("retained id").to_owned();
        let owner = runtime.open_human_conversation_db_owner(&id).expect("retained owner");
        let retained_entry = human_local_history_message(9_100_100 + index as u64);
        assert!(owner.append_transcript_entry(&retained_entry).expect("retained message"));
        let metadata = owner.read_metadata().expect("original metadata");
        let coverage = owner.get_metadata("humanHistoryCoverage").expect("original history coverage");
        let before = runtime.list_human_conversations().expect("original roster");
        let (events, subscription) = record_human_target_events(
            &id, retained_entry["id"].as_str().expect("retained entry id"),
        );
        let syncing = {
            let runtime = Arc::clone(&runtime);
            thread::spawn(move || runtime.sync_human_conversations())
        };
        let started = wait_for_body.recv_timeout(std::time::Duration::from_secs(15));
        let credential_path = root.join("host-credential.json");
        let rotation = (|| -> Result<(), String> {
            let mut credential: serde_json::Value = serde_json::from_str(
                &fs::read_to_string(&credential_path).map_err(|error| error.to_string())?
            ).map_err(|error| error.to_string())?;
            credential[field] = replacement;
            fs::write(&credential_path, credential.to_string()).map_err(|error| error.to_string())
        })();
        // Release on every assertion path before joining the request worker.
        let released = release_body.send(());
        let result = syncing.join();
        let stopped = stop.send(());
        let requests = server.join();
        started.expect("roster headers sent while body remains incomplete");
        rotation.expect("rotate exactly one credential field");
        released.expect("release final roster body");
        let error = result.expect("roster worker").expect_err("retired roster must be rejected");
        stopped.expect("stop downstream request observer");
        let requests = requests.expect("streaming roster fixture");
        assert!(error.contains("credential"), "{field}: {error}");
        assert_eq!(requests.len(), 1, "no downstream history/call/resource HTTP: {field}");
        assert!(requests[0].starts_with("GET /api/social/friends "), "{field}");
        assert_eq!(runtime.list_human_conversations().unwrap(), before, "no obsolete conversation: {field}");
        assert_eq!(owner.read_metadata().unwrap(), metadata, "no metadata mutation: {field}");
        assert_eq!(owner.get_metadata("humanHistoryCoverage").unwrap(), coverage, "no history cursor advance: {field}");
        assert_eq!(owner.get_transcript_entries().unwrap(), vec![retained_entry.clone()], "no transcript mutation: {field}");
        assert!(events.lock().unwrap().is_empty(), "no retained message event: {field}");
        drop(subscription);
        runtime.shutdown(); drop(owner); drop(runtime);

        let restored_message_id = 101 + index as u64;
        let restored_message = fixture_remote_human_message(
            restored_message_id, 4, 1, "authorized roster history", None, json!([]),
        );
        let (restored_url, restored_server) = spawn_human_message_fixture(vec![
            HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"friends":[{
                "id":4,"userId":4,"username":"restored-peer",
                "displayName":"Authorized roster","avatarUrl":null,"status":"accepted"
            }]}})),
            human_history_reply(vec![restored_message]),
            HumanMessageFixtureReply::Json(200, json!({"success":true,"calls":[]})),
        ]);
        let restored = human_message_fixture_runtime(&root, &restored_url);
        assert_eq!(restored.list_human_conversations().unwrap(), before, "restart retained original roster: {field}");
        let authorized = restored.sync_human_conversations().expect("restored account roster sync");
        assert_eq!(authorized.len(), 2, "only retained and newly authorized peer: {field}");
        assert!(authorized.iter().all(|conversation| !conversation["participantIds"].as_array().unwrap().contains(&json!("3"))),
            "obsolete roster never becomes durable: {field}");
        let new_id = authorized.iter().find(|conversation| {
            conversation["participantIds"].as_array().unwrap().contains(&json!("4"))
        }).expect("authorized conversation")["id"].as_str().unwrap().to_owned();
        let rows = restored.read_human_conversation_transcript(&new_id).expect("authorized history");
        assert_eq!(rows.len(), 1, "{field}");
        assert_eq!(rows[0]["id"], format!("human-server-message:{restored_message_id}"), "{field}");
        assert_eq!(rows[0]["content"], "authorized roster history", "{field}");
        assert_eq!(restored.read_human_conversation_transcript(&id).unwrap(), vec![retained_entry], "{field}");
        let requests = restored_server.join().expect("authorized roster/history/call fixture");
        assert_eq!(requests.len(), 3, "{field}");
        assert!(requests[0].starts_with("GET /api/social/friends "));
        assert!(requests[1].starts_with("GET /api/social/messages?contactId=4&limit=200&afterId=0 "));
        assert!(requests[2].starts_with("GET /api/social/calls?limit=200 "));
        restored.shutdown(); drop(restored);
        let reopened = human_message_fixture_runtime(&root, &restored_url);
        assert_eq!(reopened.list_human_conversations().unwrap(), authorized, "authorized roster persists: {field}");
        assert_eq!(reopened.read_human_conversation_transcript(&new_id).unwrap(), rows, "authorized history persists: {field}");
        reopened.shutdown(); drop(reopened);
        fs::remove_dir_all(root).expect("remove streaming roster fixture");
    }
}


// NMR-FEED-01: this is the shared extension constructor used by the production
// bootstrap, with the same Host bus receiver that supplies Gateway SSE.
fn start_human_transcript_feed(
    root: &std::path::Path,
    runtime: &Arc<ProductionSessionWorkers>,
) -> (TranscriptExtension, std::sync::mpsc::Receiver<serde_json::Value>) {
    let bus = SandHostEventBus::default();
    let events = bus.subscribe();
    let extension = start_transcript_extension(
        root,
        Arc::clone(runtime),
        Arc::new(move |event| bus.publish(event)),
    );
    (extension, events)
}

fn human_transcript_feed_events(
    events: &std::sync::mpsc::Receiver<serde_json::Value>,
) -> Vec<serde_json::Value> {
    events.try_iter()
        .filter(|event| event.get("channel").and_then(serde_json::Value::as_str) == Some("transcript"))
        .collect()
}

// Deliberately untrusted notifications are used only for negative/admission
// cases. The positive integration oracle below comes from a real native sync.
fn publish_human_transcript_feed_hint(
    conversation_id: &str,
    origin: Option<serde_json::Value>,
    entries: serde_json::Value,
) {
    let mut mutation = json!({
        "kind": "entries-upserted",
        "agentId": conversation_id,
        "entries": entries,
    }).as_object().expect("mutation object").clone();
    if let Some(origin) = origin {
        mutation.insert(TRANSCRIPT_MUTATION_ORIGIN_KEY.into(), origin);
    }
    publish_transcript_mutation(&mutation);
}

#[test]
fn shipping_human_committed_refresh_reaches_production_transcript_bus() {
    let root = temp_root("human-production-transcript-feed");
    let mut initial = fixture_remote_human_message(
        51, 2, 1, "before refresh", Some("peer-request"), json!([]),
    );
    initial["reactions"] = json!([{"emoji":"👍","count":1,"reactedByMe":false}]);
    let mut remote = initial.clone();
    remote["text"] = json!("current native snapshot");
    remote["readAt"] = json!("2026-10-05T03:00:00Z");
    remote["reactions"] = json!([]);
    let (url, server) = spawn_human_message_fixture(vec![
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[initial]}})),
        HumanMessageFixtureReply::Json(200, json!({"success":true,"data":{"messages":[remote]}})),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let conversation_id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("conversation id").to_string();
    let first = dispatch(&runtime, "syncHumanConversation", json!({"conversationId":conversation_id}));
    assert_eq!(first.as_array().expect("initial rows").len(), 1);
    assert_eq!(first[0]["content"], "before refresh");
    let (extension, events) = start_human_transcript_feed(&root, &runtime);
    assert!(human_transcript_feed_events(&events).is_empty());

    // The real Session sync holds its conversation mutation lane while the
    // post-commit bridge runs; a second acquisition here would deadlock.
    let refreshed = dispatch(&runtime, "syncHumanConversation", json!({"conversationId":conversation_id}));
    assert_eq!(refreshed.as_array().expect("refreshed rows").len(), 1);
    let owner = runtime.open_human_conversation_db_owner(&conversation_id).expect("owner");
    let current = owner.get_transcript_entry_by_id("human-server-message:51")
        .expect("read committed row").expect("committed row");
    assert_eq!(current["content"], "current native snapshot");
    assert_eq!(current["readAt"], "2026-10-05T03:00:00Z");
    assert_eq!(current["reactions"], json!([]));
    assert_eq!(current["remoteClientRequestId"], "peer-request");
    assert!(current.get("clientNonce").is_none());
    assert_eq!(refreshed[0], current);
    assert_eq!(human_transcript_feed_events(&events), vec![json!({
        "channel":"transcript",
        "payload":{"type":"updated","agentId":conversation_id,"entry":current},
    })], "the shipping bus receives the exact durable row without origin or before:null");

    drop(extension);
    runtime.shutdown();
    let requests = server.join().expect("native refresh requests");
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| request.contains("&afterId=0")));
    drop(owner);
    drop(runtime);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_transcript_feed_reads_current_owner_and_deduplicates_ids() {
    let root = temp_root("human-feed-current-owner");
    let (url, stop, server) = spawn_human_no_request_fixture();
    let runtime = human_message_fixture_runtime(&root, &url);
    let conversation_id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("conversation id").to_string();
    let owner = runtime.open_human_conversation_db_owner(&conversation_id).expect("owner");
    let mut first = fixture_saved_human_message(51, "2", "initial durable peer body");
    first["remoteClientRequestId"] = json!("peer-provenance");
    let mut second = fixture_saved_human_message(52, "1", "durable local body");
    second["clientNonce"] = json!("original-local-nonce");
    second["readAt"] = json!("2026-10-05T03:00:00Z");
    second["reactions"] = json!([{"emoji":"👍","by":"2"}]);
    assert_eq!(owner.append_transcript_entries(&[first.clone(), second.clone()]).expect("seed rows"), 2);
    let (extension, events) = start_human_transcript_feed(&root, &runtime);
    let stale_first = first.clone();
    first["content"] = json!("newer durable body");
    first["readAt"] = json!("2026-10-05T04:00:00Z");
    let current_first = owner.update_transcript_entry("human-server-message:51", &first)
        .expect("commit newer row").expect("newer row");
    assert_eq!(human_transcript_feed_events(&events), vec![json!({
        "channel":"transcript",
        "payload":{"type":"updated","agentId":conversation_id,"entry":current_first},
    })]);

    let origin = transcript_mutation_origin(owner.db_path()).expect("trusted owner origin");
    publish_human_transcript_feed_hint(&conversation_id, Some(json!(origin)), json!([
        {"id":"human-server-message:52","content":"forged body","authorId":"999",
         "clientNonce":"forged-nonce","attachments":[{"path":"/must-not-be-read"}]},
        stale_first,
        {"id":"human-server-message:52","content":"duplicate forged body"},
        {"id":"human-server-message:missing","content":"not persisted"},
    ]));
    assert_eq!(human_transcript_feed_events(&events), vec![
        json!({"channel":"transcript","payload":{"type":"updated","agentId":conversation_id,"entry":second}}),
        json!({"channel":"transcript","payload":{"type":"updated","agentId":conversation_id,"entry":current_first}}),
    ], "requested order is retained, duplicate IDs collapse and notification bodies have no authority");
    assert_eq!(owner.get_transcript_entries().expect("unchanged durable rows"), vec![first, second]);

    drop(extension);
    runtime.shutdown();
    stop.send(()).expect("stop no-request fixture");
    assert!(server.join().expect("no-request fixture").is_empty(), "projection performs no HTTP");
    drop(owner);
    drop(runtime);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_human_transcript_feed_rejects_foreign_roots_agents_and_unauthorized_origins() {
    let root = temp_root("human-feed-local-root");
    let foreign_root = temp_root("human-feed-foreign-root");
    let (url, stop, server) = spawn_human_no_request_fixture();
    let runtime = human_message_fixture_runtime(&root, &url);
    let foreign = human_message_fixture_runtime(&foreign_root, &url);
    let conversation_id = runtime.create_human_conversation("2", "Peer").expect("local conversation")["id"]
        .as_str().expect("local conversation id").to_string();
    let foreign_id = foreign.create_human_conversation("2", "Peer").expect("foreign conversation")["id"]
        .as_str().expect("foreign conversation id").to_string();
    assert_eq!(foreign_id, conversation_id, "the origin gate must distinguish identical conversation IDs");
    let owner = runtime.open_human_conversation_db_owner(&conversation_id).expect("local owner");
    let foreign_owner = foreign.open_human_conversation_db_owner(&foreign_id).expect("foreign owner");
    let mut local_row = fixture_saved_human_message(51, "2", "local durable content");
    assert!(owner.append_transcript_entry(&local_row).expect("local seed"));
    assert!(foreign_owner.append_transcript_entry(&fixture_saved_human_message(51, "2", "foreign seed"))
        .expect("foreign seed"));
    let local_origin = transcript_mutation_origin(owner.db_path()).expect("local origin");
    let foreign_origin = transcript_mutation_origin(foreign_owner.db_path()).expect("foreign origin");
    assert_ne!(local_origin, foreign_origin);
    let (extension, events) = start_human_transcript_feed(&root, &runtime);

    foreign_owner.update_transcript_entry(
        "human-server-message:51",
        &fixture_saved_human_message(51, "2", "foreign body must never be projected"),
    ).expect("foreign commit").expect("foreign row");
    assert!(human_transcript_feed_events(&events).is_empty(), "same-ID foreign root emits no event");

    let agent = runtime.materialize_new_session(None, "user", None).expect("ordinary Agent");
    runtime.append_agent_transcript_entries(&agent.id, &[local_row.clone()]).expect("Agent write");
    let agent_owner = runtime.open_agent_db_owner(&agent.id).expect("Agent owner");
    let agent_origin = transcript_mutation_origin(agent_owner.db_path()).expect("Agent origin");
    assert!(human_transcript_feed_events(&events).is_empty(), "ordinary Agent writes keep their existing owner");
    for origin in [
        None, Some(serde_json::Value::Null), Some(json!(7)), Some(json!([])),
        Some(json!("")), Some(json!("../untrusted-notification-path")),
        Some(json!(foreign_origin)), Some(json!(agent_origin)),
    ] {
        publish_human_transcript_feed_hint(&conversation_id, origin, json!([local_row.clone()]));
        assert!(human_transcript_feed_events(&events).is_empty(), "unqualified or different origin must not project");
    }
    let absent_id = foreign.create_human_conversation("3", "Another peer").expect("foreign-only conversation")["id"]
        .as_str().expect("foreign-only id").to_string();
    let absent_path = mahayana_host_runtime::extensions::session::session_paths::get_native_conversation_db_path(
        &root.join("agents"), &absent_id,
    ).expect("expected local path");
    assert!(!absent_path.exists());
    let absent_owner = foreign.open_human_conversation_db_owner(&absent_id).expect("foreign-only owner");
    assert!(absent_owner.append_transcript_entry(&fixture_saved_human_message(61, "3", "foreign-only row"))
        .expect("foreign-only commit"));
    assert!(human_transcript_feed_events(&events).is_empty());
    assert!(!absent_path.exists(), "projection never creates a local conversation for a notification");

    assert!(owner.set_metadata("participantIds", json!(["3","4"])).expect("remove local permission"));
    local_row["content"] = json!("committed while local account is not a participant");
    owner.update_transcript_entry("human-server-message:51", &local_row).expect("unauthorized-owner write")
        .expect("persisted row");
    assert!(human_transcript_feed_events(&events).is_empty(), "origin alone cannot grant participant authority");
    assert!(owner.set_metadata("participantIds", json!(["1","2"])).expect("restore test permission"));
    local_row["content"] = json!("authorized recovery");
    let recovered = owner.update_transcript_entry("human-server-message:51", &local_row)
        .expect("authorized commit").expect("authorized row");
    assert_eq!(human_transcript_feed_events(&events), vec![json!({
        "channel":"transcript","payload":{"type":"updated","agentId":conversation_id,"entry":recovered},
    })], "rejected notifications do not disable later authorized projection");

    drop(extension);
    runtime.shutdown();
    foreign.shutdown();

    // A qualified hint must not lazily open an existing on-disk database or
    // register an owner in a fresh Session. Only a normal owner entry may do so.
    let cold = human_message_fixture_runtime(&root, &url);
    let (cold_extension, cold_events) = start_human_transcript_feed(&root, &cold);
    for _ in 0..2 {
        publish_human_transcript_feed_hint(&conversation_id, Some(json!(local_origin)), json!([local_row.clone()]));
        assert!(human_transcript_feed_events(&cold_events).is_empty(), "notification cannot open/register a cold owner");
    }
    let loaded = cold.open_human_conversation_db_owner(&conversation_id).expect("explicit normal owner load");
    publish_human_transcript_feed_hint(&conversation_id, Some(json!(local_origin)), json!([local_row.clone()]));
    assert_eq!(human_transcript_feed_events(&cold_events), vec![json!({
        "channel":"transcript","payload":{"type":"updated","agentId":conversation_id,"entry":recovered},
    })], "normal owner admission enables projection without altering durable content");
    drop(cold_extension);
    cold.shutdown();
    drop((loaded, cold));

    stop.send(()).expect("stop no-request fixture");
    assert!(server.join().expect("no-request fixture").is_empty(), "origin/participant admission performs no HTTP");
    drop((owner, foreign_owner, absent_owner, agent_owner, runtime, foreign));
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(foreign_root);
}

#[test]
fn shipping_human_transcript_feed_disposes_copied_callbacks_before_replacement() {
    let root = temp_root("human-feed-copied-dispatch");
    let (url, stop, server) = spawn_human_no_request_fixture();
    let runtime = human_message_fixture_runtime(&root, &url);
    let conversation_id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
        .as_str().expect("conversation id").to_string();
    let owner = runtime.open_human_conversation_db_owner(&conversation_id).expect("owner");
    assert!(owner.append_transcript_entry(&fixture_saved_human_message(51, "2", "seed"))
        .expect("seed row"));
    let blocked_origin = transcript_mutation_origin(owner.db_path()).expect("trusted origin");
    let blocked_conversation = conversation_id.clone();
    let (paused, entered) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let released = std::sync::Mutex::new(released);
    let block_once = std::sync::atomic::AtomicBool::new(true);
    // Registration order is the registry's BTreeMap order. Pause after its
    // listener snapshot already includes the old extension, before admission.
    let blocker = subscribe_transcript_mutations(move |mutation| {
        let is_target = mutation.get("kind").and_then(serde_json::Value::as_str) == Some("entries-upserted")
            && mutation.get("agentId").and_then(serde_json::Value::as_str) == Some(blocked_conversation.as_str())
            && mutation.get(TRANSCRIPT_MUTATION_ORIGIN_KEY).and_then(serde_json::Value::as_str) == Some(blocked_origin.as_str())
            && mutation.get("entries").and_then(serde_json::Value::as_array)
                .is_some_and(|entries| entries.iter().any(|entry| entry.get("id")
                    .and_then(serde_json::Value::as_str) == Some("human-server-message:51")));
        if is_target && block_once.swap(false, std::sync::atomic::Ordering::AcqRel) {
            paused.send(()).expect("report copied dispatch");
            released.lock().expect("release receiver").recv_timeout(std::time::Duration::from_secs(15))
                .expect("copied dispatch must be explicitly released");
        }
    });
    let (extension, old_events) = start_human_transcript_feed(&root, &runtime);
    let manager = extension.manager();
    let writer = Arc::clone(&owner);
    let write = thread::spawn(move || {
        writer.update_transcript_entry(
            "human-server-message:51", &fixture_saved_human_message(51, "2", "committed before disposal"),
        ).expect("paused durable update").expect("paused row")
    });
    entered.recv_timeout(std::time::Duration::from_secs(15)).expect("global snapshot copied before disposal");
    drop(extension);
    assert!(manager.is_disposed());
    assert!(owner.is_closed(), "disposing the manager closes its Session database owners");
    // Reopen through the normal Session owner path before releasing the copied
    // callback. The old callback would still have a readable authorized owner
    // if its disposal fence failed; an empty cache must not mask that failure.
    let replacement_owner = runtime.open_human_conversation_db_owner(&conversation_id)
        .expect("replacement owner");
    assert!(!Arc::ptr_eq(&owner, &replacement_owner));
    assert!(!replacement_owner.is_closed());
    let durable = replacement_owner.get_transcript_entry_by_id("human-server-message:51")
        .expect("read before releasing copied callback").expect("committed durable row");
    assert_eq!(durable["content"], "committed before disposal");
    let (replacement, new_events) = start_human_transcript_feed(&root, &runtime);
    release.send(()).expect("release old copied callback");
    let committed = write.join().expect("paused writer");
    blocker.unsubscribe();
    assert_eq!(replacement_owner.get_transcript_entry_by_id("human-server-message:51").expect("read committed row"), Some(committed));
    assert!(human_transcript_feed_events(&old_events).is_empty(), "copied old callback cannot publish after disposal");
    assert!(human_transcript_feed_events(&new_events).is_empty(), "old dispatch cannot be attributed to the replacement");

    let current = replacement_owner.update_transcript_entry(
        "human-server-message:51", &fixture_saved_human_message(51, "2", "replacement current row"),
    ).expect("replacement commit").expect("replacement row");
    assert_eq!(human_transcript_feed_events(&new_events), vec![json!({
        "channel":"transcript","payload":{"type":"updated","agentId":conversation_id,"entry":current},
    })], "replacement installs exactly one projection observer");
    assert!(human_transcript_feed_events(&old_events).is_empty());
    drop(replacement);
    assert!(replacement_owner.is_closed());
    // The final zero-event observation must accompany a real committed write,
    // not the no-op behavior of an owner closed by replacement Drop.
    let final_owner = runtime.open_human_conversation_db_owner(&conversation_id)
        .expect("post-disposal owner");
    assert!(!Arc::ptr_eq(&replacement_owner, &final_owner));
    assert!(!final_owner.is_closed());
    assert_eq!(final_owner.get_transcript_entry_by_id("human-server-message:51").expect("read replacement row"), Some(current));
    let final_entry = final_owner.update_transcript_entry(
        "human-server-message:51", &fixture_saved_human_message(51, "2", "after both extensions stopped"),
    ).expect("post-disposal commit").expect("post-disposal row");
    assert_eq!(final_owner.get_transcript_entry_by_id("human-server-message:51").expect("read post-disposal row"), Some(final_entry));
    assert!(human_transcript_feed_events(&old_events).is_empty());
    assert!(human_transcript_feed_events(&new_events).is_empty());

    runtime.shutdown();
    stop.send(()).expect("stop no-request fixture");
    assert!(server.join().expect("no-request fixture").is_empty(), "relay lifecycle performs no HTTP");
    drop((final_owner, replacement_owner, owner, runtime, manager));
    let _ = fs::remove_dir_all(root);
}


#[test]
fn shipping_human_offline_origin_is_explicit_and_survives_settlement_and_restart() {
    // The existing gateway must reject malformed first observations before
    // either SQLite staging or an actual HTTP request.
    {
        let root = temp_root("human-offline-origin-admission");
        let (url, stop, server) = spawn_human_no_request_fixture();
        let runtime = human_message_fixture_runtime(&root, &url);
        let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
            .as_str().expect("conversation id").to_owned();
        let owner = runtime.open_human_conversation_db_owner(&id).expect("existing SQLite owner");
        let metadata = owner.read_metadata().expect("metadata before rejected origin");
        let original = json!({
            "conversationId":id,"text":"invalid origin must not stage","clientNonce":"origin-admission",
            "composedAtMs":1000.0,"attachments":[]
        });
        for marker in [json!(null), json!("true"), json!(1), json!([]), json!({})] {
            let mut args = original.clone();
            args["queuedOffline"] = marker.clone();
            let result = dispatch_production_session_gateway_call(&runtime, "sendHumanMessage", &args)
                .expect("registered Human send");
            assert!(matches!(result, Err(SessionGatewayError::BadRequest(_))),
                "present origin must be a boolean: {marker}");
            assert!(owner.get_transcript_entries().expect("no malformed pending row").is_empty());
        }
        for composed in [None, Some(json!(null)), Some(json!("1000"))] {
            let mut args = original.clone();
            args["queuedOffline"] = json!(true);
            match composed {
                Some(value) => { args["composedAtMs"] = value; }
                None => { args.as_object_mut().unwrap().remove("composedAtMs"); }
            }
            let result = dispatch_production_session_gateway_call(&runtime, "sendHumanMessage", &args)
                .expect("registered Human send");
            assert!(matches!(result, Err(SessionGatewayError::BadRequest(_))),
                "true origin requires a finite numeric composition time");
        }
        for composed in [None, Some(f64::NAN), Some(f64::INFINITY), Some(f64::NEG_INFINITY)] {
            let result = runtime.append_human_message_with_queue_origin(
                &id, "invalid native origin", "origin-direct-admission", composed, None, &[], true,
            );
            assert!(result.expect_err("native callers share origin admission").contains("finite composedAtMs"));
        }
        assert!(owner.get_transcript_entries().expect("all rejected sends stay unstaged").is_empty());
        assert_eq!(owner.read_metadata().expect("unchanged conversation"), metadata);
        stop.send(()).expect("stop active origin admission listener");
        assert!(server.join().expect("origin admission HTTP requests").is_empty(),
            "malformed gateway and native inputs perform zero HTTP");
        runtime.shutdown(); drop(owner); drop(runtime);
        let _ = fs::remove_dir_all(root);
    }

    // A real failed first POST records origin once. A caller using the same
    // nonce after a physical close/reopen cannot replace either that observation
    // or the original composition time. Sync wins a held duplicate receipt.
    for (label, initial_origin) in [
        ("online", Some(false)), ("legacy-timestamp", None), ("offline", Some(true)),
    ] {
        let root = temp_root(&format!("human-offline-origin-{label}"));
        let nonce = format!("origin-{label}");
        let text = format!("retain the original {label} composition");
        let composed_at_ms = if initial_origin == Some(true) { -12.5 } else { 1234.5 };
        let remote = fixture_remote_human_message(77, 1, 2, &text, Some(&nonce), json!([]));
        let mut refreshed = remote.clone();
        refreshed["readAt"] = json!("2026-10-05T01:05:00Z");
        let mut after_restart = refreshed.clone();
        after_restart["readAt"] = json!("2026-10-05T01:06:00Z");
        let (seen, accepted) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let (url, server) = spawn_human_message_fixture(vec![
            HumanMessageFixtureReply::Disconnect,
            HumanMessageFixtureReply::Deferred(
                Box::new(HumanMessageFixtureReply::Json(201,
                    json!({"success":true,"message":remote.clone()}))), seen, resume,
            ),
            // The existing bounded sync replays the admitted nonce, then reads
            // the current history while the earlier POST response remains held.
            HumanMessageFixtureReply::Json(200,
                json!({"success":true,"deduplicated":true,"message":remote.clone()})),
            HumanMessageFixtureReply::Json(200,
                json!({"success":true,"data":{"messages":[refreshed]}})),
            HumanMessageFixtureReply::Json(200,
                json!({"success":true,"data":{"messages":[after_restart]}})),
        ]);
        let runtime = human_message_fixture_runtime(&root, &url);
        let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
            .as_str().expect("conversation id").to_owned();
        let mut args = json!({
            "conversationId":id,"text":text,"clientNonce":nonce,
            "composedAtMs":composed_at_ms,"attachments":[]
        });
        if let Some(origin) = initial_origin { args["queuedOffline"] = json!(origin); }
        assert!(dispatch_production_session_gateway_call(&runtime, "sendHumanMessage", &args)
            .expect("registered first send").is_err(), "a lost response leaves a durable intent");
        let owner = runtime.open_human_conversation_db_owner(&id).expect("first SQLite owner");
        let pending = owner.get_transcript_entries().expect("persisted original intent");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0]["id"], format!("human-message:{nonce}"));
        assert_eq!(pending[0]["delivery"], "pending");
        assert_eq!(pending[0]["composedAtMs"], json!(composed_at_ms));
        if initial_origin == Some(true) {
            assert_eq!(pending[0]["queuedOffline"], true);
        } else {
            assert!(pending[0].get("queuedOffline").is_none(),
                "online and timestamp-only callers retain the existing row shape");
        }
        runtime.shutdown();
        assert!(owner.is_closed(), "the original SQLite owner really closes");
        drop(owner); drop(runtime);

        let reopened = human_message_fixture_runtime(&root, &url);
        assert_eq!(reopened.read_human_conversation_transcript(&id).expect("pure pending reopen"), pending);
        let owner = reopened.open_human_conversation_db_owner(&id).expect("replacement SQLite owner");
        args["queuedOffline"] = json!(initial_origin != Some(true));
        args["composedAtMs"] = json!(9000.0);
        let sending = {
            let runtime = Arc::clone(&reopened); let args = args.clone();
            thread::spawn(move || dispatch(&runtime, "sendHumanMessage", args))
        };
        accepted.recv_timeout(std::time::Duration::from_secs(15)).expect("replacement POST is actually held");
        let in_flight = owner.get_transcript_entries().expect("original intent during duplicate POST");
        assert_eq!(in_flight.len(), 1);
        assert_eq!(in_flight[0]["delivery"], "dispatching");
        assert_eq!(in_flight[0].get("queuedOffline"), pending[0].get("queuedOffline"),
            "a repeated caller cannot promote or demote the first origin");
        assert_eq!(in_flight[0]["composedAtMs"], pending[0]["composedAtMs"]);

        let synced = reopened.sync_human_conversation(&id).expect("existing replay and bounded history settle");
        assert_eq!(synced.len(), 1);
        let canonical = synced[0].clone();
        assert_eq!(canonical["id"], "human-server-message:77");
        assert_eq!(canonical["clientNonce"], nonce);
        assert_eq!(canonical["delivery"], "sent");
        assert_eq!(canonical.get("queuedOffline"), pending[0].get("queuedOffline"));
        assert_eq!(canonical["composedAtMs"], pending[0]["composedAtMs"]);
        assert_eq!(canonical["timestampMs"], json!(chrono::DateTime::parse_from_rfc3339(
            remote["createdAt"].as_str().unwrap()).unwrap().timestamp_millis() as f64),
            "server time stays canonical instead of being replaced by composition time");
        assert_eq!(canonical["readAt"], "2026-10-05T01:05:00Z");
        release.send(()).expect("release the old receipt after canonical history");
        assert_eq!(sending.join().expect("held send thread"), canonical,
            "late acknowledgement returns the full current durable settlement");
        assert!(owner.get_transcript_entry_by_id(&format!("human-message:{nonce}"))
            .expect("old pending identity lookup").is_none());
        assert_eq!(dispatch(&reopened, "retryHumanMessage",
            json!({"conversationId":id,"clientNonce":nonce})), canonical,
            "canonical identity-only retry does not replay payload or origin");
        assert_eq!(dispatch(&reopened, "sendHumanMessage", args), canonical,
            "an already canonical nonce cannot be rebound by new origin or time");
        reopened.shutdown();
        assert!(owner.is_closed());
        drop(owner); drop(reopened);

        let recovered = human_message_fixture_runtime(&root, &url);
        assert_eq!(recovered.read_human_conversation_transcript(&id).expect("canonical close/reopen"), vec![canonical.clone()]);
        let refreshed = recovered.sync_human_conversation(&id).expect("known canonical refresh after restart");
        assert_eq!(refreshed.len(), 1);
        let mut expected = canonical;
        expected["readAt"] = json!("2026-10-05T01:06:00Z");
        assert_eq!(refreshed, vec![expected.clone()], "known history preserves exact local provenance");
        assert_eq!(dispatch(&recovered, "retryHumanMessage",
            json!({"conversationId":id,"clientNonce":nonce})), expected);
        recovered.shutdown(); drop(recovered);
        let requests = server.join().expect("origin settlement HTTP fixture");
        assert_eq!(requests.len(), 5, "three same-nonce POSTs and two bounded history reads");
        for request in &requests[..3] {
            assert!(request.starts_with("POST /api/social/messages "));
            let body = fixture_request_json(request);
            assert_eq!(body, fixture_request_json(&requests[0]), "external service identity and payload remain unchanged");
            assert!(body.get("queuedOffline").is_none() && body.get("composedAtMs").is_none(),
                "local origin metadata does not expand the external service API");
        }
        assert!(requests[3].starts_with("GET /api/social/messages?") && requests[3].contains("&afterId=0"));
        assert!(requests[4].starts_with("GET /api/social/messages?") && requests[4].contains("&afterId=0"));
        let _ = fs::remove_dir_all(root);
    }

    // Historical timestamp-only intents remain retryable, and an explicitly
    // stored false value is retained as false rather than being inferred true.
    for retained_origin in [None, Some(false)] {
        let root = temp_root(if retained_origin.is_some() { "human-origin-retained-false" } else { "human-origin-retained-legacy" });
        let nonce = "retained-origin";
        let remote = fixture_remote_human_message(78, 1, 2, "retained legacy intent", Some(nonce), json!([]));
        let (url, server) = spawn_human_message_fixture(vec![
            HumanMessageFixtureReply::Json(201, json!({"success":true,"message":remote})),
        ]);
        let runtime = human_message_fixture_runtime(&root, &url);
        let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
            .as_str().expect("conversation id").to_owned();
        let owner = runtime.open_human_conversation_db_owner(&id).expect("retained intent owner");
        let mut pending = pending_reply_fixture_row(nonce, "retained legacy intent", None);
        if let Some(origin) = retained_origin { pending["queuedOffline"] = json!(origin); }
        assert!(owner.append_transcript_entry(&pending).expect("retain historical input"));
        let settled = dispatch(&runtime, "retryHumanMessage", json!({"conversationId":id,"clientNonce":nonce}));
        assert_eq!(settled["id"], "human-server-message:78");
        assert_eq!(settled.get("queuedOffline"), pending.get("queuedOffline"));
        assert_eq!(settled["composedAtMs"], pending["composedAtMs"]);
        runtime.shutdown(); drop(owner); drop(runtime);
        let recovered = human_message_fixture_runtime(&root, &url);
        assert_eq!(recovered.read_human_conversation_transcript(&id).expect("retained canonical origin"), vec![settled.clone()]);
        assert_eq!(dispatch(&recovered, "retryHumanMessage",
            json!({"conversationId":id,"clientNonce":nonce})), settled);
        recovered.shutdown(); drop(recovered);
        let requests = server.join().expect("retained-origin retry fixture");
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("POST /api/social/messages "));
        let _ = fs::remove_dir_all(root);
    }

    // Invalid retained markers must not dispatch a send or be overwritten by
    // history without local provenance. Independent bounded reads still progress:
    // this is not a global history preflight or a block on unrelated peer rows.
    for (label, marker, composed) in [
        ("null", json!(null), Some(json!(1234.0))),
        ("string", json!("true"), Some(json!(1234.0))),
        ("number", json!(1), Some(json!(1234.0))),
        ("array", json!([]), Some(json!(1234.0))),
        ("object", json!({}), Some(json!(1234.0))),
        ("true-no-time", json!(true), None),
        ("true-null-time", json!(true), Some(json!(null))),
        ("true-string-time", json!(true), Some(json!("1234"))),
    ] {
        let root = temp_root(&format!("human-origin-damaged-{label}"));
        let nonce = "damaged-origin";
        let text = "retain the original damaged origin";
        let peer = fixture_remote_human_message(88, 2, 1, "independent peer history", None, json!([]));
        let remote = fixture_remote_human_message(89, 1, 2, text, Some(nonce), json!([]));
        let (seen, accepted) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        release.send(()).expect("the observed history response is not artificially held");
        let (url, server) = spawn_human_message_fixture(vec![
            HumanMessageFixtureReply::Deferred(Box::new(HumanMessageFixtureReply::Json(200,
                json!({"success":true,"data":{"messages":[peer.clone()]}}))), seen, resume),
            HumanMessageFixtureReply::Json(200,
                json!({"success":true,"data":{"messages":[peer,remote]}})),
        ]);
        let runtime = human_message_fixture_runtime(&root, &url);
        let id = runtime.create_human_conversation("2", "Peer").expect("conversation")["id"]
            .as_str().expect("conversation id").to_owned();
        let owner = runtime.open_human_conversation_db_owner(&id).expect("damaged intent owner");
        let mut pending = pending_reply_fixture_row(nonce, text, None);
        pending["queuedOffline"] = marker;
        pending["retainedLocalMetadata"] = json!({"case":label,"never":"erase this intent"});
        match composed {
            Some(value) => { pending["composedAtMs"] = value; }
            None => { pending.as_object_mut().unwrap().remove("composedAtMs"); }
        }
        assert!(owner.append_transcript_entry(&pending).expect("seed retained invalid provenance"));
        let db = rusqlite::Connection::open_with_flags(forward_db_path(&root, &id),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).expect("independent durable origin observation");
        let raw_intent = || db.query_row(
            "SELECT seq, id, entry FROM transcript_entries WHERE id = ?1",
            [format!("human-message:{nonce}")],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
        ).expect("retained complete intent bytes");
        let raw_before = raw_intent();
        let retry = dispatch_production_session_gateway_call(&runtime, "retryHumanMessage",
            &json!({"conversationId":id,"clientNonce":nonce})).expect("registered identity-only retry");
        assert!(retry.is_err(), "malformed retained origin is not dispatched: {label}");
        assert!(matches!(accepted.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)),
            "identity-only rejection performs zero HTTP: {label}");
        assert_eq!(owner.get_transcript_entries().expect("intent retained after rejection"), vec![pending.clone()]);
        assert_eq!(raw_intent(), raw_before, "rejected retry preserves physical identity and bytes: {label}");

        let independent = runtime.sync_human_conversation(&id).expect("unrelated bounded history still progresses");
        accepted.recv_timeout(std::time::Duration::from_secs(15)).expect("the actual history GET reaches the service");
        assert_eq!(independent.len(), 2);
        assert_eq!(independent.iter().find(|entry| entry["id"] == pending["id"]), Some(&pending));
        assert!(independent.iter().any(|entry| entry["id"] == "human-server-message:88"));
        assert_eq!(raw_intent(), raw_before);
        let error = runtime.sync_human_conversation(&id)
            .expect_err("remote canonical data cannot supply missing validation for damaged local origin");
        assert!(error.contains("origin"), "{label}: {error}");
        assert_eq!(owner.get_transcript_entries().expect("remote cannot overwrite damaged local intent"), independent);
        assert_eq!(raw_intent(), raw_before, "history refusal retains the original bytes: {label}");
        assert!(owner.get_transcript_entry_by_id("human-server-message:89").expect("no false canonical remap").is_none());
        runtime.shutdown(); drop(owner); drop(runtime);
        let reopened = human_message_fixture_runtime(&root, &url);
        assert_eq!(reopened.read_human_conversation_transcript(&id).expect("reopen keeps damaged intent"), independent);
        assert_eq!(raw_intent(), raw_before);
        reopened.shutdown(); drop(reopened); drop(db);
        let requests = server.join().expect("retained-origin bounded HTTP fixture");
        assert_eq!(requests.len(), 2, "only the two explicit bounded history reads execute: {label}");
        assert!(requests.iter().all(|request| request.starts_with("GET /api/social/messages?")
            && request.contains("&afterId=0")), "invalid origin never dispatches an unsafe POST: {label}");
        let _ = fs::remove_dir_all(root);
    }
}


// NMR-REACTION-RECOVERY-01: real existing Session/Native/SQLite owners.
// These helpers observe the transport and durable state; they implement no
// reaction, history, credential or CallSession business behavior.
fn reaction_recovery_seed(
    runtime: &ProductionSessionWorkers,
    peer: &str,
    remote_id: u64,
    stage_intent: bool,
) -> (String, serde_json::Value, serde_json::Value) {
    let id = runtime.create_human_conversation(peer, "Retained recovery peer")
        .expect("canonical recovery conversation")["id"].as_str().unwrap().to_owned();
    let owner = runtime.open_human_conversation_db_owner(&id).expect("real recovery SQLite owner");
    let mut row = human_local_history_message(remote_id);
    row["authorId"] = json!(peer);
    row["retainedLocalMetadata"] = json!({"text":"keep original\0人","version":7});
    assert!(owner.append_transcript_entry(&row).expect("seed settled canonical target"));
    let intent = json!({
        "entryId":format!("human-server-message:{remote_id}"),
        "remoteMessageId":remote_id.to_string(),"emoji":"👍","active":true,"localHumanId":"1"
    });
    if stage_intent {
        assert!(owner.set_metadata("humanReactionIntent", intent.clone()).expect("retained desired state"));
    }
    (id, row, intent)
}

fn reaction_recovery_friends(peers: &[u64]) -> serde_json::Value {
    json!({"success":true,"data":{"friends":peers.iter().map(|peer| json!({
        "id":peer,"userId":peer,"username":format!("peer-{peer}"),
        "displayName":format!("Peer {peer}"),"avatarUrl":null,"status":"accepted"
    })).collect::<Vec<_>>()}})
}

fn reaction_recovery_history(peer: u64, id: u64, text: &str) -> HumanMessageFixtureReply {
    human_history_reply(vec![fixture_remote_human_message(id, peer, 1, text, None, json!([]))])
}

fn reaction_recovery_refusal(status: u16) -> HumanMessageFixtureReply {
    HumanMessageFixtureReply::Json(status, json!({"success":false,"error":"retained reaction refusal"}))
}

fn reaction_recovery_error(status: u16) -> String {
    format!("Fabushi Human reaction mutation was rejected (HTTP {status}): retained reaction refusal")
}

fn reaction_recovery_remote_call(call_id: &str, peer: u64, event_seq: u64) -> serde_json::Value {
    json!({
        "callId":call_id,"creatorUserId":peer,"peerUserId":1,
        "state":if event_seq == 0 { "invited" } else { "negotiating" },
        "generation":0,"eventSeq":event_seq,"terminalState":null,
        "createdAt":"2026-10-05T04:10:00Z","updatedAt":"2026-10-05T04:10:01Z"
    })
}

fn reaction_recovery_call_body(call_id: &str, peer: u64) -> serde_json::Value {
    json!({
        "success":true,"call":reaction_recovery_remote_call(call_id, peer, 1),
        "events":[{
            "callId":call_id,"seq":1,"generation":0,"userId":1,"deviceId":"device-a",
            "clientEventId":"transition:0:accept","kind":"transition",
            "payload":{"action":"accept","state":"negotiating"},
            "createdAt":"2026-10-05T04:10:01Z"
        }],"nextAfterSeq":1
    })
}

fn reaction_recovery_calls_image(runtime: &ProductionSessionWorkers) -> serde_json::Value {
    // Calling shipping get/list would itself synchronize Calls. This reader
    // uses the already initialized owner path and cannot create or alter a DB.
    let db = rusqlite::Connection::open_with_flags(
        runtime.call_session_store_path().expect("existing CallSession database"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ).expect("read-only Calls observer");
    db.busy_timeout(std::time::Duration::from_secs(5)).expect("bounded Calls read");
    let mut image = serde_json::Map::new();
    for (table, order) in [
        ("call_sessions", "1"), ("call_remote_sync", "1"), ("call_signals", "1,2,3"),
    ] {
        let mut statement = db.prepare(&format!("SELECT * FROM {table} ORDER BY {order}"))
            .expect("existing Calls table");
        let columns = statement.column_count();
        let rows = statement.query_map([], |row| {
            (0..columns).map(|column| Ok(match row.get_ref(column)? {
                rusqlite::types::ValueRef::Null => serde_json::Value::Null,
                rusqlite::types::ValueRef::Integer(value) => json!(value),
                rusqlite::types::ValueRef::Real(value) => json!(value),
                rusqlite::types::ValueRef::Text(bytes) =>
                    json!(String::from_utf8(bytes.to_vec()).expect("fixture UTF-8")),
                rusqlite::types::ValueRef::Blob(bytes) => json!({"blob":bytes}),
            })).collect::<Result<Vec<_>, rusqlite::Error>>()
        }).expect("query Calls rows").collect::<Result<Vec<_>, _>>().expect("read Calls rows");
        image.insert(table.into(), json!(rows));
    }
    serde_json::Value::Object(image)
}

fn reaction_recovery_directories(root: &std::path::Path) -> Vec<String> {
    let mut entries = fs::read_dir(root.join("agents/.conversations"))
        .expect("existing native conversation root")
        .map(|entry| entry.expect("conversation directory entry").file_name()
            .to_string_lossy().into_owned()).collect::<Vec<_>>();
    entries.sort();
    entries
}

fn reaction_recovery_state(runtime: &ProductionSessionWorkers, root: &std::path::Path) -> serde_json::Value {
    let roster = runtime.list_human_conversations().expect("local roster only");
    let conversations = roster.iter().map(|conversation| {
        let id = conversation["id"].as_str().expect("canonical roster id");
        let owner = runtime.open_human_conversation_db_owner(id).expect("existing conversation owner");
        json!({"id":id,"metadata":owner.read_metadata().expect("complete metadata"),
            "rows":owner.get_transcript_entries().expect("complete retained rows")})
    }).collect::<Vec<_>>();
    json!({"roster":roster,"conversations":conversations,
        "calls":reaction_recovery_calls_image(runtime),"directories":reaction_recovery_directories(root)})
}

fn reaction_recovery_events(root: &std::path::Path) -> (
    Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    mahayana_host_runtime::transcript_mutation_events::TranscriptMutationSubscription,
) {
    let origin_root = fs::canonicalize(root.join("agents/.conversations")).expect("owned event origin");
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = Arc::clone(&events);
    let subscription = subscribe_transcript_mutations(move |mutation| {
        if mutation.get(TRANSCRIPT_MUTATION_ORIGIN_KEY).and_then(serde_json::Value::as_str)
            .is_some_and(|origin| std::path::Path::new(origin).starts_with(&origin_root))
        {
            captured.lock().expect("capture scoped recovery event")
                .push(serde_json::Value::Object(mutation.clone()));
        }
    });
    (events, subscription)
}

fn reaction_recovery_complete_credential(root: &std::path::Path, url: &str) -> serde_json::Value {
    let expires = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 + 600_000;
    let value = json!({
        "accessToken":"fixture-token","deviceId":"device-a","userId":1,
        "sessionId":"original-recovery-session","expiresAtMs":expires,"issuer":url
    });
    // The existing fixture already created a private 0600 file.
    fs::write(root.join("host-credential.json"), value.to_string()).expect("complete six-field credential");
    value
}

fn reaction_recovery_rotated_credential(original: &serde_json::Value, field: &str) -> serde_json::Value {
    let mut value = original.clone();
    value[field] = match field {
        "userId" => json!(9),
        "deviceId" => json!("rotated-recovery-device"),
        "accessToken" => json!("rotated-recovery-token"),
        "sessionId" => json!("rotated-recovery-session"),
        "expiresAtMs" => json!(original["expiresAtMs"].as_u64().unwrap() + 60_000),
        "issuer" => json!("http://127.0.0.1:1"),
        _ => panic!("unknown credential field"),
    };
    value
}

fn assert_reaction_recovery_requests(requests: &[String], expected: &[String]) {
    assert_eq!(requests.len(), expected.len(), "exact recovery HTTP count");
    for (request, expected) in requests.iter().zip(expected) {
        assert!(request.starts_with(&format!("{expected} ")), "unexpected route; expected {expected}");
        let lower = request.to_ascii_lowercase();
        assert!(lower.contains("authorization: bearer fixture-token"));
        assert!(lower.contains("x-fabushi-device-id: device-a"));
        if expected.starts_with("POST ") && expected.ends_with("/reactions") {
            assert_eq!(fixture_request_json(request), json!({"emoji":"👍","active":true}),
                "replay the original desired state, never retoggle");
        }
    }
}

#[test]
fn shipping_human_reaction_rejection_preserves_intent_while_history_progresses_after_restart() {
    for status in [404, 409] {
        let root = temp_root(&format!("reaction-refusal-history-{status}"));
        let (stop, stopped) = std::sync::mpsc::channel();
        let ack = json!({"success":true,"messageId":17,
            "reactions":[{"emoji":"👍","count":1,"reactedByMe":true}]});
        let (url, server) = spawn_human_message_fixture(vec![
            reaction_recovery_refusal(status),
            reaction_recovery_refusal(status),
            reaction_recovery_history(2, 18, "history despite refused reaction"),
            reaction_recovery_refusal(status),
            reaction_recovery_history(2, 19, "later restart history"),
            HumanMessageFixtureReply::Json(200, ack),
            HumanMessageFixtureReply::ObserveUntilStopped(stopped),
        ]);
        let runtime = human_message_fixture_runtime(&root, &url);
        let (id, original, intent) = reaction_recovery_seed(&runtime, "2", 17, false);
        let target = original["id"].as_str().unwrap();
        assert_eq!(runtime.toggle_human_message_reaction(&id, target, "👍").unwrap_err(),
            reaction_recovery_error(status), "direct command keeps its original refusal");
        let owner = runtime.open_human_conversation_db_owner(&id).unwrap();
        assert_eq!(owner.get_metadata("humanReactionIntent").unwrap(), Some(intent.clone()));
        assert_eq!(owner.get_transcript_entries().unwrap(), vec![original.clone()]);
        runtime.shutdown(); drop(owner); drop(runtime);

        let restarted = human_message_fixture_runtime(&root, &url);
        let owner = restarted.open_human_conversation_db_owner(&id).unwrap();
        let (events, subscription) = reaction_recovery_events(&root);
        assert_eq!(restarted.sync_human_conversation(&id).unwrap_err(), reaction_recovery_error(status));
        let first_progress = owner.get_transcript_entries().unwrap();
        assert_eq!(human_row_ids(&first_progress), ["human-server-message:17", "human-server-message:18"]);
        assert_eq!(first_progress[0], original);
        assert_eq!(first_progress[1]["content"], "history despite refused reaction");
        assert_eq!(owner.get_metadata("humanHistoryCoverage").unwrap(),
            Some(json!({"version":1,"afterId":18,"complete":true})));
        assert_eq!(owner.get_metadata("humanReactionIntent").unwrap(), Some(intent.clone()));
        let published = events.lock().unwrap().clone();
        assert_eq!(published.len(), 1, "only independent committed history reaches the real feed");
        assert_eq!(published[0]["entries"], json!([first_progress[1].clone()]));
        subscription.unsubscribe();
        restarted.shutdown(); drop(owner); drop(restarted);

        let reopened = human_message_fixture_runtime(&root, &url);
        let owner = reopened.open_human_conversation_db_owner(&id).unwrap();
        assert_eq!(owner.get_transcript_entries().unwrap(), first_progress, "pure reopen retains progress");
        assert_eq!(owner.get_metadata("humanReactionIntent").unwrap(), Some(intent.clone()));
        assert_eq!(reopened.sync_human_conversation(&id).unwrap_err(), reaction_recovery_error(status));
        let progressed = owner.get_transcript_entries().unwrap();
        assert_eq!(human_row_ids(&progressed),
            ["human-server-message:17", "human-server-message:18", "human-server-message:19"]);
        assert_eq!(owner.get_metadata("humanHistoryCoverage").unwrap(),
            Some(json!({"version":1,"afterId":19,"complete":true})));
        assert_eq!(owner.get_metadata("humanReactionIntent").unwrap(), Some(intent));
        let settled = reopened.toggle_human_message_reaction(&id, target, "👍")
            .expect("same occupied intent can later receive a real acknowledgement");
        assert_eq!(settled["remoteReactions"], json!([{"emoji":"👍","count":1,"reactedByMe":true}]));
        assert_eq!(owner.get_metadata("humanReactionIntent").unwrap(), Some(json!(null)));
        let final_rows = owner.get_transcript_entries().unwrap();
        assert_eq!(final_rows.len(), 3);
        assert_eq!(final_rows[1..], progressed[1..], "settlement preserves independent history");
        reopened.shutdown(); drop(owner); drop(reopened);
        let final_owner = human_message_fixture_runtime(&root, &url);
        let owner = final_owner.open_human_conversation_db_owner(&id).unwrap();
        assert_eq!(owner.get_transcript_entries().unwrap(), final_rows);
        assert_eq!(owner.get_metadata("humanReactionIntent").unwrap(), Some(json!(null)));
        final_owner.shutdown(); drop(owner); drop(final_owner);
        stop.send(()).unwrap();
        let requests = server.join().expect("live history observer");
        assert_reaction_recovery_requests(&requests, &[
            "POST /api/social/messages/17/reactions".into(),
            "POST /api/social/messages/17/reactions".into(),
            "GET /api/social/messages?contactId=2&limit=200&afterId=0".into(),
            "POST /api/social/messages/17/reactions".into(),
            "GET /api/social/messages?contactId=2&limit=200&afterId=0".into(),
            "POST /api/social/messages/17/reactions".into(),
        ]);
        fs::remove_dir_all(root).expect("remove owned reaction recovery fixture");
    }
}

#[test]
fn shipping_human_roster_reaction_rejection_does_not_starve_other_conversations_or_calls() {
    let root = temp_root("reaction-refusal-roster-calls");
    let call_id = "reaction-recovery-incoming-call";
    let (stop, stopped) = std::sync::mpsc::channel();
    let (url, server) = spawn_human_message_fixture(vec![
        // A repeated roster peer must not replay an occupied reaction twice.
        HumanMessageFixtureReply::Json(200, reaction_recovery_friends(&[2, 2, 3])),
        reaction_recovery_refusal(409),
        reaction_recovery_history(2, 18, "first peer history"),
        reaction_recovery_history(3, 31, "second peer history"),
        HumanMessageFixtureReply::Json(200, json!({"success":true,
            "calls":[reaction_recovery_remote_call(call_id, 4, 1)]})),
        HumanMessageFixtureReply::Json(200, reaction_recovery_call_body(call_id, 4)),
        HumanMessageFixtureReply::ObserveUntilStopped(stopped),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let (id, original, intent) = reaction_recovery_seed(&runtime, "2", 17, true);
    let owner = runtime.open_human_conversation_db_owner(&id).unwrap();
    let (events, subscription) = reaction_recovery_events(&root);
    assert_eq!(runtime.sync_human_conversations().unwrap_err(), reaction_recovery_error(409));
    let roster = runtime.list_human_conversations().expect("UI local roster fallback");
    assert_eq!(roster.len(), 3, "both friends and the independently discovered call peer");
    let find_peer = |peer: &str| roster.iter().find(|row|
        row["participantIds"].as_array().unwrap().contains(&json!(peer)))
        .expect("actual canonical peer")["id"].as_str().unwrap().to_owned();
    let second = find_peer("3");
    let call_scope = find_peer("4");
    let first_rows = owner.get_transcript_entries().unwrap();
    assert_eq!(human_row_ids(&first_rows), ["human-server-message:17", "human-server-message:18"]);
    assert_eq!(first_rows[0], original);
    assert_eq!(owner.get_metadata("humanReactionIntent").unwrap(), Some(intent.clone()));
    assert_eq!(owner.get_metadata("humanHistoryCoverage").unwrap(),
        Some(json!({"version":1,"afterId":18,"complete":true})));
    let second_owner = runtime.open_human_conversation_db_owner(&second).unwrap();
    assert_eq!(human_row_ids(&second_owner.get_transcript_entries().unwrap()), ["human-server-message:31"]);
    assert_eq!(second_owner.get_metadata("humanHistoryCoverage").unwrap(),
        Some(json!({"version":1,"afterId":31,"complete":true})));
    let calls = reaction_recovery_calls_image(&runtime);
    assert_eq!(calls["call_sessions"].as_array().unwrap().len(), 1);
    assert_eq!(calls["call_sessions"][0][0], call_id);
    assert_eq!(calls["call_sessions"][0][1], call_scope);
    assert_eq!(calls["call_sessions"][0][3], "negotiating");
    assert_eq!(calls["call_sessions"][0][4], 0);
    assert_eq!(calls["call_remote_sync"], json!([[call_id, 1]]));
    assert_eq!(calls["call_signals"], json!([]));
    let published = events.lock().unwrap().clone();
    assert_eq!(published.len(), 2, "only the two real committed history rows");
    assert!(published.iter().any(|event| event["entries"][0]["id"] == "human-server-message:18"));
    assert!(published.iter().any(|event| event["entries"][0]["id"] == "human-server-message:31"));
    let durable = reaction_recovery_state(&runtime, &root);
    subscription.unsubscribe();
    runtime.shutdown(); drop(second_owner); drop(owner); drop(runtime);
    let reopened = human_message_fixture_runtime(&root, &url);
    assert_eq!(reaction_recovery_state(&reopened, &root), durable, "roster/history/Calls survive pure reopen");
    let owner = reopened.open_human_conversation_db_owner(&id).unwrap();
    assert_eq!(owner.get_metadata("humanReactionIntent").unwrap(), Some(intent));
    reopened.shutdown(); drop(owner); drop(reopened);
    stop.send(()).unwrap();
    assert_reaction_recovery_requests(&server.join().expect("roster continuation observer"), &[
        "GET /api/social/friends".into(),
        "POST /api/social/messages/17/reactions".into(),
        "GET /api/social/messages?contactId=2&limit=200&afterId=0".into(),
        "GET /api/social/messages?contactId=3&limit=200&afterId=0".into(),
        "GET /api/social/calls?limit=200".into(),
        format!("GET /api/social/calls/{call_id}?afterSeq=0&limit=200"),
    ]);
    fs::remove_dir_all(root).expect("remove owned roster continuation fixture");
}


#[test]
fn shipping_human_reaction_rejection_continuation_refuses_changed_credentials_before_downstream_work() {
    // Every field starts present, and both expiry values remain in the future.
    // This is an unchanged-invocation test, not merely an expired-token test.
    let fields = ["userId", "deviceId", "accessToken", "sessionId", "expiresAtMs", "issuer"];
    for stage in ["reaction", "history", "empty-history", "latest-history", "calls-list", "calls-get"] {
        for field in fields {
            let root = temp_root(&format!("reaction-continuation-{stage}-{field}"));
            let (started, body_started) = std::sync::mpsc::channel();
            let (release, resume) = std::sync::mpsc::channel();
            let (stop, stopped) = std::sync::mpsc::channel();
            let call_id = "held-recovery-call";
            let mut replies = vec![HumanMessageFixtureReply::Json(200, reaction_recovery_friends(&[2, 3]))];
            let mut routes = vec!["GET /api/social/friends".to_string()];
            if stage == "reaction" {
                replies.push(HumanMessageFixtureReply::StreamingJson(404,
                    json!({"success":false,"error":"retained reaction refusal"}), started, resume));
                routes.push("POST /api/social/messages/17/reactions".into());
            } else {
                replies.push(reaction_recovery_refusal(404));
                routes.push("POST /api/social/messages/17/reactions".into());
                match stage {
                    "history" | "empty-history" => {
                        let messages = if stage == "empty-history" { vec![] } else {
                            vec![fixture_remote_human_message(18, 2, 1, "stale history body", None, json!([]))]
                        };
                        replies.push(HumanMessageFixtureReply::StreamingJson(200,
                            json!({"success":true,"data":{"messages":messages}}), started, resume));
                        routes.push("GET /api/social/messages?contactId=2&limit=200&afterId=0".into());
                    }
                    "latest-history" => {
                        replies.push(human_history_reply(vec![]));
                        routes.push("GET /api/social/messages?contactId=2&limit=200&afterId=200".into());
                        replies.push(HumanMessageFixtureReply::StreamingJson(200,
                            json!({"success":true,"data":{"messages":[
                                fixture_remote_human_message(17, 2, 1, "stale latest refresh", None, json!([]))
                            ]}}), started, resume));
                        routes.push("GET /api/social/messages?contactId=2&limit=200".into());
                    }
                    "calls-list" | "calls-get" => {
                        replies.push(reaction_recovery_history(2, 18, "authorized prefix peer2"));
                        replies.push(reaction_recovery_history(3, 31, "authorized prefix peer3"));
                        routes.push("GET /api/social/messages?contactId=2&limit=200&afterId=0".into());
                        routes.push("GET /api/social/messages?contactId=3&limit=200&afterId=0".into());
                        let body = json!({"success":true,"calls":[reaction_recovery_remote_call(call_id, 4, 1)]});
                        routes.push("GET /api/social/calls?limit=200".into());
                        if stage == "calls-list" {
                            replies.push(HumanMessageFixtureReply::StreamingJson(200, body, started, resume));
                        } else {
                            replies.push(HumanMessageFixtureReply::Json(200, body));
                            replies.push(HumanMessageFixtureReply::StreamingJson(200,
                                reaction_recovery_call_body(call_id, 4), started, resume));
                            routes.push(format!("GET /api/social/calls/{call_id}?afterSeq=0&limit=200"));
                        }
                    }
                    _ => unreachable!(),
                }
            }
            replies.push(HumanMessageFixtureReply::ObserveUntilStopped(stopped));
            let (url, server) = spawn_human_message_fixture(replies);
            let runtime = human_message_fixture_runtime(&root, &url);
            let (id, _, intent) = reaction_recovery_seed(&runtime, "2", 17, true);
            let owner = runtime.open_human_conversation_db_owner(&id).unwrap();
            if stage == "latest-history" {
                assert!(owner.set_metadata("humanHistoryCoverage",
                    json!({"version":1,"afterId":200,"complete":true})).unwrap());
            }
            let credential = reaction_recovery_complete_credential(&root, &url);
            let (events, subscription) = reaction_recovery_events(&root);
            let running = {
                let runtime = Arc::clone(&runtime);
                thread::spawn(move || runtime.sync_human_conversations())
            };
            let observed = body_started.recv_timeout(std::time::Duration::from_secs(15));
            // Already-authorized histories and a pre-get invited call form the
            // allowed prefix. Nothing after the rotated binding may change it.
            let before = reaction_recovery_state(&runtime, &root);
            let before_events = events.lock().unwrap().clone();
            let rotated = reaction_recovery_rotated_credential(&credential, field);
            let changed = fs::write(root.join("host-credential.json"), rotated.to_string());
            let released = release.send(());
            let result = running.join();
            let stopped = stop.send(());
            let requests = server.join();
            observed.expect("HTTP headers and first body byte arrived");
            changed.expect("rotate exactly one complete credential field");
            released.expect("always release held response before assertions");
            let error = result.expect("real roster worker").expect_err("retired binding must fail");
            stopped.expect("stop live downstream observer");
            assert!(error.contains("credential") || error.contains("issuer") || error.contains("origin"),
                "{stage}/{field}: {error}");
            assert_ne!(error, reaction_recovery_error(404), "credential failure outranks saved refusal");
            assert_eq!(reaction_recovery_state(&runtime, &root), before, "{stage}/{field}: no downstream write");
            assert_eq!(*events.lock().unwrap(), before_events, "{stage}/{field}: no downstream event");
            assert_eq!(owner.get_metadata("humanReactionIntent").unwrap(), Some(intent));
            assert_reaction_recovery_requests(&requests.expect("live body observer"), &routes);
            subscription.unsubscribe();
            runtime.shutdown(); drop(owner); drop(runtime);
            fs::remove_dir_all(root).expect("remove credential response-boundary fixture");
        }
    }

    // These private-file failures happen while the admitted 404 body is held,
    // and must not be downgraded to a recoverable service refusal.
    for fault in ["missing-file", "invalid-json", "nonregular-file"] {
        let root = temp_root(&format!("reaction-continuation-{fault}"));
        let (started, body_started) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let (stop, stopped) = std::sync::mpsc::channel();
        let (url, server) = spawn_human_message_fixture(vec![
            HumanMessageFixtureReply::Json(200, reaction_recovery_friends(&[2, 3])),
            HumanMessageFixtureReply::StreamingJson(409,
                json!({"success":false,"error":"retained reaction refusal"}), started, resume),
            HumanMessageFixtureReply::ObserveUntilStopped(stopped),
        ]);
        let runtime = human_message_fixture_runtime(&root, &url);
        let (_, _, _) = reaction_recovery_seed(&runtime, "2", 17, true);
        reaction_recovery_complete_credential(&root, &url);
        let before = reaction_recovery_state(&runtime, &root);
        let (events, subscription) = reaction_recovery_events(&root);
        let running = { let runtime = Arc::clone(&runtime);
            thread::spawn(move || runtime.sync_human_conversations()) };
        let observed = body_started.recv_timeout(std::time::Duration::from_secs(15));
        let path = root.join("host-credential.json");
        let changed = (|| -> std::io::Result<()> {
            match fault {
                "missing-file" => fs::remove_file(&path),
                "invalid-json" => fs::write(&path, b"{"),
                "nonregular-file" => { fs::remove_file(&path)?; fs::create_dir(&path) },
                _ => unreachable!(),
            }
        })();
        let released = release.send(());
        let result = running.join();
        let stopped = stop.send(());
        let requests = server.join();
        observed.expect("held rejection body");
        changed.expect("private credential fault");
        released.expect("release private-file refusal fixture");
        let error = result.unwrap().expect_err("private credential failure remains hard");
        stopped.unwrap();
        assert!(error.contains("credential"), "{fault}: {error}");
        assert_ne!(error, reaction_recovery_error(409));
        assert_eq!(reaction_recovery_state(&runtime, &root), before);
        assert!(events.lock().unwrap().is_empty());
        assert_reaction_recovery_requests(&requests.unwrap(), &[
            "GET /api/social/friends".into(), "POST /api/social/messages/17/reactions".into(),
        ]);
        subscription.unsubscribe();
        runtime.shutdown(); drop(runtime);
        fs::remove_dir_all(root).expect("remove owned credential-fault fixture");
    }

    // Rotation by an actual committed-row observer is after the request's
    // Native stability check. Rebinding before the next friend would incorrectly
    // accept it. No internal controller state or synthetic mutation is used.
    for field in fields {
        let root = temp_root(&format!("reaction-continuation-postcommit-{field}"));
        let (stop, stopped) = std::sync::mpsc::channel();
        let (url, server) = spawn_human_message_fixture(vec![
            HumanMessageFixtureReply::Json(200, reaction_recovery_friends(&[2, 3])),
            reaction_recovery_refusal(404),
            reaction_recovery_history(2, 18, "commit under original credential"),
            HumanMessageFixtureReply::ObserveUntilStopped(stopped),
        ]);
        let runtime = human_message_fixture_runtime(&root, &url);
        let (id, original, intent) = reaction_recovery_seed(&runtime, "2", 17, true);
        let owner = runtime.open_human_conversation_db_owner(&id).unwrap();
        let credential = reaction_recovery_complete_credential(&root, &url);
        let rotated = reaction_recovery_rotated_credential(&credential, field);
        let calls = reaction_recovery_calls_image(&runtime);
        let dirs = reaction_recovery_directories(&root);
        let original_metadata = owner.read_metadata().unwrap();
        let origin = transcript_mutation_origin(owner.db_path()).expect("exact original SQLite origin");
        let credential_path = root.join("host-credential.json");
        let changed = Arc::new(std::sync::Mutex::new(None::<Result<(), String>>));
        let callback_result = Arc::clone(&changed);
        let expected_id = id.clone();
        let (events, event_subscription) = reaction_recovery_events(&root);
        let rotation_subscription = subscribe_transcript_mutations(move |mutation| {
            if mutation.get(TRANSCRIPT_MUTATION_ORIGIN_KEY).and_then(serde_json::Value::as_str) != Some(origin.as_str())
                || mutation.get("agentId").and_then(serde_json::Value::as_str) != Some(expected_id.as_str())
                || !mutation.get("entries").and_then(serde_json::Value::as_array)
                    .is_some_and(|rows| rows.iter().any(|row| row["id"] == "human-server-message:18"))
            { return; }
            if let Ok(mut observed) = callback_result.lock() {
                if observed.is_none() {
                    *observed = Some(fs::write(&credential_path, rotated.to_string()).map_err(|error| error.to_string()));
                }
            }
        });
        let result = runtime.sync_human_conversations();
        stop.send(()).unwrap();
        let requests = server.join().expect("post-commit continuation observer");
        let error = result.expect_err("the invocation cannot obtain a fresh lease for its next friend");
        changed.lock().unwrap().as_ref().expect("actual commit callback executed").as_ref()
            .expect("credential rotation succeeded outside swallowed callback assertions");
        assert!(error.contains("credential") || error.contains("issuer") || error.contains("origin"), "{field}: {error}");
        assert_ne!(error, reaction_recovery_error(404));
        let rows = owner.get_transcript_entries().unwrap();
        assert_eq!(human_row_ids(&rows), ["human-server-message:17", "human-server-message:18"]);
        assert_eq!(rows[0], original);
        assert_eq!(rows[1]["content"], "commit under original credential");
        assert_eq!(owner.get_metadata("humanReactionIntent").unwrap(), Some(intent));
        assert!(owner.get_metadata("humanHistoryCoverage").unwrap().is_none(), "no checkpoint after revoked binding");
        let mut permitted_metadata = original_metadata;
        permitted_metadata["lastActivityAt"] = rows[1]["timestampMs"].clone();
        let actual_metadata = owner.read_metadata().unwrap();
        if let Some(rev) = actual_metadata.get("lifecycleRevision") {
            permitted_metadata["lifecycleRevision"] = rev.clone();
        }
        assert_eq!(actual_metadata, permitted_metadata,
            "only the already admitted materialization/activity unit may finish");
        assert_eq!(reaction_recovery_calls_image(&runtime), calls);
        assert_eq!(reaction_recovery_directories(&root), dirs, "next friend is not created");
        assert_eq!(events.lock().unwrap().len(), 1);
        assert_reaction_recovery_requests(&requests, &[
            "GET /api/social/friends".into(), "POST /api/social/messages/17/reactions".into(),
            "GET /api/social/messages?contactId=2&limit=200&afterId=0".into(),
        ]);
        rotation_subscription.unsubscribe(); event_subscription.unsubscribe();
        runtime.shutdown(); drop(owner); drop(runtime);
        fs::remove_dir_all(root).expect("remove post-commit binding fixture");
    }

    // Admission is repeated after an HTTP refusal. The fixture's independent
    // mutation remains authoritative; the recovery path must not undo it.
    for change in ["target-identity", "target-author", "target-author-local", "target-delivery", "participants",
        "intent", "closed-owner", "root-file"] {
        let root = temp_root(&format!("reaction-refusal-readmission-{change}"));
        let (started, body_started) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let (stop, stopped) = std::sync::mpsc::channel();
        let (url, server) = spawn_human_message_fixture(vec![
            HumanMessageFixtureReply::Json(200, reaction_recovery_friends(&[2, 3])),
            HumanMessageFixtureReply::StreamingJson(404,
                json!({"success":false,"error":"retained reaction refusal"}), started, resume),
            HumanMessageFixtureReply::ObserveUntilStopped(stopped),
        ]);
        let runtime = human_message_fixture_runtime(&root, &url);
        let (id, original, intent) = reaction_recovery_seed(&runtime, "2", 17, true);
        let owner = runtime.open_human_conversation_db_owner(&id).unwrap();
        if change == "closed-owner" {
            assert_eq!(mahayana_host_runtime::storage::store_db::live_db_handle_count(owner.db_path()), 1,
                "the fixture starts with exactly its original registered owner");
        }
        let calls = reaction_recovery_calls_image(&runtime);
        let dirs = reaction_recovery_directories(&root);
        let mut retained_metadata = owner.read_metadata().unwrap();
        let mut retained_rows = vec![original.clone()];
        let mut retained_intent = intent;
        let (events, subscription) = reaction_recovery_events(&root);
        let running = { let runtime = Arc::clone(&runtime);
            thread::spawn(move || runtime.sync_human_conversations()) };
        let observed = body_started.recv_timeout(std::time::Duration::from_secs(15));
        let changed = (|| -> Result<(), String> {
            match change {
                "target-identity" | "target-author" | "target-author-local" | "target-delivery" => {
                    let mut current = original.clone();
                    match change {
                        "target-identity" => current["remoteMessageId"] = json!("999"),
                        "target-author" => current["authorId"] = json!("9"),
                        "target-author-local" => current["authorId"] = json!("1"),
                        _ => current["delivery"] = json!("pending"),
                    }
                    owner.update_transcript_entry(original["id"].as_str().unwrap(), &current)
                        .map_err(|error| error.to_string())?.ok_or("target disappeared")?;
                    retained_rows = vec![current];
                }
                "participants" => {
                    owner.set_metadata("participantIds", json!(["1", "9"])).map_err(|error| error.to_string())?;
                    retained_metadata = owner.read_metadata().map_err(|error| error.to_string())?;
                }
                "intent" => {
                    retained_intent["active"] = json!(false);
                    owner.set_metadata("humanReactionIntent", retained_intent.clone()).map_err(|error| error.to_string())?;
                    retained_metadata = owner.read_metadata().map_err(|error| error.to_string())?;
                }
                "closed-owner" => runtime.shutdown(),
                "root-file" => {
                    fs::rename(root.join("agents/.conversations"), root.join("retained-conversations"))
                        .map_err(|error| error.to_string())?;
                    fs::write(root.join("agents/.conversations"), b"owned root fault")
                        .map_err(|error| error.to_string())?;
                }
                _ => unreachable!(),
            }
            Ok(())
        })();
        let fixture_events = events.lock().unwrap().clone();
        let closed_handle_count = (change == "closed-owner").then(||
            mahayana_host_runtime::storage::store_db::live_db_handle_count(owner.db_path()));
        let released = release.send(());
        let result = running.join();
        let stopped = stop.send(());
        let requests = server.join();
        if change == "root-file" {
            fs::remove_file(root.join("agents/.conversations")).expect("remove only owned fault");
            fs::rename(root.join("retained-conversations"), root.join("agents/.conversations"))
                .expect("restore original owned root");
        }
        observed.expect("held refusal before independent mutation");
        changed.expect("change admitted owner or target");
        released.expect("release response before observing refusal");
        let error = result.unwrap().expect_err("changed admission cannot authorize recovery");
        stopped.unwrap();
        assert_ne!(error, reaction_recovery_error(404), "{change}: admission error takes precedence");
        assert!(!error.is_empty());
        assert_eq!(*events.lock().unwrap(), fixture_events, "no extra recovery event: {change}");
        if let Some(after_close) = closed_handle_count {
            assert_eq!(after_close, 0, "close released the original registered handle");
            assert!(owner.is_closed(), "the original Arc remains closed");
            assert_eq!(mahayana_host_runtime::storage::store_db::live_db_handle_count(owner.db_path()), 0,
                "refusal must not cold-open and register a replacement owner");
        }
        assert_eq!(reaction_recovery_calls_image(&runtime), calls, "no Calls effect: {change}");
        assert_eq!(reaction_recovery_directories(&root), dirs, "no later friend: {change}");
        assert_reaction_recovery_requests(&requests.unwrap(), &[
            "GET /api/social/friends".into(), "POST /api/social/messages/17/reactions".into(),
        ]);
        subscription.unsubscribe();
        runtime.shutdown(); drop(owner); drop(runtime);
        let reopened = human_message_fixture_runtime(&root, &url);
        // Direct READ_ONLY inspection also works when the fixture intentionally
        // left invalid participants; opening through Session must then refuse.
        let db = rusqlite::Connection::open_with_flags(forward_db_path(&root, &id),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).expect("retained invalid-state observer");
        let mut statement = db.prepare("SELECT entry FROM transcript_entries ORDER BY rowid")
            .expect("existing transcript table");
        let stored_rows = statement.query_map([], |row| row.get::<_, String>(0)).unwrap()
            .map(|row| serde_json::from_str::<serde_json::Value>(&row.unwrap()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(stored_rows, retained_rows, "independent target change survives restart: {change}");
        let metadata = read_forward_metadata_from_disk(&root, &id);
        assert_eq!(metadata, retained_metadata, "no metadata rewrite: {change}");
        assert_eq!(metadata["humanReactionIntent"], retained_intent);
        assert_eq!(reaction_recovery_calls_image(&reopened), calls);
        drop(statement); drop(db);
        reopened.shutdown(); drop(reopened);
        fs::remove_dir_all(root).expect("remove owner readmission fixture");
    }
}


#[test]
fn shipping_human_reaction_rejection_keeps_admission_and_protocol_failures_fail_closed() {
    let responses = vec![
        ("401", HumanMessageFixtureReply::Json(401, json!({"success":false,"error":"denied"})), "HTTP 401"),
        ("403", HumanMessageFixtureReply::Json(403, json!({"success":false,"error":"denied"})), "HTTP 403"),
        ("503-forged-404", HumanMessageFixtureReply::Json(503,
            json!({"success":false,"error":"provider text says HTTP 404; do not classify strings"})), "HTTP 503"),
        ("invalid-json-404", HumanMessageFixtureReply::Bytes(404, b"not-json".to_vec()), "invalid JSON"),
        ("truncated-409", HumanMessageFixtureReply::Truncated(409), "could not be read"),
        ("disconnect", HumanMessageFixtureReply::Disconnect, "mutation failed"),
        ("missing-success", HumanMessageFixtureReply::Json(404, json!({"error":"missing boolean"})), "invalid JSON"),
        ("null-success", HumanMessageFixtureReply::Json(409, json!({"success":null})), "invalid JSON"),
        ("string-success", HumanMessageFixtureReply::Json(404, json!({"success":"false"})), "invalid JSON"),
        ("contradictory-success", HumanMessageFixtureReply::Json(404,
            json!({"success":true,"messageId":17,"reactions":[]})), "HTTP 404"),
        ("false-200", HumanMessageFixtureReply::Json(200,
            json!({"success":false,"error":"retained reaction refusal"})), "HTTP 200"),
        ("false-with-message-id", HumanMessageFixtureReply::Json(404,
            json!({"success":false,"error":"retained reaction refusal","messageId":17})), "HTTP 404"),
        ("false-with-summary", HumanMessageFixtureReply::Json(409,
            json!({"success":false,"error":"retained reaction refusal","reactions":[]})), "HTTP 409"),
        ("false-with-success-fields", HumanMessageFixtureReply::Json(404,
            json!({"success":false,"error":"retained reaction refusal","messageId":17,
                "reactions":[{"emoji":"👍","count":1,"reactedByMe":true}]})), "HTTP 404"),
        ("ack-missing-id", HumanMessageFixtureReply::Json(200,
            json!({"success":true,"reactions":[{"emoji":"👍","count":1,"reactedByMe":true}]})), "omitted the message id"),
        ("ack-wrong-id", HumanMessageFixtureReply::Json(200,
            json!({"success":true,"messageId":18,"reactions":[{"emoji":"👍","count":1,"reactedByMe":true}]})), "mismatched message id"),
        ("ack-missing-summary", HumanMessageFixtureReply::Json(200,
            json!({"success":true,"messageId":17})), "omitted the reaction summary"),
        ("ack-unconfirmed", HumanMessageFixtureReply::Json(200,
            json!({"success":true,"messageId":17,"reactions":[]})), "did not confirm"),
        ("ack-invalid-summary", HumanMessageFixtureReply::Json(200,
            json!({"success":true,"messageId":17,"reactions":[{"emoji":"👍","count":0,"reactedByMe":true}]})), "reaction"),
    ];
    for (label, response, expected_error) in responses {
        let root = temp_root(&format!("reaction-hard-protocol-{label}"));
        let (stop, stopped) = std::sync::mpsc::channel();
        let (url, server) = spawn_human_message_fixture(vec![
            HumanMessageFixtureReply::Json(200, reaction_recovery_friends(&[2, 3])),
            response,
            HumanMessageFixtureReply::ObserveUntilStopped(stopped),
        ]);
        let runtime = human_message_fixture_runtime(&root, &url);
        let _ = reaction_recovery_seed(&runtime, "2", 17, true);
        let before = reaction_recovery_state(&runtime, &root);
        let (events, subscription) = reaction_recovery_events(&root);
        let error = runtime.sync_human_conversations().expect_err("only qualified rejection may continue");
        stop.send(()).unwrap();
        let requests = server.join().expect("hard failure live observer");
        assert!(error.contains(expected_error), "{label}: {error}");
        assert_eq!(reaction_recovery_state(&runtime, &root), before,
            "no optimistic reaction, history, cursor, friend or Calls effect: {label}");
        assert!(events.lock().unwrap().is_empty(), "{label}");
        assert_reaction_recovery_requests(&requests, &[
            "GET /api/social/friends".into(), "POST /api/social/messages/17/reactions".into(),
        ]);
        subscription.unsubscribe();
        runtime.shutdown(); drop(runtime);
        let reopened = human_message_fixture_runtime(&root, &url);
        assert_eq!(reaction_recovery_state(&reopened, &root), before, "hard failure retains exact durable input: {label}");
        reopened.shutdown(); drop(reopened);
        fs::remove_dir_all(root).expect("remove protocol refusal fixture");
    }

    // A's refusal puts the whole invocation into strict continuation. B has no
    // reaction: its failure cannot be swallowed by the old normal-history path.
    for failure in ["bad-pending", "foreign-history", "history-json", "history-401", "db-write"] {
        let root = temp_root(&format!("reaction-strict-later-friend-{failure}"));
        let (stop, stopped) = std::sync::mpsc::channel();
        let mut responses = vec![
            HumanMessageFixtureReply::Json(200, reaction_recovery_friends(&[2, 3, 4])),
            reaction_recovery_refusal(404),
            reaction_recovery_history(2, 18, "A's permitted prefix"),
        ];
        let expected_error = match failure {
            "bad-pending" => "Human queuedOffline origin must be a boolean",
            "foreign-history" => {
                responses.push(human_history_reply(vec![
                    fixture_remote_human_message(31, 3, 1, "valid B prefix must not apply", None, json!([])),
                    fixture_remote_human_message(32, 9, 1, "foreign B row", None, json!([])),
                ]));
                "remote Human message does not belong to the selected Human conversation"
            }
            "history-json" => {
                responses.push(HumanMessageFixtureReply::Bytes(200, b"not-json".to_vec()));
                "invalid JSON"
            }
            "history-401" => {
                responses.push(HumanMessageFixtureReply::Json(401, json!({"success":false,"error":"history revoked"})));
                "HTTP 401"
            }
            "db-write" => {
                responses.push(reaction_recovery_history(3, 31, "B row whose SQL write fails"));
                "NMR-REACTION-RECOVERY injected row failure"
            }
            _ => unreachable!(),
        };
        responses.push(HumanMessageFixtureReply::ObserveUntilStopped(stopped));
        let (url, server) = spawn_human_message_fixture(responses);
        let runtime = human_message_fixture_runtime(&root, &url);
        let (first_id, original, intent) = reaction_recovery_seed(&runtime, "2", 17, true);
        let second_id = runtime.create_human_conversation("3", "B has no reaction").unwrap()["id"]
            .as_str().unwrap().to_owned();
        let first_owner = runtime.open_human_conversation_db_owner(&first_id).unwrap();
        let second_owner = runtime.open_human_conversation_db_owner(&second_id).unwrap();
        if failure == "bad-pending" {
            let mut pending = pending_reply_fixture_row("strict-later-friend", "retain invalid pending origin", None);
            pending["queuedOffline"] = json!("true");
            assert!(second_owner.append_transcript_entry(&pending).unwrap());
        }
        assert!(second_owner.get_metadata("humanReactionIntent").unwrap().is_none());
        let second_rows = second_owner.get_transcript_entries().unwrap();
        let second_metadata = second_owner.read_metadata().unwrap();
        let dirs = reaction_recovery_directories(&root);
        let calls = reaction_recovery_calls_image(&runtime);
        let fault_db = if failure == "db-write" {
            let db = rusqlite::Connection::open(forward_db_path(&root, &second_id)).unwrap();
            db.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
            db.execute_batch("CREATE TRIGGER reject_recovery_row BEFORE INSERT ON transcript_entries
                WHEN NEW.id = 'human-server-message:31'
                BEGIN SELECT RAISE(ABORT, 'NMR-REACTION-RECOVERY injected row failure'); END;")
                .expect("reject actual B SQLite write");
            Some(db)
        } else { None };
        let (events, subscription) = reaction_recovery_events(&root);
        let error = runtime.sync_human_conversations().expect_err("later hard failure must terminate strict continuation");
        stop.send(()).unwrap();
        let requests = server.join().expect("later-friend live observer");
        assert!(error.contains(expected_error), "{failure}: {error}");
        assert_ne!(error, reaction_recovery_error(404), "later original error takes precedence");
        let first_rows = first_owner.get_transcript_entries().unwrap();
        assert_eq!(human_row_ids(&first_rows), ["human-server-message:17", "human-server-message:18"]);
        assert_eq!(first_rows[0], original);
        assert_eq!(first_owner.get_metadata("humanReactionIntent").unwrap(), Some(intent));
        assert_eq!(first_owner.get_metadata("humanHistoryCoverage").unwrap(),
            Some(json!({"version":1,"afterId":18,"complete":true})));
        assert_eq!(second_owner.get_transcript_entries().unwrap(), second_rows, "{failure}: no B apply");
        assert_eq!(second_owner.read_metadata().unwrap(), second_metadata, "{failure}: no B checkpoint/activity");
        assert_eq!(reaction_recovery_directories(&root), dirs, "C is not created: {failure}");
        assert_eq!(reaction_recovery_calls_image(&runtime), calls, "Calls is not touched: {failure}");
        let published = events.lock().unwrap().clone();
        assert_eq!(published.len(), 1, "only A's prior valid commit is observable: {failure}");
        assert_eq!(published[0]["entries"], json!([first_rows[1].clone()]));
        let mut expected_routes = vec![
            "GET /api/social/friends".into(), "POST /api/social/messages/17/reactions".into(),
            "GET /api/social/messages?contactId=2&limit=200&afterId=0".into(),
        ];
        if failure != "bad-pending" {
            expected_routes.push("GET /api/social/messages?contactId=3&limit=200&afterId=0".into());
        }
        assert_reaction_recovery_requests(&requests, &expected_routes);
        if let Some(db) = fault_db {
            db.execute_batch("DROP TRIGGER reject_recovery_row;").expect("remove only fixture trigger");
        }
        let durable = reaction_recovery_state(&runtime, &root);
        subscription.unsubscribe();
        runtime.shutdown(); drop(second_owner); drop(first_owner); drop(runtime);
        let reopened = human_message_fixture_runtime(&root, &url);
        assert_eq!(reaction_recovery_state(&reopened, &root), durable, "valid prefix and denied B survive reopen");
        reopened.shutdown(); drop(reopened);
        fs::remove_dir_all(root).expect("remove later-friend strict fixture");
    }

    // The continued Calls lane must bind both event callId and the original
    // requested scope. A valid event for another locally authorized call is not
    // permission to mutate that call or advance the requested call's cursor.
    for failure in ["foreign-event", "foreign-event-duplicate", "changed-call-scope"] {
        let root = temp_root(&format!("reaction-strict-calls-{failure}"));
        let call_id = "strict-recovery-call-a";
        let other_call_id = "strict-recovery-call-b";
        let (started, body_started) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let (stop, stopped) = std::sync::mpsc::channel();
        let mut replay = reaction_recovery_call_body(call_id, 4);
        if failure.starts_with("foreign-event") {
            replay["events"][0]["callId"] = json!(other_call_id);
            if failure == "foreign-event-duplicate" {
                // Even an already-covered sequence must not smuggle another
                // call's event through the normal deduplication branch.
                replay["call"] = reaction_recovery_remote_call(call_id, 4, 0);
                replay["events"][0]["seq"] = json!(0);
                replay["nextAfterSeq"] = json!(0);
            }
        }
        let (url, server) = spawn_human_message_fixture(vec![
            HumanMessageFixtureReply::Json(200, reaction_recovery_friends(&[2])),
            reaction_recovery_refusal(404),
            reaction_recovery_history(2, 18, "valid history before hard Calls failure"),
            HumanMessageFixtureReply::Json(200, json!({"success":true,
                "calls":[reaction_recovery_remote_call(call_id, 4,
                    if failure == "foreign-event-duplicate" { 0 } else { 1 })]})),
            HumanMessageFixtureReply::StreamingJson(200, replay, started, resume),
            HumanMessageFixtureReply::ObserveUntilStopped(stopped),
        ]);
        let runtime = human_message_fixture_runtime(&root, &url);
        let (id, _, intent) = reaction_recovery_seed(&runtime, "2", 17, true);
        let scope = runtime.create_human_conversation("4", "Call A peer").unwrap()["id"]
            .as_str().unwrap().to_owned();
        let other_scope = runtime.create_human_conversation("5", "Call B peer").unwrap()["id"]
            .as_str().unwrap().to_owned();
        // Seed two ordinary records in the already initialized actual Calls
        // schema, as an older installed owner could have left them. No new store.
        let db = rusqlite::Connection::open(runtime.call_session_store_path().unwrap())
            .expect("owned Calls fixture writer");
        db.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
        for (call, scope, creator, peer) in [
            (call_id, scope.as_str(), "4", "4"),
            (other_call_id, other_scope.as_str(), "5", "5"),
        ] {
            db.execute("INSERT INTO call_sessions
                (id, scope_id, creator_id, state, generation, signal_seq, participant_ids_json,
                 media_capabilities_json, device_selection_json, created_at_ms, updated_at_ms)
                VALUES (?1, ?2, ?3, 'invited', 0, 0, ?4, '{}', '{}', 17, 17)",
                rusqlite::params![call, scope, creator, json!(["1", peer]).to_string()])
                .expect("seed actual authorized call");
        }
        let running = { let runtime = Arc::clone(&runtime);
            thread::spawn(move || runtime.sync_human_conversations()) };
        let observed = body_started.recv_timeout(std::time::Duration::from_secs(15));
        let changed = if failure == "changed-call-scope" {
            db.execute("UPDATE call_sessions SET scope_id = ?1 WHERE id = ?2",
                rusqlite::params![other_scope, call_id]).map(|count| count == 1)
        } else { Ok(true) };
        let retained_calls = reaction_recovery_calls_image(&runtime);
        let retained_state = reaction_recovery_state(&runtime, &root);
        let released = release.send(());
        let result = running.join();
        let stopped = stop.send(());
        let requests = server.join();
        observed.expect("Calls body held after discovery admission");
        assert!(changed.expect("independent Calls scope fixture mutation"));
        released.expect("release actual Calls body");
        let error = result.unwrap().expect_err("strict Calls binding must fail");
        stopped.unwrap();
        assert_ne!(error, reaction_recovery_error(404), "Calls error outranks deferred reaction");
        assert!(error.to_ascii_lowercase().contains("call"), "{failure}: {error}");
        assert_eq!(reaction_recovery_calls_image(&runtime), retained_calls,
            "neither call/event/cursor may be mutated: {failure}");
        assert_eq!(reaction_recovery_state(&runtime, &root), retained_state);
        let owner = runtime.open_human_conversation_db_owner(&id).unwrap();
        assert_eq!(owner.get_metadata("humanReactionIntent").unwrap(), Some(intent));
        assert_reaction_recovery_requests(&requests.unwrap(), &[
            "GET /api/social/friends".into(), "POST /api/social/messages/17/reactions".into(),
            "GET /api/social/messages?contactId=2&limit=200&afterId=0".into(),
            "GET /api/social/calls?limit=200".into(),
            format!("GET /api/social/calls/{call_id}?afterSeq=0&limit=200"),
        ]);
        drop(db);
        runtime.shutdown(); drop(owner); drop(runtime);
        let reopened = human_message_fixture_runtime(&root, &url);
        assert_eq!(reaction_recovery_calls_image(&reopened), retained_calls, "refused Calls effects remain absent after reopen");
        reopened.shutdown(); drop(reopened);
        fs::remove_dir_all(root).expect("remove strict Calls fixture");
    }
}

#[test]
fn shipping_human_history_coverage_incomplete_signal_is_exposed_until_sync_reaches_end() {
    let root = temp_root("human-history-coverage-incomplete-signal");
    let (url, server) = spawn_human_message_fixture(vec![
        human_history_reply(vec![human_history_message(6_401)]),
        human_history_reply(vec![human_history_message(6_401)]),
    ]);
    let runtime = human_message_fixture_runtime(&root, &url);
    let id = runtime
        .create_human_conversation("2", "Peer")
        .expect("conversation")["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let owner = runtime
        .open_human_conversation_db_owner(&id)
        .expect("owner");
    assert!(owner
        .append_transcript_entry(&human_local_history_message(6_400))
        .expect("seed 6400th row"));
    assert!(owner
        .set_metadata(
            "humanHistoryCoverage",
            json!({"version": 1, "afterId": 6_400, "complete": false}),
        )
        .expect("seed nonterminal coverage checkpoint"));

    let roster_incomplete = runtime
        .list_human_conversations()
        .expect("list incomplete roster");
    assert_eq!(roster_incomplete.len(), 1);
    assert_eq!(
        roster_incomplete[0].get("historyCoverageComplete"),
        Some(&json!(false)),
        "listHumanConversations must expose historyCoverageComplete: false when humanHistoryCoverage.complete is false"
    );
    let tail_incomplete = dispatch(
        &runtime,
        "getHumanConversationTranscriptTail",
        json!({"conversationId": id, "limit": 200}),
    );
    assert_eq!(
        tail_incomplete.get("historyCoverageComplete"),
        Some(&json!(false)),
        "getHumanConversationTranscriptTail must expose historyCoverageComplete: false when humanHistoryCoverage.complete is false"
    );

    let synced = runtime
        .sync_human_conversation(&id)
        .expect("resume remaining history window");
    assert_eq!(
        human_row_ids(&synced),
        vec![
            "human-server-message:6400".to_string(),
            "human-server-message:6401".to_string()
        ]
    );
    assert_eq!(
        owner
            .get_metadata("humanHistoryCoverage")
            .expect("completed coverage checkpoint"),
        Some(json!({"version": 1, "afterId": 6_401, "complete": true}))
    );

    let roster_complete = runtime
        .list_human_conversations()
        .expect("list completed roster");
    assert_eq!(roster_complete.len(), 1);
    assert_eq!(
        roster_complete[0].get("historyCoverageComplete"),
        None,
        "completed coverage omits historyCoverageComplete to preserve backwards-compatible summary shape"
    );
    let tail_complete = dispatch(
        &runtime,
        "getHumanConversationTranscriptTail",
        json!({"conversationId": id, "limit": 200}),
    );
    assert_eq!(
        tail_complete.get("historyCoverageComplete"),
        None,
        "completed coverage omits historyCoverageComplete on tail responses"
    );

    let requests = server.join().expect("coverage continuation fixture");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /api/social/messages?contactId=2&limit=200&afterId=6400 "));
    assert!(requests[1].starts_with("GET /api/social/messages?contactId=2&limit=200 "));
    runtime.shutdown();
    drop(owner);
    drop(runtime);
    let _ = fs::remove_dir_all(root);
}

fn create_test_miniapp_bundle(
    app_id: &str,
    version: &str,
) -> (Vec<u8>, mahayana_host_runtime::extensions::mcp::installable_package::CatalogAuthorization) {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};
    use std::io::{Cursor, Write};
    use zip::{ZipWriter, write::SimpleFileOptions};

    let key = SigningKey::from_bytes(&[7; 32]);
    let expected = b"<!doctype html><p>offline</p>";
    let mut hasher = Sha256::new();
    hasher.update(expected);
    let expected_hash = {
        let hash = hasher.finalize();
        let mut out = String::with_capacity(64);
        for byte in hash {
            use std::fmt::Write;
            let _ = write!(&mut out, "{byte:02x}");
        }
        out
    };

    let mut manifest = serde_json::json!({
        "manifestVersion": 1, "packageClass": "catalog", "appId": app_id,
        "publisherId": "test-publisher", "version": version, "displayName": "Test App",
        "entrypoint": "index.html", "runtime": "web", "minHostApi": 1, "offlineCore": true,
        "requestedCapabilities": [], "networkPolicy": { "mode": "deny", "allowedOrigins": [] },
        "files": { "index.html": { "sha256": expected_hash, "bytes": expected.len(), "mime": "text/html" } }
    });

    fn sorted(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let ordered: std::collections::BTreeMap<_, _> =
                    map.iter().map(|(k, v)| (k.clone(), sorted(v))).collect();
                serde_json::Value::Object(ordered.into_iter().collect())
            }
            serde_json::Value::Array(items) => serde_json::Value::Array(items.iter().map(sorted).collect()),
            _ => value.clone(),
        }
    }

    let domain = b"fabushi.fabapp.manifest.v1\0";
    let mut payload = domain.to_vec();
    payload.extend(serde_json::to_vec(&sorted(&manifest)).unwrap());
    manifest["signature"] = serde_json::json!({
        "algorithm": "Ed25519", "keyId": "fixture-only",
        "value": URL_SAFE_NO_PAD.encode(key.sign(&payload).to_bytes())
    });

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let items = vec![
        ("manifest.json".to_string(), serde_json::to_vec(&manifest).unwrap()),
        ("index.html".to_string(), expected.to_vec()),
    ];
    for (name, data) in items {
        writer
            .start_file(
                name,
                SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored)
                    .unix_permissions(0o644),
            )
            .unwrap();
        writer.write_all(&data).unwrap();
    }
    let bytes = writer.finish().unwrap().into_inner();

    let mut pkg_hasher = Sha256::new();
    pkg_hasher.update(&bytes);
    let pkg_hash = {
        let hash = pkg_hasher.finalize();
        let mut out = String::with_capacity(64);
        for byte in hash {
            use std::fmt::Write;
            let _ = write!(&mut out, "{byte:02x}");
        }
        out
    };

    let policy = mahayana_host_runtime::extensions::mcp::installable_package::CatalogAuthorization {
        app_id: app_id.into(),
        publisher_id: "test-publisher".into(),
        version: version.into(),
        package_sha256: pkg_hash,
        key_id: "fixture-only".into(),
        public_key: key.verifying_key().to_bytes(),
        trust_epoch: 1,
        host_api: 1,
        requested_capabilities: vec![],
    };
    (bytes, policy)
}

#[test]
fn miniapp_post_to_conversation_contract_enforces_host_authoritative_session_and_safety_fences() {
    let root = temp_root("miniapp-conversation-contract");
    let agents = root.join("agents");
    let memory = Arc::new(mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(agents.clone()));
    let runtime = Arc::new(ProductionSessionWorkers::with_agents_root_identity_and_dependencies(
        &agents,
        500,
        Some("human-local".into()),
        Arc::new(|| None),
        memory,
    ));

    let created = dispatch(
        &runtime,
        "createHumanConversation",
        json!({
            "localHumanId": "human-local",
            "peerHumanId": "human-peer",
            "title": "MiniApp Test Conversation"
        }),
    );
    let conversation_id = created["id"].as_str().expect("conversation id").to_string();

    // Set up ledger package verifier backed by existing installable_package owner
    let package_db = Arc::new(std::sync::Mutex::new(rusqlite::Connection::open_in_memory().unwrap()));
    {
        let db = package_db.lock().unwrap();
        mahayana_host_runtime::extensions::mcp::installable_package::ledger::initialize(&db).expect("ledger initialized");
    }

    // Dynamic scope provider: generation is NOT hardcoded!
    let current_gen = Arc::new(std::sync::atomic::AtomicU64::new(1));
    let gen_clone = Arc::clone(&current_gen);
    let scope_provider: Arc<dyn Fn() -> Result<mahayana_host_runtime::extensions::mcp::installable_package::ledger::HostInstallScope, String> + Send + Sync> = Arc::new(move || {
        Ok(mahayana_host_runtime::extensions::mcp::installable_package::ledger::HostInstallScope {
            account_scope: "human-local".into(),
            generation: gen_clone.load(std::sync::atomic::Ordering::SeqCst),
        })
    });
    runtime.set_mini_app_package_verifier(
        ProductionSessionWorkers::create_ledger_package_verifier(
            Arc::clone(&package_db),
            scope_provider,
        ),
    );

    // 1. issueMiniAppLaunchSession on uninstalled package fails closed (Point 3)
    let uninstalled = dispatch_production_session_gateway_call(
        &runtime,
        "issueMiniAppLaunchSession",
        &json!({
            "appId": "org.fabushi.shop",
            "conversationId": conversation_id,
        }),
    )
    .expect("handled call")
    .expect_err("uninstalled package must be rejected");
    assert!(matches!(uninstalled, SessionGatewayError::BadRequest(ref msg) if msg.contains("not installed")));

    // Install real verified package into installable_package ledger owner (Point 4)
    let (bundle_bytes, policy) = create_test_miniapp_bundle("org.fabushi.shop", "1.0.0");
    let verified_pkg = mahayana_host_runtime::extensions::mcp::installable_package::verify_catalog_bundle(&bundle_bytes, &policy).expect("verified package");
    let real_digest = verified_pkg.digest().to_string();
    {
        let mut db = package_db.lock().unwrap();
        let scope = mahayana_host_runtime::extensions::mcp::installable_package::ledger::HostInstallScope {
            account_scope: "human-local".into(),
            generation: 1,
        };
        mahayana_host_runtime::extensions::mcp::installable_package::ledger::activate(&mut db, &scope, &scope, "op-1", verified_pkg).expect("activated");
        mahayana_host_runtime::extensions::mcp::installable_package::ledger::verified_native_account(
            &db, "human-local", "binding-test", 9_999_999_999_999, 1000
        ).expect("verified native account initialized");
    }

    // 2. Digest mismatch fails closed
    let mismatch = dispatch_production_session_gateway_call(
        &runtime,
        "issueMiniAppLaunchSession",
        &json!({
            "appId": "org.fabushi.shop",
            "conversationId": conversation_id,
            "version": "1.0.0",
            "packageDigest": "0000000000000000000000000000000000000000000000000000000000000000",
        }),
    )
    .expect("handled call")
    .expect_err("digest mismatch must be rejected");
    assert!(matches!(mismatch, SessionGatewayError::BadRequest(ref msg) if msg.contains("digest mismatch")));

    // 3. Generation mismatch (account rotated to generation 2) fails closed (Point 2)
    current_gen.store(2, std::sync::atomic::Ordering::SeqCst);
    let gen_mismatch = dispatch_production_session_gateway_call(
        &runtime,
        "issueMiniAppLaunchSession",
        &json!({
            "appId": "org.fabushi.shop",
            "conversationId": conversation_id,
            "version": "1.0.0",
            "packageDigest": real_digest,
        }),
    )
    .expect("handled call")
    .expect_err("generation mismatch must fail closed");
    assert!(matches!(gen_mismatch, SessionGatewayError::BadRequest(_)));
    // Restore generation for remaining tests
    current_gen.store(1, std::sync::atomic::Ordering::SeqCst);

    // 4. issueMiniAppLaunchSession on non-participant conversation fails closed
    let non_participant = dispatch_production_session_gateway_call(
        &runtime,
        "issueMiniAppLaunchSession",
        &json!({
            "appId": "org.fabushi.shop",
            "conversationId": "foreign-conversation-id",
        }),
    )
    .expect("handled call")
    .expect_err("non-participant conversation must be rejected");
    assert!(matches!(non_participant, SessionGatewayError::BadRequest(_)));

    // 5. issueMiniAppLaunchSession succeeds for verified package + participant + matching digest
    // Notice: without prior user grant, effective permissions are EMPTY (Point 2: no self-granting!)
    let launch_res = dispatch(
        &runtime,
        "issueMiniAppLaunchSession",
        json!({
            "appId": "org.fabushi.shop",
            "conversationId": conversation_id,
            "startParameter": "promo_2026",
            "version": "1.0.0",
            "packageDigest": real_digest,
        }),
    );
    let launch_session_id = launch_res["launchSessionId"].as_str().expect("launchSessionId").to_string();
    assert_eq!(launch_res["appId"], "org.fabushi.shop");
    assert_eq!(launch_res["conversationId"], conversation_id);
    assert_eq!(launch_res["startParameter"], "promo_2026");

    // Calling SendMessage without explicit user grant fails with permission denied
    let nogrant_call = dispatch_production_session_gateway_call(
        &runtime,
        "miniAppPostToConversation",
        &json!({
            "launchSessionId": launch_session_id,
            "requestId": "req-0",
            "text": "Attempt without user grant",
        }),
    )
    .expect("handled call")
    .expect_err("request without user grant must fail closed");
    assert!(matches!(nogrant_call, SessionGatewayError::BadRequest(ref msg) if msg.contains("permission denied")));

    // Record explicit user grant for the app
    let local_human = runtime.local_human_id().expect("local human");
    runtime
        .set_miniapp_grant(fabushi_messaging_core::MiniAppGrant {
            mini_app_id: "org.fabushi.shop".into(),
            actor_id: fabushi_messaging_core::ActorId::new(local_human),
            permissions: vec![fabushi_messaging_core::MiniAppPermission::SendMessage],
            granted_at_ms: 1000,
            expires_at_ms: None,
        })
        .expect("set user grant");

    // Re-issue launch session with the user grant active
    let granted_launch = dispatch(
        &runtime,
        "issueMiniAppLaunchSession",
        json!({
            "appId": "org.fabushi.shop",
            "conversationId": conversation_id,
        }),
    );
    let granted_session_id = granted_launch["launchSessionId"].as_str().expect("session id").to_string();

    // 4. Point 1: MiniApp cannot self-confirm. SendMessage ALWAYS returns UserConfirmationRequired
    // Even if guest script passes userConfirmed: true
    let send_attempt = dispatch_production_session_gateway_call(
        &runtime,
        "miniAppPostToConversation",
        &json!({
            "launchSessionId": granted_session_id,
            "requestId": "req-1",
            "text": "Attacker trying userConfirmed: true",
            "userConfirmed": true
        }),
    )
    .expect("handled call")
    .expect_err("SendMessage without Host confirmation UI must fail closed");
    assert!(matches!(send_attempt, SessionGatewayError::BadRequest(ref msg) if msg.contains("user confirmation")));

    // Verify that NO message was written into the transcript!
    let transcript = runtime
        .read_human_conversation_transcript(&conversation_id)
        .expect("read transcript");
    assert!(
        transcript.is_empty(),
        "no message must be appended when SendMessage fails closed"
    );

    // 5. Point 2: SendData in Human direct conversation fails closed (no bot recipient)
    let send_data_denied = dispatch_production_session_gateway_call(
        &runtime,
        "miniAppPostToConversation",
        &json!({
            "launchSessionId": granted_session_id,
            "requestId": "req-data",
            "data": "{\"sku\": \"42\"}",
        }),
    )
    .expect("handled call")
    .expect_err("SendData in human direct conversation must fail closed");
    assert!(matches!(send_data_denied, SessionGatewayError::BadRequest(ref msg) if msg.contains("bot recipient")));

    // 6. Point 4: In-Memory Session Invalidation across restart
    // Clear in-memory launch sessions to simulate Host process restart
    runtime
        .clear_miniapp_launch_sessions_for_test()
        .expect("cleared memory sessions");

    let post_after_restart = dispatch_production_session_gateway_call(
        &runtime,
        "miniAppPostToConversation",
        &json!({
            "launchSessionId": granted_session_id,
            "requestId": "req-restart",
            "text": "Post after restart",
        }),
    )
    .expect("handled call")
    .expect_err("pre-restart launchSessionId must fail closed");
    assert!(matches!(post_after_restart, SessionGatewayError::BadRequest(ref msg) if msg.contains("not found")));

    runtime.shutdown();
    drop(runtime);
    let _ = fs::remove_dir_all(root);
}
