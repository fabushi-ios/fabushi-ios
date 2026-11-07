use mahayana_node_agent_coordinator::inference_router::{
    INFERENCE_TRANSCRIPT_SCHEMA_VERSION, StoredEntry, StoredRole, TranscriptStore,
    parse_send_prompt_attachments, project_transcript_entry,
};
use serde_json::json;

#[test]
fn local_inference_attachment_contract_persists_and_projects_user_attachments() {
    let args = json!({
        "attachmentPaths": ["/tmp/agent-notes.txt", "/tmp/context.md"],
        "attachmentNames": ["agent-notes.txt", "context.md"]
    });
    let attachments = parse_send_prompt_attachments(&args).expect("parse attachments");
    assert_eq!(attachments.len(), 2);
    assert_eq!(attachments[0].path, "/tmp/agent-notes.txt");
    assert_eq!(attachments[0].name, "agent-notes.txt");

    let entry = StoredEntry {
        provider: "codex".into(),
        role: StoredRole::User,
        content: String::new(),
        rich_text: None,
        id: "t0u".into(),
        client_nonce: Some("nonce-attachment".into()),
        attachments,
        reactions: Vec::new(),
        timestamp_ms: 42,
    };
    let projected = project_transcript_entry(&entry);
    assert_eq!(projected["clientNonce"], "nonce-attachment");
    assert_eq!(projected["attachments"][0]["path"], "/tmp/agent-notes.txt");
    assert_eq!(projected["attachments"][0]["name"], "agent-notes.txt");

    let persisted = json!({
        "schemaVersion": INFERENCE_TRANSCRIPT_SCHEMA_VERSION,
        "agents": {
            "agent-1": [serde_json::to_value(&entry).expect("serialize stored entry")]
        }
    });
    let restored = TranscriptStore::parse(persisted);
    assert_eq!(restored.entries("agent-1"), &[entry]);
}

#[test]
fn local_inference_attachment_contract_rejects_unpaired_arrays() {
    let error = parse_send_prompt_attachments(&json!({
        "attachmentPaths": ["/tmp/agent-notes.txt"],
        "attachmentNames": []
    }))
    .expect_err("mismatched attachment arrays must fail");
    assert_eq!(error.code, "INFERENCE_ROUTER_INVALID_ATTACHMENTS");
}
