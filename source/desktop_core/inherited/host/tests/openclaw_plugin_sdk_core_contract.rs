use mahayana_host_runtime::extensions::plugin_sdk::approval_security::{
    resolve_approval_approvers, validate_outbound_url, ApprovalBinding, NativeApprovalControlRegistry,
    SsrfCheckResult,
};
use mahayana_host_runtime::extensions::plugin_sdk::channel_runtime::{
    calculate_retry_delay, normalize_recipient_list, ChannelConfig, ChannelReplayGuard, DmPolicy,
    GroupPolicy, ReplayClaimResult,
};
use mahayana_host_runtime::extensions::plugin_sdk::session_memory::{
    decode_visible_transcript_cursor, encode_visible_transcript_cursor, new_visible_transcript_cursor,
    normalize_page_limit, paginate_sessions, select_visible_transcript_messages,
    read_visible_transcript_page, visible_reset_boundary_changed, with_visible_reset_boundary,
    SessionCatalogQueryParams, SessionSummary, SessionTranscriptLockRegistry, SessionVisibility,
    VisibleTranscriptReadResult, VisibleTranscriptResetReason, VisibleTranscriptStart,
    VisibleTranscriptReadFence,
    DEFAULT_PAGE_LIMIT, MAX_CURSOR_LENGTH, MAX_PAGE_LIMIT,
};
use mahayana_host_runtime::extensions::plugin_sdk::tool_runtime::{
    BrowserHookBridge, PluginCommandDispatcher, PluginToolDefinition,
};
use serde_json::json;
use std::time::Duration;

#[test]
fn test_ocrm_plugin_sdk_channel_runtime() {
    let mut config = ChannelConfig::new("chan-main");
    config.dm_policy = DmPolicy::Allowlist;
    config.group_policy = GroupPolicy::Allowlist;
    config.allow_from = vec!["alice".to_string(), "bob".to_string()];

    // Access check
    assert!(config.can_access_dm("alice"));
    assert!(config.can_access_dm("ALICE"));
    config.allow_from = vec![" alice ".to_string(), "bob".to_string()];
    assert!(config.can_access_dm("  ALICE  "));
    assert!(!config.can_access_dm(""));
    assert!(!config.can_access_dm("charlie"));

    assert!(config.can_access_group("grp-1", "bob"));
    assert!(!config.can_access_group("grp-1", "david"));

    // Replay guard
    let guard = ChannelReplayGuard::new();
    let ttl = Duration::from_millis(500);

    // Initial claim succeeds
    assert_eq!(guard.claim("msg-001", ttl), ReplayClaimResult::Claimed);

    // Inflight while pending commit
    assert_eq!(guard.claim("msg-001", ttl), ReplayClaimResult::InFlight);

    // Commit
    assert!(guard.commit("msg-001", ttl));

    // After commit, claim returns Duplicate
    assert_eq!(guard.claim("msg-001", ttl), ReplayClaimResult::Duplicate);
    assert!(guard.is_committed("msg-001"));

    // Retry calculation
    assert_eq!(calculate_retry_delay(0, 100, 5000), Duration::from_millis(100));
    assert_eq!(calculate_retry_delay(1, 100, 5000), Duration::from_millis(200));
    assert_eq!(calculate_retry_delay(2, 100, 5000), Duration::from_millis(400));
    assert_eq!(calculate_retry_delay(10, 100, 5000), Duration::from_millis(5000));

    // Recipient normalization
    let recips = normalize_recipient_list("user:1, user:2; USER:1 \n user:3");
    assert_eq!(recips, vec!["user:1", "user:2", "user:3"]);

    // Bundled channel contract tests (from channel-*.contract.test.ts)
    use mahayana_host_runtime::extensions::plugin_sdk::channel_runtime::BundledChannelContractValidator;

    // 1. configWrites admission
    assert!(BundledChannelContractValidator::is_config_writes_admitted(&["token", "configWrites"]));
    assert!(!BundledChannelContractValidator::is_config_writes_admitted(&["token"]));

    // 2. historyLimit support (root vs account level)
    assert!(BundledChannelContractValidator::supports_history_limit("line", false));
    assert!(BundledChannelContractValidator::supports_history_limit("line", true));
    assert!(BundledChannelContractValidator::supports_history_limit("mattermost", true));
    assert!(BundledChannelContractValidator::supports_history_limit("twitch", false));
    assert!(!BundledChannelContractValidator::supports_history_limit("twitch", true));
    assert!(!BundledChannelContractValidator::supports_history_limit("unsupported", false));

    // 3. mediaMaxMb validation (number > 0)
    assert!(BundledChannelContractValidator::validate_media_max_mb(&json!(20)));
    assert!(BundledChannelContractValidator::validate_media_max_mb(&json!(0.5)));
    assert!(!BundledChannelContractValidator::validate_media_max_mb(&json!(0)));
    assert!(!BundledChannelContractValidator::validate_media_max_mb(&json!(-1)));
    assert!(!BundledChannelContractValidator::validate_media_max_mb(&json!("20")));

    // 4. replyToMode support
    assert!(BundledChannelContractValidator::supports_reply_to_mode("feishu"));
    assert!(BundledChannelContractValidator::supports_reply_to_mode("irc"));
    assert!(BundledChannelContractValidator::supports_reply_to_mode("nextcloud-talk"));
    assert!(!BundledChannelContractValidator::supports_reply_to_mode("discord"));

    // 5. responsePrefix string validation
    assert!(BundledChannelContractValidator::validate_response_prefix(&json!("[bot]")));
    assert!(BundledChannelContractValidator::validate_response_prefix(&json!("auto")));
    assert!(BundledChannelContractValidator::validate_response_prefix(&json!("")));
    assert!(!BundledChannelContractValidator::validate_response_prefix(&json!(42)));

    // 6. Channel skill eligibility and retired pattern checks
    assert!(BundledChannelContractValidator::is_channel_skill_eligible(true, true));
    assert!(!BundledChannelContractValidator::is_channel_skill_eligible(false, true));
    assert!(!BundledChannelContractValidator::is_channel_skill_eligible(true, false));

    assert!(BundledChannelContractValidator::contains_retired_skill_pattern("use the 'slack' tool to send"));
    assert!(BundledChannelContractValidator::contains_retired_skill_pattern("calling feishu_doc_list_blocks"));
    assert!(!BundledChannelContractValidator::contains_retired_skill_pattern("clean skill instructions"));
}

#[test]
fn test_ocrm_plugin_sdk_approval_security() {
    // 1. Approver resolution
    let explicit = vec!["lead-approver".to_string()];
    let allow_from = vec!["team-member".to_string()];
    let approvers = resolve_approval_approvers(Some(&explicit), Some(&allow_from), None, Some("admin"));
    assert_eq!(approvers, vec!["lead-approver"]);

    let empty_explicit: Vec<String> = vec![];
    let approvers_fallback =
        resolve_approval_approvers(Some(&empty_explicit), Some(&allow_from), None, Some("admin"));
    assert_eq!(approvers_fallback, vec!["team-member", "admin"]);

    // 2. Native approval control registry & token generation
    let registry = NativeApprovalControlRegistry::new();
    let token = NativeApprovalControlRegistry::generate_token().expect("Token generation");
    assert_eq!(token.len(), 24); // 18 bytes URL_SAFE_NO_PAD base64 = 24 chars

    let binding = ApprovalBinding {
        token: token.clone(),
        action_name: "deploy_service".to_string(),
        session_id: "sess-99".to_string(),
        approver_id: None,
        expires_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 60_000,
    };
    assert!(registry.register(binding));

    let retrieved = registry.get(&token).expect("Binding should be retrieved");
    assert_eq!(retrieved.action_name, "deploy_service");

    let completed = registry.complete(&token).expect("Complete removes binding");
    assert_eq!(completed.action_name, "deploy_service");
    assert!(registry.get(&token).is_none());

    // 3. SSRF Protection
    assert!(matches!(
        validate_outbound_url("http://127.0.0.1:8080/api"),
        SsrfCheckResult::Disallowed(_)
    ));
    assert!(matches!(
        validate_outbound_url("http://localhost/secret"),
        SsrfCheckResult::Disallowed(_)
    ));
    assert!(matches!(
        validate_outbound_url("http://10.200.1.5/admin"),
        SsrfCheckResult::Disallowed(_)
    ));
    assert!(matches!(
        validate_outbound_url("http://172.16.50.1/meta"),
        SsrfCheckResult::Disallowed(_)
    ));
    assert!(matches!(
        validate_outbound_url("http://192.168.1.1/router"),
        SsrfCheckResult::Disallowed(_)
    ));
    assert!(matches!(
        validate_outbound_url("http://169.254.169.254/latest/meta-data"),
        SsrfCheckResult::Disallowed(_)
    ));
    assert!(matches!(
        validate_outbound_url("ftp://files.example.com/data"),
        SsrfCheckResult::Disallowed(_)
    ));
    assert_eq!(
        validate_outbound_url("https://api.github.com/v1/repos"),
        SsrfCheckResult::Allowed
    );
}

#[test]
fn test_ocrm_plugin_sdk_session_memory() {
    let sessions = vec![
        SessionSummary {
            session_id: "s-1".to_string(),
            thread_id: "th-1".to_string(),
            title: "Project Alpha Discussion".to_string(),
            visibility: SessionVisibility::Public,
            updated_at_ms: 1000,
            message_count: 5,
        },
        SessionSummary {
            session_id: "s-2".to_string(),
            thread_id: "th-2".to_string(),
            title: "Private Alpha Security Review".to_string(),
            visibility: SessionVisibility::Direct,
            updated_at_ms: 2000,
            message_count: 12,
        },
        SessionSummary {
            session_id: "s-3".to_string(),
            thread_id: "th-3".to_string(),
            title: "Ephemeral Beta Exploration".to_string(),
            visibility: SessionVisibility::Incognito,
            updated_at_ms: 3000,
            message_count: 2,
        },
    ];

    // Page 1 with limit 2
    let params = SessionCatalogQueryParams {
        search_term: Some("Alpha".to_string()),
        limit: 2,
        cursor: None,
    };

    let page1 = paginate_sessions(&sessions, &params).expect("Pagination succeeds");
    assert_eq!(page1.items.len(), 2);
    assert_eq!(page1.total_count, 2);
    assert!(page1.next_cursor.is_none()); // only 2 matched, limit is 2, no more items

    // Enforce cursor length limits and max page limit
    assert_eq!(normalize_page_limit(999), MAX_PAGE_LIMIT);
    let long_cursor = "a".repeat(MAX_CURSOR_LENGTH + 1);
    let overflow_params = SessionCatalogQueryParams {
        search_term: None,
        limit: DEFAULT_PAGE_LIMIT,
        cursor: Some(long_cursor),
    };
    assert!(paginate_sessions(&sessions, &overflow_params).is_err());

    // Incognito sync restriction
    let lock_registry = SessionTranscriptLockRegistry::new();
    let lock = lock_registry.acquire_lock("agent-1", "s-1");
    let _guard = lock.lock().unwrap();

    assert!(lock_registry
        .enforce_sync_allowed(SessionVisibility::Public)
        .is_ok());
    assert!(lock_registry
        .enforce_sync_allowed(SessionVisibility::Direct)
        .is_ok());
    let err = lock_registry
        .enforce_sync_allowed(SessionVisibility::Incognito)
        .unwrap_err();
    assert!(err.contains("IncognitoSessionSyncAccessDenied"));

    let entries = vec![
        json!({"role": "user", "content": "question"}),
        json!({
            "role": "assistant",
            "content": [{"type": "toolCall", "id": "call-1", "name": "read", "arguments": {}}]
        }),
        json!({"role": "toolResult", "toolCallId": "call-1", "content": [{"type": "text", "text": "paired"}]}),
        json!({"role": "toolResult", "toolCallId": "orphan", "content": [{"type": "text", "text": "orphan"}]}),
        json!({"role": "assistant", "content": "answer"}),
    ];
    assert_eq!(
        select_visible_transcript_messages(&entries, Some(1)),
        entries[1..3].to_vec().into_iter().chain(entries[4..5].to_vec()).collect::<Vec<_>>(),
    );

    let cursor = new_visible_transcript_cursor("agent-1", "session-1", "generation-1");
    let cursor = with_visible_reset_boundary(cursor, Some((42, Some("reset-42"))))
        .expect("reset boundary should be valid");
    let encoded = encode_visible_transcript_cursor(&cursor).expect("cursor should encode");
    assert_eq!(decode_visible_transcript_cursor(&encoded), Some(cursor.clone()));
    assert!(visible_reset_boundary_changed(
        &cursor,
        Some((43, Some("reset-43"))),
        true,
    ));
    assert!(!visible_reset_boundary_changed(
        &cursor,
        Some((42, Some("reset-42"))),
        true,
    ));
    assert!(visible_reset_boundary_changed(
        &cursor,
        Some((1, Some("reset-new-generation"))),
        false,
    ));

    let first = read_visible_transcript_page(
        &entries,
        "agent-1",
        "session-1",
        "generation-1",
        Some((42, Some("reset-42"))),
        Some(1),
        VisibleTranscriptStart::ResetWindow,
        None,
        None,
        2,
        10_000,
    )
    .expect("visible page should read");
    let (next_cursor, first_entries) = match first {
        VisibleTranscriptReadResult::Page { cursor, entries, .. } => (cursor, entries),
        other => panic!("expected page, got {other:?}"),
    };
    assert_eq!(first_entries, entries[1..3].to_vec());
    let reset = read_visible_transcript_page(
        &entries,
        "agent-1",
        "session-1",
        "generation-1",
        Some((43, Some("reset-43"))),
        Some(1),
        VisibleTranscriptStart::ResetWindow,
        Some(&next_cursor),
        None,
        2,
        10_000,
    )
    .expect("reset response should be returned");
    assert!(matches!(
        reset,
        VisibleTranscriptReadResult::Reset {
            reason: VisibleTranscriptResetReason::SessionReset,
            ..
        }
    ));

    let fence = VisibleTranscriptReadFence {
        agent_id: "agent-1".into(),
        session_id: "session-1".into(),
        generation: "generation-1".into(),
        before_message_position: 2,
    };
    let fenced = read_visible_transcript_page(
        &entries,
        "agent-1",
        "session-1",
        "generation-1",
        Some((42, Some("reset-42"))),
        Some(1),
        VisibleTranscriptStart::ResetWindow,
        None,
        Some(&fence),
        10,
        10_000,
    )
    .expect("current-turn fence should bound the page");
    match fenced {
        VisibleTranscriptReadResult::Page {
            entries: fenced_entries,
            ..
        } => {
            assert_eq!(fenced_entries, vec![entries[1].clone(), entries[2].clone()]);
        }
        other => panic!("expected fenced page, got {other:?}"),
    }
}

#[test]
fn test_ocrm_plugin_sdk_tool_runtime() {
    let tool = PluginToolDefinition::new(
        "calculator",
        "Perform arithmetic",
        json!({
            "type": "object",
            "required": ["operation", "operands"],
            "properties": {
                "operation": {"type": "string"},
                "operands": {"type": "array"}
            }
        }),
    );

    // Valid arguments
    let valid_args = json!({
        "operation": "add",
        "operands": [1, 2]
    });
    assert!(tool.validate_arguments(&valid_args).is_ok());

    // Missing required argument
    let invalid_args = json!({
        "operands": [1, 2]
    });
    assert!(tool.validate_arguments(&invalid_args).is_err());

    // Dispatcher
    let dispatcher = PluginCommandDispatcher::new();
    dispatcher.register("echo", |args| Ok(args.clone()));

    let echo_res = dispatcher
        .dispatch("echo", &json!({"hello": "world"}), Duration::from_secs(1))
        .expect("Dispatch echo");
    assert_eq!(echo_res, json!({"hello": "world"}));

    // Browser hook bridge
    let bridge = BrowserHookBridge::new();
    let res = bridge.execute_with_tab_lock("tab-123", || Ok("tab locked and executed"));
    assert_eq!(res.unwrap(), "tab locked and executed");
}
