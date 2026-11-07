use mahayana_host_runtime::extensions::session::session_projection::{
    build_attachment_last_entry, collect_last_attachment_batch_kinds,
    get_last_entry_from_transcript, get_last_message_from_transcript,
    get_summary_updated_at, get_updated_at_from_stats, is_https_url,
    last_message_preview_text, seed_activity_from_mtime_value,
};
use serde_json::json;

#[test]
fn time_projection_and_activity_seed_match_frozen_contract() {
    assert_eq!(get_updated_at_from_stats(10.0, Some(12.9)), 12.0);
    assert_eq!(get_updated_at_from_stats(20.0, Some(12.9)), 20.0);
    assert_eq!(get_summary_updated_at(10.0, 30.0), 30.0);
    assert_eq!(
        seed_activity_from_mtime_value(0.0, true, 10.0, Some(12.9)),
        Some(12.0)
    );
    assert_eq!(seed_activity_from_mtime_value(1.0, true, 10.0, Some(12.9)), None);
    assert_eq!(seed_activity_from_mtime_value(0.0, false, 10.0, Some(12.9)), None);
}

#[test]
fn attachment_projection_preserves_link_and_batch_semantics() {
    assert!(is_https_url("https://example.com/a"));
    assert!(!is_https_url("http://example.com/a"));
    let entries = vec![
        json!({"kind":"user-attachment","batchId":"b","file_name":"a.png","file_path":"/a.png"}),
        json!({"kind":"user-attachment","batchId":"b","file_name":"b.pdf","file_path":"/b.pdf"}),
    ];
    assert_eq!(
        collect_last_attachment_batch_kinds(&entries, 1),
        vec!["image".to_string(), "document".to_string()]
    );
    assert_eq!(
        build_attachment_last_entry(&entries, 1),
        json!({"kind":"attachment","count":2,"kinds":{"document":1,"image":1}})
    );
    assert_eq!(
        last_message_preview_text(&json!({"type":"attachment","url":"https://example.com","file_name":null})),
        "Sent a link · https://example.com"
    );
}

#[test]
fn last_message_and_last_entry_follow_frozen_visibility_rules() {
    let entries = vec![
        json!({"id":"old","kind":"send-message","message":{"type":"text","content":"**Hello**   world"}}),
        json!({"id":"branch","kind":"message","content":"branch","branched":true}),
        json!({"id":"hidden","kind":"message","content":"hidden","hidden":true}),
        json!({"id":"peer","kind":"message","content":"peer","peerAgentId":"agent-b"}),
        json!({"id":"latest","kind":"message","content":"visible"}),
    ];
    let last_message = get_last_message_from_transcript(&entries).expect("last message");
    assert_eq!(last_message.id, "old");
    assert_eq!(last_message.preview, "Hello world");
    assert_eq!(
        get_last_entry_from_transcript(&entries),
        Some(json!({"kind":"text","text":"visible"}))
    );
}
