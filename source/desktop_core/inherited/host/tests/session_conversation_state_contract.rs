use std::collections::BTreeMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::agent_db_schema::AGENT_DB_SCHEMA;
use mahayana_host_runtime::extensions::session::agent_db_transcript_pages::{
    TranscriptPageQuery, TranscriptWindowQuery,
};
use mahayana_host_runtime::extensions::session::agent_db::compare_and_set_persisted_latest_root_blob_id;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::session::session_conversation_state::{
    ConversationOutlineItem, SessionConversationState,
};
use rusqlite::params;
use serde_json::json;
use sha2::{Digest, Sha256};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-session-conversation-state-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn seed(db_path: &std::path::Path) {
    let db = rusqlite::Connection::open(db_path).expect("db");
    db.execute_batch(AGENT_DB_SCHEMA).expect("schema");
    for (id, entry) in [
        ("root", json!({"id":"root","kind":"message","role":"user","content":"root","timestampMs":10})),
        ("branch-1", json!({"id":"branch-1","kind":"message","role":"assistant","content":"one","branched":true,"replyTo":"root","timestampMs":11})),
        ("branch-2", json!({"id":"branch-2","kind":"message","role":"assistant","content":"two","branched":true,"replyTo":"branch-1","timestampMs":12})),
        ("other-branch", json!({"id":"other-branch","kind":"message","role":"assistant","content":"other","branched":true,"replyTo":"other-root","timestampMs":13})),
        ("tool", json!({"id":"tool","kind":"tool-call","name":"search","timestampMs":14})),
        ("notice", json!({"id":"notice","kind":"notice","text":"notice","timestampMs":15})),
    ] {
        db.execute(
            "INSERT INTO transcript_entries (id, entry) VALUES (?1, ?2)",
            params![id, entry.to_string()],
        )
        .expect("entry");
    }
}

#[test]
fn transcript_read_owner_matches_frozen_page_window_tail_and_thread_semantics() {
    let root = temp_root("reads");
    let agent_dir = root.join("agent-a");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    seed(&db_path);
    let state = SessionConversationState::new(500);

    let all = state
        .read_agent_transcript_entries(&db_path)
        .expect("all entries");
    assert_eq!(all.len(), 6);

    let page = state
        .read_agent_transcript_page(
            &db_path,
            TranscriptPageQuery {
                before_seq: None,
                since_ms: None,
                until_ms: 100,
                limit: 5,
            },
        )
        .expect("page");
    assert_eq!(
        page.entries
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["root"]
    );

    let window = state
        .read_agent_transcript_window(
            &db_path,
            TranscriptWindowQuery {
                before_seq: None,
                limit: 10,
            },
        )
        .expect("window");
    assert_eq!(
        window
            .entries
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["root", "notice"]
    );
    assert_eq!(
        window.thread_counts,
        BTreeMap::from([("root".to_string(), 2usize)])
    );

    let tail = state
        .read_agent_transcript_tail(
            &db_path,
            TranscriptWindowQuery {
                before_seq: None,
                limit: 10,
            },
        )
        .expect("tail");
    assert_eq!(tail.entries.len(), 6);

    let thread = state
        .read_agent_thread(&db_path, "root")
        .expect("thread");
    assert_eq!(
        thread
            .entries
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["root", "branch-1", "branch-2"]
    );
    assert!(
        state
            .read_agent_thread(&db_path, "missing")
            .expect("missing thread")
            .entries
            .is_empty()
    );

    let _ = fs::remove_dir_all(root);
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

fn blob_id(data: &[u8]) -> Vec<u8> {
    Sha256::digest(data).to_vec()
}

#[test]
fn durable_outline_resolves_frozen_user_steps_send_message_task_and_shell_semantics() {
    let root = temp_root("outline");
    let agents = root.join("agents");
    let runtime = std::sync::Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let record = runtime.materialize_new_session(None, "user", None).expect("agent");
    let store = runtime.create_agent_blob_store(&record.id).expect("blob store");

    let mut user = Vec::new();
    push_string(1, "[SAND_HIDDEN_PROMPT][SAND_TRUSTED_AUTOMATION_PROMPT]secret", &mut user);
    push_string(2, "message-1", &mut user);
    let user_id = blob_id(&user);
    futures::executor::block_on(store.set_blob(&(), &user_id, &user)).expect("user blob");

    let mut assistant = Vec::new();
    push_string(1, "assistant text", &mut assistant);
    let mut assistant_step = Vec::new();
    push_bytes(1, &assistant, &mut assistant_step);
    let assistant_step_id = blob_id(&assistant_step);
    futures::executor::block_on(store.set_blob(&(), &assistant_step_id, &assistant_step))
        .expect("assistant step");

    let mut thinking = Vec::new();
    push_string(1, "thinking", &mut thinking);
    push_varint(2, 17, &mut thinking);
    let mut thinking_step = Vec::new();
    push_bytes(3, &thinking, &mut thinking_step);
    let thinking_step_id = blob_id(&thinking_step);
    futures::executor::block_on(store.set_blob(&(), &thinking_step_id, &thinking_step))
        .expect("thinking step");

    let mut send_text = Vec::new();
    push_string(1, "sent text", &mut send_text);
    let mut send_args = Vec::new();
    push_bytes(1, &send_text, &mut send_args);
    let mut send_call = Vec::new();
    push_bytes(1, &send_args, &mut send_call);
    let mut send_tool = Vec::new();
    push_bytes(55, &send_call, &mut send_tool);
    let mut send_step = Vec::new();
    push_bytes(2, &send_tool, &mut send_step);
    let send_step_id = blob_id(&send_step);
    futures::executor::block_on(store.set_blob(&(), &send_step_id, &send_step))
        .expect("send-message step");

    let mut task_args = Vec::new();
    push_string(1, "  investigate  ", &mut task_args);
    let mut task_error = Vec::new();
    push_string(1, "task failed", &mut task_error);
    let mut task_result = Vec::new();
    push_bytes(2, &task_error, &mut task_result);
    let mut task_call = Vec::new();
    push_bytes(1, &task_args, &mut task_call);
    push_bytes(2, &task_result, &mut task_call);
    let mut task_tool = Vec::new();
    push_bytes(19, &task_call, &mut task_tool);
    let mut task_step = Vec::new();
    push_bytes(2, &task_tool, &mut task_step);
    let task_step_id = blob_id(&task_step);
    futures::executor::block_on(store.set_blob(&(), &task_step_id, &task_step))
        .expect("task step");

    let mut agent_turn = Vec::new();
    push_bytes(1, &user_id, &mut agent_turn);
    for step in [&assistant_step_id, &thinking_step_id, &send_step_id, &task_step_id] {
        push_bytes(2, step, &mut agent_turn);
    }
    let mut agent_turn_wrapper = Vec::new();
    push_bytes(1, &agent_turn, &mut agent_turn_wrapper);
    let agent_turn_id = blob_id(&agent_turn_wrapper);
    futures::executor::block_on(store.set_blob(&(), &agent_turn_id, &agent_turn_wrapper))
        .expect("agent turn");

    let mut shell_command = Vec::new();
    push_string(1, "pwd", &mut shell_command);
    let shell_command_id = blob_id(&shell_command);
    futures::executor::block_on(store.set_blob(&(), &shell_command_id, &shell_command))
        .expect("shell command");
    let mut shell_output = Vec::new();
    push_string(1, "/tmp", &mut shell_output);
    let shell_output_id = blob_id(&shell_output);
    futures::executor::block_on(store.set_blob(&(), &shell_output_id, &shell_output))
        .expect("shell output");
    let mut shell_turn = Vec::new();
    push_bytes(1, &shell_command_id, &mut shell_turn);
    push_bytes(2, &shell_output_id, &mut shell_turn);
    let mut shell_turn_wrapper = Vec::new();
    push_bytes(2, &shell_turn, &mut shell_turn_wrapper);
    let shell_turn_id = blob_id(&shell_turn_wrapper);
    futures::executor::block_on(store.set_blob(&(), &shell_turn_id, &shell_turn_wrapper))
        .expect("shell turn");

    let mut todo = Vec::new();
    push_string(1, "todo-1", &mut todo);
    push_string(2, "finish parity", &mut todo);
    push_varint(3, 2, &mut todo);
    push_varint(4, 100, &mut todo);
    push_varint(5, 200, &mut todo);
    push_string(6, "dependency-a", &mut todo);
    let todo_id = blob_id(&todo);
    futures::executor::block_on(store.set_blob(&(), &todo_id, &todo)).expect("todo");

    let mut summary = Vec::new();
    push_string(1, "conversation summary", &mut summary);
    let summary_id = blob_id(&summary);
    futures::executor::block_on(store.set_blob(&(), &summary_id, &summary)).expect("summary");

    let mut root_blob = Vec::new();
    push_bytes(3, &todo_id, &mut root_blob);
    push_bytes(6, &summary_id, &mut root_blob);
    push_bytes(8, &agent_turn_id, &mut root_blob);
    push_bytes(8, &shell_turn_id, &mut root_blob);
    let root_id = blob_id(&root_blob);
    futures::executor::block_on(store.set_blob(&(), &root_id, &root_blob)).expect("root blob");
    assert!(compare_and_set_persisted_latest_root_blob_id(
        &record.db_path,
        500,
        &[],
        &root_id,
    ).expect("root CAS"));

    let resolved = runtime
        .read_agent_conversation_state(&record.id)
        .expect("resolved state")
        .expect("materialized state");
    assert_eq!(resolved.turns.len(), 2);
    assert_eq!(resolved.todos.len(), 1);
    assert_eq!(resolved.todos[0].id, "todo-1");
    assert_eq!(resolved.todos[0].content, "finish parity");
    assert_eq!(resolved.todos[0].status, 2);
    assert_eq!(resolved.todos[0].created_at, 100);
    assert_eq!(resolved.todos[0].updated_at, 200);
    assert_eq!(resolved.todos[0].dependencies, vec!["dependency-a".to_string()]);
    assert_eq!(resolved.summary.as_deref(), Some("conversation summary"));

    let outline = runtime.read_agent_outline(&record.id).expect("outline");
    assert_eq!(outline.len(), 6);
    assert!(matches!(
        &outline[0],
        ConversationOutlineItem::User { id, text, hidden }
            if id == "outline-user-0" && text == "secret" && *hidden
    ));
    assert!(matches!(
        &outline[1],
        ConversationOutlineItem::AssistantText { id, text }
            if id == "outline-0-0" && text == "assistant text"
    ));
    assert!(matches!(
        &outline[2],
        ConversationOutlineItem::Thinking { id, text, duration_ms }
            if id == "outline-0-1" && text == "thinking" && *duration_ms == Some(17)
    ));
    assert!(matches!(
        &outline[3],
        ConversationOutlineItem::SendMessage { id, message }
            if id == "outline-0-2"
                && message == &json!({"type":"text","content":"sent text"})
    ));
    assert!(matches!(
        &outline[4],
        ConversationOutlineItem::ToolCall { id, name, status, summary }
            if id == "outline-0-3"
                && name == "Task"
                && status == "failed"
                && summary.as_deref() == Some("task failed")
    ));
    assert!(matches!(
        &outline[5],
        ConversationOutlineItem::ToolCall { id, name, status, summary }
            if id == "outline-shell-1"
                && name == "shellToolCall"
                && status == "done"
                && summary.as_deref() == Some("pwd")
    ));

    let prepared = runtime
        .prepare_existing_agent(&record.id)
        .expect("prepare existing agent")
        .expect("prepared agent");
    assert_eq!(
        prepared
            .transcript_tail
            .entries
            .iter()
            .map(|entry| entry.get("kind").and_then(serde_json::Value::as_str).unwrap_or(""))
            .collect::<Vec<_>>(),
        vec!["send-message", "tool-call", "tool-call"]
    );

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}
