#[path = "common/mod.rs"]
mod common;

use common::*;
use axum::http::StatusCode;
use gpui_telegram_agent::hermes::providers::{CredentialEntry, CredentialPool, ProviderRouter};
use magpie_gateway::{
    build_gemini_generate_content_payload, inbound_chat_completions_to_unified,
    parse_magpie_import_link, MaskingConfig, MaskingEngine, RouteRuntimeMetrics,
    RoutingGroup, RoutingGroupMember, RoutingStrategy, StreamingUnmasker,
    VertexAdcCredentials, VertexTokenManager,
};
use serde_json::json;
use std::collections::HashMap;

// ============================================================================
// Tier 3: Cross-Feature Combinations (5 Tests)
// ============================================================================

#[tokio::test]
async fn test_tier3_discovery_fallback_stream() {
    let temp = new_temp_dir("tier3_discovery");
    let (oauth_url, oauth_state) = spawn_mock_oauth().await;
    let (openai_url, _gemini_url, mock_prov_state) = spawn_mock_providers().await;
    mock_prov_state
        .custom_reply_text
        .lock()
        .unwrap()
        .replace("TIER3_DISCOVERY_FALLBACK_STREAM_OK".to_string());

    // 1. Create mock Vertex ADC credentials
    let _adc_path = create_mock_vertex_adc_dir(
        temp.path(),
        "mock-client-id.apps.googleusercontent.com",
        "mock-client-secret",
        "1//04mock_refresh_token",
        Some("mock-quota-project"),
        Some("vertex-agent@example.com"),
    );

    // 2. Discover credentials in temp home
    let router = ProviderRouter::new();
    let reg = router.refresh_local_subscriptions_in_home(temp.path(), None);
    assert!(reg.has_active_subscription());
    assert!(router.has_active_subscription());
    assert!(!router.needs_provider_selection());

    // 3. Test OAuth2 token exchange with mock OAuth server
    let creds = VertexAdcCredentials {
        client_id: "mock-client-id.apps.googleusercontent.com".to_string(),
        client_secret: "mock-client-secret".to_string(),
        refresh_token: "1//04mock_refresh_token".to_string(),
        quota_project_id: Some("mock-quota-project".to_string()),
        account: Some("vertex-agent@example.com".to_string()),
    };
    let token_mgr = VertexTokenManager::new(creds).with_token_url(oauth_url);
    let http_client = reqwest::Client::new();
    let token_resp = token_mgr
        .execute_refresh(&http_client)
        .await
        .expect("fetch oauth token from mock");
    assert!(!token_resp.access_token.is_empty());
    assert_eq!(oauth_state.call_count.load(std::sync::atomic::Ordering::SeqCst), 1);

    // 4. Autonomous fallback: verify router does NOT return PROVIDER_ERROR
    let (_dir, state, app_router) = boot_test_server();
    // Scan subscriptions into server
    state
        .hermes
        .providers()
        .refresh_local_subscriptions_in_home(temp.path(), None);
    assert!(state.hermes.providers().has_active_subscription());
    assert!(!state.hermes.providers().needs_provider_selection());

    let (_uid, tok) = register_user(&app_router, "tier3_fallback_user").await;

    // Check magpie/state shows needs_provider_selection = false
    let (st_state, state_val) = http_json(
        &app_router,
        "GET",
        "/api/v1/hermes/magpie/state",
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st_state, StatusCode::OK);
    assert_eq!(state_val["needs_provider_selection"].as_bool(), Some(false));

    // 5. Wire discovered credential / refreshed OAuth token to mock upstream endpoint
    // and execute end-to-end streaming chat turn.
    let (st_save, _) = http_json(
        &app_router,
        "POST",
        "/api/v1/hermes/magpie/providers",
        Some(&tok),
        Some(json!({
            "preset_id": "deepseek",
            "api_key": &token_resp.access_token,
            "base_url": openai_url,
            "default_model": "deepseek-chat",
            "activate": true
        })),
    )
    .await;
    assert_eq!(st_save, StatusCode::OK);

    // Dispatch streaming chat turn through /api/v1/magpie/v1/chat/completions
    let (st_stream, ct, bytes) = http_raw(
        &app_router,
        "POST",
        "/api/v1/magpie/v1/chat/completions",
        Some(&tok),
        Some(json!({
            "model": "deepseek-chat",
            "messages": [{"role": "user", "content": "Execute fallback turn"}],
            "stream": true
        })),
    )
    .await;
    assert_eq!(st_stream, StatusCode::OK);
    assert!(ct.contains("text/event-stream"));
    let body_str = String::from_utf8_lossy(&bytes);
    assert!(
        body_str.contains("TIER3_DISCOVERY_FALLBACK_STREAM_OK"),
        "Stream output must contain custom reply text: {body_str}"
    );
    assert!(body_str.contains("[DONE]"));

    // Verify mock upstream was invoked and received the refreshed OAuth access token
    assert!(mock_prov_state.call_count.load(std::sync::atomic::Ordering::SeqCst) >= 1);
    let captured_headers = mock_prov_state.captured_headers.lock().unwrap();
    assert!(!captured_headers.is_empty());
    let auth_hdr = captured_headers[0].get("authorization").expect("auth header present");
    assert!(
        auth_hdr.contains(&token_resp.access_token),
        "Upstream request must carry bearer token from OAuth exchange: {auth_hdr}"
    );
}

#[tokio::test]
async fn test_tier3_multi_account_rotation_quota_exhaustion() {
    let now_ms = 1_000_000i64;

    // 1. Setup multi-account pool with Account 1 and Account 2
    let mut pool = CredentialPool::new();
    pool.add_entry(CredentialEntry::new("account_1", "sk-key-1", "primary"));
    pool.add_entry(CredentialEntry::new("account_2", "sk-key-2", "backup"));

    assert_eq!(pool.len(), 2);
    assert_eq!(pool.active_count(now_ms), 2);

    // Acquire turn 1 -> Account 1
    let acq1 = pool.acquire(now_ms).expect("acquire turn 1");
    assert_eq!(acq1.id, "account_1");

    // Account 1 hits upstream 429 Too Many Requests
    pool.report_rate_limit("account_1", now_ms, 429, Some(60));
    assert_eq!(pool.active_count(now_ms), 1);
    assert_eq!(pool.cooldown_count(now_ms), 1);

    // Failover: next acquire automatically selects Account 2!
    let acq2 = pool.acquire(now_ms).expect("failover to account 2");
    assert_eq!(acq2.id, "account_2");

    // Account 2 succeeds
    pool.report_success("account_2");

    // 2. Also test routing group level failover
    let mut groups = HashMap::new();
    let mut group = RoutingGroup::new("multi_acc", "Multi Account Group", RoutingStrategy::Smart);
    group.members = vec![
        RoutingGroupMember::new("p1/m1"),
        RoutingGroupMember::new("p2/m2"),
    ];
    groups.insert("multi_acc".to_string(), group);

    let mut metrics = RouteRuntimeMetrics::default();
    // Mark p1 in 429 cooldown
    metrics.record_failure("p1/m1", Some(429), Some(30), now_ms);

    let targets = magpie_gateway::routing::resolve_group_targets(
        "group/multi_acc",
        &groups,
        &mut metrics,
        "p1",
        None,
        "Turn prompt",
        1,
        0,
        now_ms,
    )
    .expect("resolve failover targets");

    // Candidate 1 was p1/m1, but since it is cooled down, smart ranking demotes it
    // Strictly verify failover: p2 is primary target, p1 is demoted
    assert_eq!(targets[0].provider_id, "p2");
    assert_eq!(targets[0].model, "m2");
    assert_eq!(targets[1].provider_id, "p1");
    assert_eq!(targets[1].model, "m1");
}

#[test]
fn test_tier3_privacy_masking_with_protocol_translation() {
    // 1. Mask sensitive tokens in prompt using MaskingEngine with enabled=true
    let mut config = MaskingConfig::default();
    config.enabled = true;
    let engine = MaskingEngine::new(config);

    let sensitive_prompt = "My email is developer@internal.net and secret API token is sk-live-secret-token-5555";
    let mask_res = engine.mask_text(sensitive_prompt);
    assert!(!mask_res.masked_text.contains("developer@internal.net"));
    assert!(!mask_res.masked_text.contains("sk-live-secret-token-5555"));
    assert!(!mask_res.hits.is_empty());

    // 2. Convert masked request from OpenAI format to Unified, then to Gemini format
    let openai_req = json!({
        "model": "gemini-2.5-flash",
        "messages": [
            { "role": "user", "content": mask_res.masked_text }
        ]
    });

    let unified = inbound_chat_completions_to_unified(&openai_req).expect("convert to unified");
    let gemini_payload = build_gemini_generate_content_payload(&unified);

    // Verify Gemini payload contains masked tokens and zero raw secrets
    let gemini_str = gemini_payload.to_string();
    assert!(!gemini_str.contains("developer@internal.net"));
    assert!(!gemini_str.contains("sk-live-secret-token-5555"));

    // 3. Unmask incoming response stream using StreamingUnmasker
    let mut stream_unmasker = StreamingUnmasker::new();
    let simulated_stream_chunk = format!("Echo: {}", mask_res.hits[0].placeholder);
    let emitted = stream_unmasker.push(&simulated_stream_chunk, &engine);
    let final_unmasked = format!("{}{}", emitted, stream_unmasker.finish(&engine));
    assert!(
        final_unmasked.contains("developer@internal.net")
            || final_unmasked.contains("sk-live-secret-token-5555"),
        "unmasked stream must contain restored secret: {final_unmasked}"
    );
}

#[tokio::test]
async fn test_tier3_import_deep_link_to_active_routing() {
    let link = "magpie://import?name=MyDeepSeekRelay&preset=deepseek&key=sk-imported-relay-key-1234";
    let spec = parse_magpie_import_link(link).expect("parse deep link");
    assert_eq!(spec.preset_id.as_deref(), Some("deepseek"));
    assert_eq!(spec.api_key.as_deref(), Some("sk-imported-relay-key-1234"));
    assert_eq!(spec.base_url, "https://api.deepseek.com/v1");

    // Import into test server
    let (_dir, state, router) = boot_test_server();
    isolate_empty_providers(&state, _dir.path());
    let (_uid, tok) = register_user(&router, "tier3_import_user").await;

    let (st_import, val_import) = http_json(
        &router,
        "POST",
        "/api/v1/hermes/magpie/import",
        Some(&tok),
        Some(json!({ "raw": link })),
    )
    .await;
    assert_eq!(st_import, StatusCode::OK, "import: {val_import}");
    assert_eq!(val_import["imported_count"].as_u64(), Some(1));

    // Verify imported provider exists in server state
    let (st_state, val_state) = http_json(
        &router,
        "GET",
        "/api/v1/hermes/magpie/state",
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st_state, StatusCode::OK);
    let provs = val_state["providers"].as_array().expect("providers");
    assert!(
        provs.iter().any(|p| p["provider_id"].as_str() == Some("deepseek")),
        "imported provider must exist in server state"
    );
}

#[test]
fn test_tier3_session_affinity_with_rebalance() {
    let mut groups = HashMap::new();
    let mut group = RoutingGroup::new("sticky_rebal", "Sticky Rebalance", RoutingStrategy::Smart);
    group.sticky_ttl_ms = 10_000;
    group.members = vec![
        RoutingGroupMember::new("p1/m1"),
        RoutingGroupMember::new("p2/m2"),
    ];
    groups.insert("sticky_rebal".to_string(), group);

    let mut metrics = RouteRuntimeMetrics::default();
    let now_ms = 2_000_000i64;

    // Pin session to candidate A (p1/m1)
    metrics.session_affinity.insert(
        "sess_turn::group/sticky_rebal".to_string(),
        magpie_gateway::SessionAffinityEntry {
            target: "p1/m1".to_string(),
            expires_at_ms: now_ms + 10_000,
        },
    );

    // Initial check: routes to p1/m1 via session affinity
    let targets_init = magpie_gateway::routing::resolve_group_targets(
        "group/sticky_rebal",
        &groups,
        &mut metrics,
        "p1",
        Some("sess_turn"),
        "Turn 1",
        1,
        0,
        now_ms,
    )
    .expect("turn 1");
    assert_eq!(targets_init[0].provider_id, "p1");
    assert_eq!(targets_init[0].reason, "session_affinity");

    // Candidate A fails with 500 server error and enters cooldown
    metrics.record_failure("p1/m1", Some(500), None, now_ms);
    assert!(metrics.seats.get("p1/m1").unwrap().is_in_cooldown(now_ms));

    // Next turn for same session: because candidate A is in cooldown, affinity is skipped
    // and session gracefully rebalances to healthy candidate B (p2/m2)!
    let targets_rebal = magpie_gateway::routing::resolve_group_targets(
        "group/sticky_rebal",
        &groups,
        &mut metrics,
        "p1",
        Some("sess_turn"),
        "Turn 2",
        1,
        0,
        now_ms,
    )
    .expect("turn 2 rebalance");

    assert_eq!(targets_rebal[0].provider_id, "p2");
    assert_eq!(targets_rebal[0].model, "m2");
}
