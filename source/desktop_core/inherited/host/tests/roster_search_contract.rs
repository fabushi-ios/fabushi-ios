use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::agents::agent_profile::SandAgentProfile;
use mahayana_host_runtime::extensions::content_search::search_index_db::{
    AttachmentKind, MediaSearchResult, MessageSearchResult,
};
use mahayana_host_runtime::extensions::session::gateway::{
    dispatch_production_session_gateway_call,
    dispatch_production_session_gateway_call_with_content_search,
};
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::roster_search::{
    AGENT_CONTENT_SEARCH_MAX_MATCHES_PER_AGENT, RosterContentSearch, build_content_snippet,
    find_agent_content_matches, search_agents_linear,
};
use serde_json::json;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-roster-search-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn profile(name: &str) -> SandAgentProfile {
    SandAgentProfile {
        name: name.into(),
        description: String::new(),
        title: String::new(),
        avatar_shape: String::new(),
        avatar_color: String::new(),
    }
}

#[test]
fn frozen_content_match_rules_preserve_snippets_order_and_hidden_peer_filter() {
    let entries = vec![
        json!({"id":"one","kind":"message","role":"user","content":"before needle after","timestampMs":10}),
        json!({"id":"two","kind":"notice","text":"NEEDLE second","timestampMs":20}),
        json!({"id":"hidden","kind":"message","role":"assistant","content":"needle hidden","timestampMs":30,"hiddenOutboundAgentPeerMessage":true}),
        json!({"id":"three","kind":"send-message","message":{"type":"text","content":"needle third"},"timestampMs":40}),
    ];
    let matches = find_agent_content_matches(
        &entries,
        "needle",
        AGENT_CONTENT_SEARCH_MAX_MATCHES_PER_AGENT,
    );
    assert_eq!(
        matches.iter().map(|entry| entry.entry_id.as_str()).collect::<Vec<_>>(),
        vec!["three", "two", "one"]
    );
    assert_eq!(matches[0].role, "assistant");
    assert_eq!(matches[2].role, "user");
    assert_eq!(
        build_content_snippet("alpha   needle\n beta", "needle").as_deref(),
        Some("alpha needle beta")
    );

    let rich_entries = vec![
        json!({
            "id": "rich-msg",
            "kind": "message",
            "role": "user",
            "authorName": "ReleaseLead",
            "forwardedFromAuthorName": "SecurityReviewer",
            "content": "Ship checklist",
            "attachments": [{"name": "release-signoff.pdf"}],
            "timestampMs": 50
        }),
        json!({
            "id": "rich-widget",
            "kind": "send-message",
            "message": {"type": "widget", "widget": {"prompt": "Confirm production cutover"}},
            "timestampMs": 60
        }),
        json!({
            "id": "rich-email",
            "kind": "send-message",
            "message": {"type": "email-draft", "draft": {"subject": "Incident Postmortem", "body": "Root cause analysis"}},
            "timestampMs": 70
        }),
        json!({
            "id": "rich-slack",
            "kind": "send-message",
            "message": {"type": "slack-draft", "draft": {"body": "Deploy window open"}},
            "timestampMs": 80
        }),
    ];
    let attachment_matches = find_agent_content_matches(&rich_entries, "securityreviewer release-signoff.pdf", 5);
    assert_eq!(attachment_matches.len(), 1);
    assert_eq!(attachment_matches[0].entry_id, "rich-msg");
    assert_eq!(attachment_matches[0].role, "user");

    let widget_matches = find_agent_content_matches(&rich_entries, "production cutover", 5);
    assert_eq!(widget_matches.len(), 1);
    assert_eq!(widget_matches[0].entry_id, "rich-widget");

    let email_matches = find_agent_content_matches(&rich_entries, "postmortem root cause", 5);
    assert_eq!(email_matches.len(), 1);
    assert_eq!(email_matches[0].entry_id, "rich-email");

    let slack_matches = find_agent_content_matches(&rich_entries, "deploy window", 5);
    assert_eq!(slack_matches.len(), 1);
    assert_eq!(slack_matches[0].entry_id, "rich-slack");
}

#[test]
fn production_search_agents_scans_real_session_transcripts_and_gateway_dispatches_it() {
    let root = temp_root("production");
    let session = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let first = session
        .materialize_new_session(Some(&profile("First")), "user", None)
        .expect("first agent");
    let second = session
        .materialize_new_session(Some(&profile("Second")), "user", None)
        .expect("second agent");

    session
        .append_agent_transcript_entries(
            &first.id,
            &[
                json!({"id":"first-old","kind":"message","role":"user","content":"needle old","timestampMs":100}),
                json!({"id":"first-new","kind":"message","role":"assistant","content":"needle newest","timestampMs":300}),
            ],
        )
        .expect("first transcript");
    session
        .append_agent_transcript_entries(
            &second.id,
            &[json!({"id":"second","kind":"notice","text":"needle middle","timestampMs":200})],
        )
        .expect("second transcript");

    let results = search_agents_linear(&session, "  NeEdLe  ", 10).expect("search");
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].entry_id, "first-new");
    assert_eq!(results[1].entry_id, "second");
    assert_eq!(results[2].entry_id, "first-old");

    let gateway = dispatch_production_session_gateway_call(
        &session,
        "searchAgents",
        &json!({"query":"needle","limit":2}),
    )
    .expect("gateway method")
    .expect("gateway search");
    assert_eq!(gateway.as_array().map(Vec::len), Some(2));
    assert_eq!(gateway[0]["entryId"], "first-new");

    session.shutdown();
    let _ = fs::remove_dir_all(root);
}


struct FakeIndexedRosterSearch {
    agent_id: String,
}

impl RosterContentSearch for FakeIndexedRosterSearch {
    fn is_search_ready(&self) -> bool { true }
    fn max_matches_per_agent(&self) -> usize { 5 }
    fn max_results(&self) -> usize { 50 }

    fn search_messages(&self, query: &str, _limit: usize) -> Option<Vec<MessageSearchResult>> {
        assert_eq!(query, "needle");
        Some(vec![
            MessageSearchResult {
                agent_id: self.agent_id.clone(),
                entry_id: "indexed-message".into(),
                role: "assistant".into(),
                timestamp_ms: 900,
                snippet: "indexed needle result".into(),
            },
            MessageSearchResult {
                agent_id: "deleted-agent".into(),
                entry_id: "stale-message".into(),
                role: "assistant".into(),
                timestamp_ms: 1000,
                snippet: "must be filtered".into(),
            },
        ])
    }

    fn search_media(&self, query: &str, _limit: usize) -> Option<Vec<MediaSearchResult>> {
        assert_eq!(query, "needle");
        Some(vec![
            MediaSearchResult {
                agent_id: self.agent_id.clone(),
                entry_id: "indexed-media".into(),
                file_name: "needle.png".into(),
                ext: "png".into(),
                mime: Some("image/png".into()),
                kind: AttachmentKind::Image,
                timestamp_ms: 901,
                width: Some(640),
                height: Some(480),
            },
            MediaSearchResult {
                agent_id: "deleted-agent".into(),
                entry_id: "stale-media".into(),
                file_name: "stale.png".into(),
                ext: "png".into(),
                mime: Some("image/png".into()),
                kind: AttachmentKind::Image,
                timestamp_ms: 1001,
                width: None,
                height: None,
            },
        ])
    }
}

#[test]
fn production_gateway_consumes_ready_content_search_for_indexed_messages_and_media() {
    let root = temp_root("indexed-gateway");
    let session = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let agent = session
        .materialize_new_session(Some(&profile("Indexed")), "user", None)
        .expect("indexed agent");
    let indexed = FakeIndexedRosterSearch {
        agent_id: agent.id.clone(),
    };

    let messages = dispatch_production_session_gateway_call_with_content_search(
        &session,
        Some(&indexed),
        "searchAgents",
        &json!({"query":" needle ","limit":10}),
    )
    .expect("searchAgents method")
    .expect("indexed searchAgents");
    assert_eq!(messages.as_array().map(Vec::len), Some(1));
    assert_eq!(messages[0]["agentId"], agent.id);
    assert_eq!(messages[0]["entryId"], "indexed-message");
    assert_eq!(messages[0]["snippet"], "indexed needle result");

    let media = dispatch_production_session_gateway_call_with_content_search(
        &session,
        Some(&indexed),
        "searchMedia",
        &json!({"query":"needle","limit":10}),
    )
    .expect("searchMedia method")
    .expect("indexed searchMedia");
    assert_eq!(media.as_array().map(Vec::len), Some(1));
    assert_eq!(media[0]["agentId"], agent.id);
    assert_eq!(media[0]["entryId"], "indexed-media");
    assert_eq!(media[0]["fileName"], "needle.png");
    assert_eq!(media[0]["kind"], "image");
    assert_eq!(media[0]["width"], 640);
    assert_eq!(media[0]["height"], 480);

    session.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn canonical_search_merges_authenticated_human_and_agent_conversations_and_enforces_account_isolation() {
    let root = temp_root("canonical-human-agent-search");
    let memory = Arc::new(
        mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(root.clone()),
    );
    let session = Arc::new(
        ProductionSessionWorkers::with_agents_root_identity_and_dependencies(
            &root,
            500,
            Some("human-local".into()),
            Arc::new(|| None),
            memory,
        ),
    );
    let agent = session
        .materialize_new_session(Some(&profile("IndexedAgent")), "user", None)
        .expect("indexed agent");
    let human_conversation = session
        .create_human_conversation("human-peer", "Alice")
        .expect("human conversation");
    let conversation_id = human_conversation["id"]
        .as_str()
        .expect("conversation id")
        .to_string();

    let human_first = session
        .append_human_message(
            &conversation_id,
            "needle from local human",
            "nonce-search-1",
            Some(950.0),
            None,
            &[json!({"path": "/tmp/needle-report.pdf", "name": "needle-report.pdf"})],
        )
        .expect("first human message");
    let human_agent_handoff = session
        .append_human_agent_message(
            &conversation_id,
            &agent.id,
            "stream-search-1",
            None,
            "needle from handoff agent",
            980.0,
        )
        .expect("human agent handoff message");

    let indexed = FakeIndexedRosterSearch {
        agent_id: agent.id.clone(),
    };

    let unified_messages = dispatch_production_session_gateway_call_with_content_search(
        &session,
        Some(&indexed),
        "searchAgents",
        &json!({"query":" needle ","limit":10}),
    )
    .expect("searchAgents method")
    .expect("unified searchAgents");
    let unified_array = unified_messages.as_array().expect("searchAgents array");
    assert_eq!(unified_array.len(), 3);
    assert_eq!(unified_array[0]["agentId"], conversation_id);
    assert_eq!(unified_array[0]["entryId"], human_agent_handoff["id"]);
    assert_eq!(unified_array[0]["role"], "assistant");
    assert_eq!(unified_array[1]["agentId"], conversation_id);
    assert_eq!(unified_array[1]["entryId"], human_first["id"]);
    assert_eq!(unified_array[1]["role"], "user");
    assert_eq!(unified_array[2]["agentId"], agent.id);
    assert_eq!(unified_array[2]["entryId"], "indexed-message");

    let linear_messages = dispatch_production_session_gateway_call(
        &session,
        "searchAgents",
        &json!({"query":"needle-report.pdf","limit":10}),
    )
    .expect("searchAgents linear method")
    .expect("linear searchAgents");
    let linear_array = linear_messages.as_array().expect("linear array");
    assert_eq!(linear_array.len(), 1);
    assert_eq!(linear_array[0]["agentId"], conversation_id);
    assert_eq!(linear_array[0]["entryId"], human_first["id"]);

    let unified_media = dispatch_production_session_gateway_call_with_content_search(
        &session,
        Some(&indexed),
        "searchMedia",
        &json!({"query":"needle","limit":10}),
    )
    .expect("searchMedia method")
    .expect("unified searchMedia");
    let media_array = unified_media.as_array().expect("searchMedia array");
    assert_eq!(media_array.len(), 2);
    assert_eq!(media_array[0]["agentId"], conversation_id);
    assert_eq!(media_array[0]["entryId"], human_first["id"]);
    assert_eq!(media_array[0]["fileName"], "needle-report.pdf");
    assert_eq!(media_array[0]["ext"], ".pdf");
    assert_eq!(media_array[0]["kind"], "pdf");
    assert_eq!(media_array[1]["agentId"], agent.id);
    assert_eq!(media_array[1]["entryId"], "indexed-media");

    let scoped_human_messages = dispatch_production_session_gateway_call_with_content_search(
        &session,
        Some(&indexed),
        "searchHumanMessages",
        &json!({
            "conversationId": conversation_id,
            "query": "needle",
            "limit": 10
        }),
    )
    .expect("searchHumanMessages method")
    .expect("scoped searchHumanMessages");
    assert_eq!(
        scoped_human_messages,
        json!([human_agent_handoff, human_first])
    );

    let multi_term_scoped = dispatch_production_session_gateway_call(
        &session,
        "searchHumanMessages",
        &json!({
            "conversationId": conversation_id,
            "query": "local needle-report.pdf",
            "limit": 10
        }),
    )
    .expect("multi-term searchHumanMessages method")
    .expect("multi-term searchHumanMessages");
    assert_eq!(multi_term_scoped, json!([human_first]));

    let multi_term_media = dispatch_production_session_gateway_call(
        &session,
        "searchMedia",
        &json!({"query": "local pdf", "limit": 10}),
    )
    .expect("multi-term searchMedia method")
    .expect("multi-term searchMedia");
    let multi_term_media_array = multi_term_media.as_array().expect("multi-term media array");
    assert_eq!(multi_term_media_array.len(), 1);
    assert_eq!(multi_term_media_array[0]["agentId"], conversation_id);
    assert_eq!(multi_term_media_array[0]["entryId"], human_first["id"]);

    let origin_message = session
        .append_human_agent_message(
            &conversation_id,
            "OriginArchitect",
            "stream-origin-1",
            None,
            "quarterly architecture roadmap",
            990.0,
        )
        .expect("origin handoff message");
    assert_eq!(origin_message["authorName"], json!("OriginArchitect"));
    let bob_conversation = session
        .create_human_conversation("human-bob", "Bob")
        .expect("bob conversation");
    let bob_id = bob_conversation["id"]
        .as_str()
        .expect("bob conversation id")
        .to_string();
    let carol_conversation = session
        .create_human_conversation("human-carol", "Carol")
        .expect("carol conversation");
    let carol_id = carol_conversation["id"]
        .as_str()
        .expect("carol conversation id")
        .to_string();

    let bob_forward = session
        .forward_human_message(
            &conversation_id,
            origin_message["id"].as_str().expect("origin message id"),
            std::slice::from_ref(&bob_id),
            "fwd-keep-origin",
            false,
            false,
        )
        .expect("forward preserving origin author");
    assert_eq!(bob_forward["destinations"][0]["status"], json!("sent"));
    let carol_forward = session
        .forward_human_message(
            &conversation_id,
            origin_message["id"].as_str().expect("origin message id"),
            std::slice::from_ref(&carol_id),
            "fwd-drop-origin",
            true,
            false,
        )
        .expect("forward dropping origin author");
    assert_eq!(carol_forward["destinations"][0]["status"], json!("sent"));

    let bob_forward_matches = dispatch_production_session_gateway_call(
        &session,
        "searchHumanMessages",
        &json!({
            "conversationId": bob_id,
            "query": "OriginArchitect roadmap",
            "limit": 10
        }),
    )
    .expect("bob searchHumanMessages method")
    .expect("bob searchHumanMessages");
    assert_eq!(bob_forward_matches.as_array().map(Vec::len), Some(1));

    let carol_hidden_origin_matches = dispatch_production_session_gateway_call(
        &session,
        "searchHumanMessages",
        &json!({
            "conversationId": carol_id,
            "query": "OriginArchitect",
            "limit": 10
        }),
    )
    .expect("carol origin searchHumanMessages method")
    .expect("carol origin searchHumanMessages");
    assert_eq!(carol_hidden_origin_matches, json!([]));

    let carol_content_matches = dispatch_production_session_gateway_call(
        &session,
        "searchHumanMessages",
        &json!({
            "conversationId": carol_id,
            "query": "architecture roadmap",
            "limit": 10
        }),
    )
    .expect("carol content searchHumanMessages method")
    .expect("carol content searchHumanMessages");
    assert_eq!(carol_content_matches.as_array().map(Vec::len), Some(1));

    let cross_conversation_origin_search = dispatch_production_session_gateway_call(
        &session,
        "searchAgents",
        &json!({"query": "OriginArchitect roadmap", "limit": 10}),
    )
    .expect("origin searchAgents method")
    .expect("origin searchAgents");
    let origin_search_array = cross_conversation_origin_search
        .as_array()
        .expect("origin searchAgents array");
    assert_eq!(origin_search_array.len(), 2);
    assert!(origin_search_array
        .iter()
        .any(|item| item["agentId"] == bob_id));
    assert!(origin_search_array
        .iter()
        .any(|item| item["agentId"] == conversation_id));
    assert!(!origin_search_array
        .iter()
        .any(|item| item["agentId"] == carol_id));

    let dave_conversation = session
        .create_human_conversation("human-dave", "Dave")
        .expect("dave conversation");
    let dave_id = dave_conversation["id"]
        .as_str()
        .expect("dave conversation id")
        .to_string();
    let bob_forwarded_entry_id = bob_forward_matches
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item["id"].as_str())
        .expect("bob forwarded entry id");
    let dave_chained_forward = session
        .forward_human_message(
            &bob_id,
            bob_forwarded_entry_id,
            std::slice::from_ref(&dave_id),
            "fwd-chained-origin",
            false,
            false,
        )
        .expect("chained forward preserving original origin author");
    assert_eq!(dave_chained_forward["destinations"][0]["status"], json!("sent"));

    let carol_forwarded_entry_id = carol_content_matches
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item["id"].as_str())
        .expect("carol anonymously forwarded entry id");
    let dave_anon_reforward = session
        .forward_human_message(
            &carol_id,
            carol_forwarded_entry_id,
            std::slice::from_ref(&dave_id),
            "fwd-chained-anon",
            false,
            false,
        )
        .expect("re-forward of anonymous forward must not leak hidden origin");
    assert_eq!(dave_anon_reforward["destinations"][0]["status"], json!("sent"));

    let dave_origin_matches = dispatch_production_session_gateway_call(
        &session,
        "searchHumanMessages",
        &json!({
            "conversationId": dave_id,
            "query": "OriginArchitect roadmap",
            "limit": 10
        }),
    )
    .expect("dave searchHumanMessages method")
    .expect("dave searchHumanMessages");
    let dave_origin_array = dave_origin_matches.as_array().expect("dave origin array");
    assert_eq!(dave_origin_array.len(), 1);
    assert_eq!(
        dave_origin_array[0].pointer("/forwardContext/origin/authorName"),
        Some(&json!("OriginArchitect")),
    );

    let dave_transcript = session
        .read_human_conversation_transcript(&dave_id)
        .expect("dave transcript");
    assert_eq!(dave_transcript.len(), 2);
    assert_eq!(
        dave_transcript[1].pointer("/forwardContext/origin/authorId"),
        Some(&json!("human-local")),
    );
    assert_eq!(
        dave_transcript[1].pointer("/forwardContext/origin/authorName"),
        None,
    );

    let bob_owner = session
        .open_human_conversation_db_owner(&bob_id)
        .expect("bob owner");
    assert!(bob_owner
        .append_transcript_entry(&json!({
            "id": "human-server-message:801",
            "kind": "message",
            "role": "user",
            "authorKind": "human",
            "authorId": "human-bob",
            "content": "peer fallback note",
            "timestampMs": 1010.0,
            "delivery": "sent",
            "remoteMessageId": "801",
            "remoteCreatedAt": "1970-01-01T00:00:01Z"
        }))
        .expect("seed bob peer row without authorName"));
    let peer_fallback_forward = session
        .forward_human_message(
            &bob_id,
            "human-server-message:801",
            std::slice::from_ref(&carol_id),
            "fwd-peer-title-fallback",
            false,
            false,
        )
        .expect("forward peer row with conversation title fallback");
    assert_eq!(
        peer_fallback_forward["destinations"][0]["status"],
        json!("sent")
    );
    let carol_peer_fallback_matches = dispatch_production_session_gateway_call(
        &session,
        "searchHumanMessages",
        &json!({
            "conversationId": carol_id,
            "query": "Bob fallback note",
            "limit": 10
        }),
    )
    .expect("carol peer fallback searchHumanMessages method")
    .expect("carol peer fallback searchHumanMessages");
    let carol_peer_fallback_array = carol_peer_fallback_matches
        .as_array()
        .expect("carol peer fallback array");
    assert_eq!(carol_peer_fallback_array.len(), 1);
    assert_eq!(
        carol_peer_fallback_array[0].pointer("/forwardContext/origin/authorId"),
        Some(&json!("human-bob")),
    );
    assert_eq!(
        carol_peer_fallback_array[0].pointer("/forwardContext/origin/authorName"),
        Some(&json!("Bob")),
    );

    let album_message = session
        .append_human_message(
            &conversation_id,
            "architecture visual pack",
            "nonce-album-media",
            Some(1025.0),
            None,
            &[
                json!({
                    "path": "/tmp/arch-flow.png",
                    "name": "arch-flow.png",
                    "mimeType": "image/png",
                    "width": 1920,
                    "height": 1080
                }),
                json!({
                    "path": "/tmp/arch-schema.png",
                    "name": "arch-schema.png",
                    "mimeType": "image/png",
                    "width": 1280,
                    "height": 720
                }),
            ],
        )
        .expect("append multi-attachment human album message");
    let album_media_results = dispatch_production_session_gateway_call(
        &session,
        "searchMedia",
        &json!({"query": "visual pack png", "limit": 10}),
    )
    .expect("album searchMedia method")
    .expect("album searchMedia");
    let album_media_array = album_media_results.as_array().expect("album media array");
    assert_eq!(album_media_array.len(), 2);
    assert_eq!(album_media_array[0]["entryId"], album_message["id"]);
    assert_eq!(album_media_array[0]["fileName"], json!("arch-flow.png"));
    assert_eq!(album_media_array[0]["width"], json!(1920));
    assert_eq!(album_media_array[0]["height"], json!(1080));
    assert_eq!(album_media_array[1]["entryId"], album_message["id"]);
    assert_eq!(album_media_array[1]["fileName"], json!("arch-schema.png"));
    assert_eq!(album_media_array[1]["width"], json!(1280));
    assert_eq!(album_media_array[1]["height"], json!(720));

    session.shutdown();

    let foreign_memory = Arc::new(
        mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(root.clone()),
    );
    let foreign = Arc::new(
        ProductionSessionWorkers::with_agents_root_identity_and_dependencies(
            &root,
            500,
            Some("human-foreign".into()),
            Arc::new(|| None),
            foreign_memory,
        ),
    );
    let foreign_messages = dispatch_production_session_gateway_call_with_content_search(
        &foreign,
        Some(&indexed),
        "searchAgents",
        &json!({"query":"needle","limit":10}),
    )
    .expect("foreign searchAgents method")
    .expect("foreign searchAgents");
    let foreign_array = foreign_messages.as_array().expect("foreign array");
    assert_eq!(foreign_array.len(), 1);
    assert_eq!(foreign_array[0]["agentId"], agent.id);

    let foreign_media = dispatch_production_session_gateway_call_with_content_search(
        &foreign,
        Some(&indexed),
        "searchMedia",
        &json!({"query":"needle","limit":10}),
    )
    .expect("foreign searchMedia method")
    .expect("foreign searchMedia");
    let foreign_media_array = foreign_media.as_array().expect("foreign media array");
    assert_eq!(foreign_media_array.len(), 1);
    assert_eq!(foreign_media_array[0]["agentId"], agent.id);

    let foreign_scoped = dispatch_production_session_gateway_call_with_content_search(
        &foreign,
        Some(&indexed),
        "searchHumanMessages",
        &json!({
            "conversationId": conversation_id,
            "query": "needle",
            "limit": 10
        }),
    )
    .expect("foreign searchHumanMessages")
    .expect_err("foreign account must be rejected from non-participant Human search");
    assert!(format!("{foreign_scoped:?}").contains("not a conversation participant"));

    foreign.shutdown();
    let _ = fs::remove_dir_all(root);
}
