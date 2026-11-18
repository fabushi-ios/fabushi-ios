#[path = "common/mod.rs"]
mod common;

use common::*;
use axum::http::StatusCode;
use magpie_gateway::{
    all_presets, canonical_preset_alias, discover_subscriptions_in_home, find_preset,
    parse_magpie_logins, parse_magpie_providers, parse_vertex_adc_credentials,
    resolve_preset_endpoints, scan_vertex_adc, ProtocolKind,
    RouteRuntimeMetrics, RouteSeatMetrics, RoutingGroup, RoutingGroupMember, RoutingStrategy,
    SubscriptionKind,
};
use serde_json::json;
use std::collections::HashMap;

// ============================================================================
// Feature Family 1: Discovery Scanners (5 Tests)
// ============================================================================

#[test]
fn test_tier1_discovery_vertex_adc() {
    let temp = new_temp_dir("adc");
    let adc_path = create_mock_vertex_adc_dir(
        temp.path(),
        "mock-client-id-12345.apps.googleusercontent.com",
        "mock-secret-98765",
        "1//04mock_refresh_token_vertex_adc",
        Some("test-vertex-quota-project"),
        Some("vertex-dev@example.com"),
    );

    let raw = std::fs::read_to_string(&adc_path).expect("read adc");
    let parsed = parse_vertex_adc_credentials(&raw).expect("parse adc credentials");
    assert_eq!(parsed.client_id, "mock-client-id-12345.apps.googleusercontent.com");
    assert_eq!(parsed.client_secret, "mock-secret-98765");
    assert_eq!(parsed.refresh_token, "1//04mock_refresh_token_vertex_adc");
    assert_eq!(parsed.quota_project_id.as_deref(), Some("test-vertex-quota-project"));
    assert_eq!(parsed.account.as_deref(), Some("vertex-dev@example.com"));

    let account = scan_vertex_adc(temp.path(), None).expect("scan_vertex_adc account");
    assert_eq!(account.kind, SubscriptionKind::GoogleVertex);
    assert!(account.active);
    assert_eq!(account.account_id.as_deref(), Some("test-vertex-quota-project"));
    assert_eq!(account.email.as_deref(), Some("vertex-dev@example.com"));
    assert_eq!(
        account.headers.get("x-goog-user-project").map(String::as_str),
        Some("test-vertex-quota-project")
    );

    // Verify secrets are omitted from serialized summary
    let serialized = serde_json::to_string(&account).expect("serialize account");
    assert!(!serialized.contains("mock-secret-98765"));
    assert!(!serialized.contains("1//04mock_refresh_token_vertex_adc"));
}

#[test]
fn test_tier1_discovery_codex_jwt() {
    let temp = new_temp_dir("codex");
    let jwt = create_mock_jwt("codex-user@company.com", "plus", "acct_codex_9999", 3600);
    create_mock_codex_auth_dir(
        temp.path(),
        "codex-user@company.com",
        "plus",
        &jwt,
        "acct_codex_9999",
    );

    let reg = discover_subscriptions_in_home(temp.path(), None);
    assert!(
        reg.accounts.iter().any(|a| a.kind == SubscriptionKind::CodexChatGpt),
        "expected CodexChatGpt in discovered accounts: {:?}",
        reg.accounts
    );
    let codex_acc = reg
        .accounts
        .iter()
        .find(|a| a.kind == SubscriptionKind::CodexChatGpt)
        .unwrap();
    assert!(codex_acc.active);
    assert_eq!(codex_acc.email.as_deref(), Some("codex-user@company.com"));
    assert_eq!(codex_acc.plan.as_deref(), Some("plus"));
    assert_eq!(
        codex_acc.headers.get("originator").map(String::as_str),
        Some("codex_cli_rs")
    );
}

#[test]
fn test_tier1_discovery_claude_profiles() {
    let temp = new_temp_dir("claude");
    create_mock_claude_dir(
        temp.path(),
        "sk-ant-mock-claude-token-777",
        Some("claude-architect@example.com"),
        Some("team"),
        Some("claude-3-5-sonnet-20241022"),
    );

    let reg = discover_subscriptions_in_home(temp.path(), None);
    let claude_acc = reg
        .accounts
        .iter()
        .find(|a| a.kind == SubscriptionKind::ClaudeCode)
        .expect("found ClaudeCode account");
    assert!(claude_acc.active);
    assert_eq!(claude_acc.protocol, ProtocolKind::AnthropicMessages);
    assert_eq!(
        claude_acc.default_model.as_deref(),
        Some("claude-3-5-sonnet-20241022")
    );
}

#[test]
fn test_tier1_discovery_gemini_assist() {
    let temp = new_temp_dir("gemini");
    create_mock_gemini_dir(
        temp.path(),
        "ya29.mock-gemini-token-8888",
        Some("gemini-specialist@example.com"),
        Some("refresh-token-gemini-8888"),
    );

    let reg = discover_subscriptions_in_home(temp.path(), None);
    let gemini_acc = reg
        .accounts
        .iter()
        .find(|a| a.kind == SubscriptionKind::GeminiCodeAssist)
        .expect("found GeminiCodeAssist account");
    assert!(gemini_acc.active);
    assert_eq!(gemini_acc.protocol, ProtocolKind::GeminiGenerateContent);
    assert_eq!(gemini_acc.email.as_deref(), Some("gemini-specialist@example.com"));
}

#[test]
fn test_tier1_discovery_magpie_mirroring() {
    let temp = new_temp_dir("magpie_state");
    let adc_path = create_mock_vertex_adc_dir(
        temp.path(),
        "mock-id",
        "mock-sec",
        "mock-rt",
        Some("vertex-magpie-proj"),
        None,
    );

    let providers_json = json!({
        "providers": [
            {
                "id": "google-vertex",
                "name": "Google Vertex Production",
                "off": false,
                "vertex": {
                    "project": "vertex-magpie-proj",
                    "credentials": adc_path.to_string_lossy().to_string()
                }
            }
        ]
    });

    let jwt = create_mock_jwt("magpie-login@example.com", "pro", "acct_magpie_111", 7200);
    let logins_json = json!([
        {
            "id": "codex-primary",
            "agent": "codex",
            "user": "magpie-login@example.com",
            "plan": "pro",
            "on": true,
            "auth": {
                "tokens": {
                    "access_token": jwt,
                    "account_id": "acct_magpie_111"
                }
            }
        }
    ]);

    create_mock_magpie_dir(temp.path(), providers_json.clone(), logins_json.clone());

    let provs = parse_magpie_providers(&providers_json.to_string());
    assert_eq!(provs.len(), 1);
    assert_eq!(provs[0].kind, SubscriptionKind::GoogleVertex);
    assert!(provs[0].active);
    assert_eq!(provs[0].account_id.as_deref(), Some("vertex-magpie-proj"));

    let now_ms = chrono::Utc::now().timestamp_millis();
    let logins = parse_magpie_logins(&logins_json.to_string(), now_ms);
    assert_eq!(logins.len(), 1);
    assert_eq!(logins[0].kind, SubscriptionKind::CodexChatGpt);
    assert!(logins[0].active);
    assert_eq!(logins[0].email.as_deref(), Some("magpie-login@example.com"));
}

// ============================================================================
// Feature Family 2: Presets Catalog (5 Tests)
// ============================================================================

#[test]
fn test_tier1_presets_vertex_regions() {
    let preset = find_preset("google-vertex").expect("google-vertex preset exists");
    assert_eq!(preset.default_protocol, ProtocolKind::GeminiGenerateContent);

    // Test region endpoint resolution
    let resolved_us = resolve_preset_endpoints("google-vertex", Some("us-central1"), None)
        .expect("resolve us-central1");
    assert!(
        resolved_us.base_url.contains("us-central1-aiplatform.googleapis.com")
            || resolved_us.base_url.contains("aiplatform.googleapis.com"),
        "us-central1 endpoint: {}",
        resolved_us.base_url
    );

    let resolved_global = resolve_preset_endpoints("google-vertex", None, None)
        .expect("resolve global vertex");
    assert!(resolved_global.base_url.contains("aiplatform.googleapis.com"));
}

#[test]
fn test_tier1_presets_global_vendors() {
    let presets = all_presets();
    assert!(presets.len() >= 40, "expected >= 40 presets, got {}", presets.len());

    for vendor in ["deepseek", "anthropic", "openai", "moonshot", "ollama"] {
        let p = find_preset(vendor).unwrap_or_else(|| panic!("missing vendor preset: {vendor}"));
        assert!(!p.name.is_empty());
        assert!(!p.default_model.is_empty());
    }
}

#[test]
fn test_tier1_presets_relays_and_local() {
    let _remote = find_preset("remote-magpie").expect("remote-magpie preset");
    let resolved = resolve_preset_endpoints("remote-magpie", None, None).expect("resolve remote");
    assert!(resolved.base_url.contains("magpie") || resolved.base_url.contains("127.0.0.1"));

    let localhost = find_preset("localhost-magpie").or_else(|| find_preset("ollama"));
    assert!(localhost.is_some(), "local runner or relay preset present");
}

#[test]
fn test_tier1_presets_endpoint_resolution() {
    let deepseek = find_preset("deepseek").unwrap();
    assert_eq!(deepseek.default_protocol, ProtocolKind::ChatCompletions);

    let anthropic = find_preset("anthropic").unwrap();
    assert_eq!(anthropic.default_protocol, ProtocolKind::AnthropicMessages);

    let google = find_preset("google").unwrap();
    assert_eq!(google.default_protocol, ProtocolKind::GeminiGenerateContent);
}

#[test]
fn test_tier1_presets_search_and_aliases() {
    assert_eq!(canonical_preset_alias("gemini").0, "google");
    assert_eq!(canonical_preset_alias("kimi").0, "moonshot");
    assert_eq!(canonical_preset_alias("glm").0, "zhipu");
    assert_eq!(canonical_preset_alias("vertex-us").0, "google-vertex");
    assert_eq!(canonical_preset_alias("vertex-us").1, Some("us-central1"));

    let found_by_id = find_preset("openai");
    assert!(found_by_id.is_some());
    assert_eq!(found_by_id.unwrap().id, "openai");

    let found_by_alias = find_preset("gemini");
    assert!(found_by_alias.is_some());
    assert_eq!(found_by_alias.unwrap().id, "google");
}

// ============================================================================
// Feature Family 3: Routing Strategies (5 Tests)
// ============================================================================

#[test]
fn test_tier1_routing_strategy_order() {
    let mut groups = HashMap::new();
    let mut group = RoutingGroup::new("test_order", "Order Group", RoutingStrategy::Order);
    group.members = vec![
        RoutingGroupMember::new("p1/m1"),
        RoutingGroupMember::new("p2/m2"),
    ];
    groups.insert("test_order".to_string(), group);

    let mut metrics = RouteRuntimeMetrics::default();
    let now_ms = chrono::Utc::now().timestamp_millis();

    let targets = magpie_gateway::routing::resolve_group_targets(
        "group/test_order",
        &groups,
        &mut metrics,
        "default_prov",
        None,
        "Hello",
        1,
        0,
        now_ms,
    )
    .expect("resolve targets");

    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].provider_id, "p1");
    assert_eq!(targets[0].model, "m1");
    assert_eq!(targets[1].provider_id, "p2");
    assert_eq!(targets[1].model, "m2");
}

#[test]
fn test_tier1_routing_strategy_rotate() {
    let mut groups = HashMap::new();
    let mut group = RoutingGroup::new("test_rotate", "Rotate Group", RoutingStrategy::Rotate);
    group.members = vec![
        RoutingGroupMember::new("p1/m1"),
        RoutingGroupMember::new("p2/m2"),
    ];
    groups.insert("test_rotate".to_string(), group);

    let mut metrics = RouteRuntimeMetrics::default();
    let now_ms = chrono::Utc::now().timestamp_millis();

    // Call 1: cursor 0 -> p1 first
    let targets1 = magpie_gateway::routing::resolve_group_targets(
        "group/test_rotate",
        &groups,
        &mut metrics,
        "default_prov",
        None,
        "Turn 1",
        1,
        0,
        now_ms,
    )
    .expect("turn 1");
    assert_eq!(targets1[0].provider_id, "p1");

    // Call 2: cursor 1 -> p2 first
    let targets2 = magpie_gateway::routing::resolve_group_targets(
        "group/test_rotate",
        &groups,
        &mut metrics,
        "default_prov",
        None,
        "Turn 2",
        1,
        0,
        now_ms,
    )
    .expect("turn 2");
    assert_eq!(targets2[0].provider_id, "p2");
}

#[test]
fn test_tier1_routing_strategy_smart() {
    let mut groups = HashMap::new();
    let mut group = RoutingGroup::new("test_smart", "Smart Group", RoutingStrategy::Smart);
    group.members = vec![
        RoutingGroupMember::new("p1/m1"),
        RoutingGroupMember::new("p2/m2"),
    ];
    groups.insert("test_smart".to_string(), group);

    let mut metrics = RouteRuntimeMetrics::default();
    let now_ms = chrono::Utc::now().timestamp_millis();

    // Give p1 failures and high latency
    metrics.record_failure("p1/m1", Some(500), None, now_ms);
    metrics.record_failure("p1/m1", Some(500), None, now_ms);

    // Give p2 successful usage
    metrics.record_usage("p2/m2", 100, Some(50), now_ms);

    let targets = magpie_gateway::routing::resolve_group_targets(
        "group/test_smart",
        &groups,
        &mut metrics,
        "default_prov",
        None,
        "Analyze this code",
        2,
        0,
        now_ms,
    )
    .expect("resolve smart");

    // Healthy p2 should be chosen first
    assert_eq!(targets[0].provider_id, "p2");
}

#[test]
fn test_tier1_routing_strategy_usage_cost() {
    let mut groups = HashMap::new();
    let mut group = RoutingGroup::new("test_cheapest", "Cheapest Group", RoutingStrategy::Cheapest);
    let mut m1 = RoutingGroupMember::new("p1/expensive");
    m1.input_cost_per_1m = Some(20.0);
    m1.output_cost_per_1m = Some(60.0);

    let mut m2 = RoutingGroupMember::new("p2/cheapest");
    m2.input_cost_per_1m = Some(0.5);
    m2.output_cost_per_1m = Some(1.5);

    group.members = vec![m1, m2];
    groups.insert("test_cheapest".to_string(), group);

    let mut metrics = RouteRuntimeMetrics::default();
    let now_ms = chrono::Utc::now().timestamp_millis();

    let targets = magpie_gateway::routing::resolve_group_targets(
        "group/test_cheapest",
        &groups,
        &mut metrics,
        "default_prov",
        None,
        "Query",
        1,
        0,
        now_ms,
    )
    .expect("resolve cheapest");

    assert_eq!(targets[0].provider_id, "p2");
    assert_eq!(targets[0].model, "cheapest");
}

#[test]
fn test_tier1_routing_strategy_pace_drain() {
    let mut groups = HashMap::new();
    let mut group = RoutingGroup::new("test_drain", "Drain Group", RoutingStrategy::Drain);
    group.members = vec![
        RoutingGroupMember::new("p1/drain_target"),
        RoutingGroupMember::new("p2/backup_target"),
    ];
    groups.insert("test_drain".to_string(), group);

    let mut metrics = RouteRuntimeMetrics::default();
    let now_ms = chrono::Utc::now().timestamp_millis();

    // Drain strategy pins p1 first until drained
    let targets1 = magpie_gateway::routing::resolve_group_targets(
        "group/test_drain",
        &groups,
        &mut metrics,
        "default_prov",
        None,
        "Turn 1",
        1,
        0,
        now_ms,
    )
    .expect("drain turn 1");
    assert_eq!(targets1[0].provider_id, "p1");

    // Next turn keeps pinning p1
    let targets2 = magpie_gateway::routing::resolve_group_targets(
        "group/test_drain",
        &groups,
        &mut metrics,
        "default_prov",
        None,
        "Turn 2",
        1,
        0,
        now_ms,
    )
    .expect("drain turn 2");
    assert_eq!(targets2[0].provider_id, "p1");
}

// ============================================================================
// Feature Family 4: Proxy Endpoints (5 Tests)
// ============================================================================

#[tokio::test]
async fn test_tier1_proxy_models_endpoint() {
    let (dir, state, router) = boot_test_server();
    isolate_empty_providers(&state, dir.path());
    let (_uid, tok) = register_user(&router, "tier1_models_user").await;

    let (st, val) = http_json(&router, "GET", "/api/v1/magpie/v1/models", Some(&tok), None).await;
    assert_eq!(st, StatusCode::OK, "GET /api/v1/magpie/v1/models: {val}");
    assert_eq!(val["object"].as_str(), Some("list"));
    let data = val["data"].as_array().expect("data array");
    assert!(!data.is_empty(), "models list should not be empty");

    // Also verify root /v1/models returns JSON
    let (st_root, val_root) = http_json(&router, "GET", "/v1/models", Some(&tok), None).await;
    assert_eq!(st_root, StatusCode::OK, "GET /v1/models: {val_root}");
    assert_eq!(val_root["object"].as_str(), Some("list"));
}

#[tokio::test]
async fn test_tier1_proxy_chat_completions_sync() {
    let (dir, state, router) = boot_test_server();
    isolate_empty_providers(&state, dir.path());
    let (openai_url, _gemini_url, mock_state) = spawn_mock_providers().await;
    mock_state.custom_reply_text.lock().unwrap().replace("TIER1_SYNC_PROXY_OK".to_string());

    let (_uid, tok) = register_user(&router, "tier1_sync_user").await;

    // Save DeepSeek provider pointing to mock OpenAI upstream
    let (st_save, _) = http_json(
        &router,
        "POST",
        "/api/v1/hermes/magpie/providers",
        Some(&tok),
        Some(json!({
            "preset_id": "deepseek",
            "api_key": "sk-mock-key-111",
            "base_url": openai_url,
            "default_model": "deepseek-chat",
            "activate": true
        })),
    )
    .await;
    assert_eq!(st_save, StatusCode::OK);

    // Call /api/v1/magpie/v1/chat/completions non-streaming
    let (st_chat, val_chat) = http_json(
        &router,
        "POST",
        "/api/v1/magpie/v1/chat/completions",
        Some(&tok),
        Some(json!({
            "model": "deepseek-chat",
            "messages": [{"role": "user", "content": "Ping"}],
            "stream": false
        })),
    )
    .await;
    assert_eq!(st_chat, StatusCode::OK, "sync chat completions: {val_chat}");
    let reply = val_chat["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
    assert!(reply.contains("TIER1_SYNC_PROXY_OK"), "reply: {reply}");
}

#[tokio::test]
async fn test_tier1_proxy_chat_completions_sse() {
    let (dir, state, router) = boot_test_server();
    isolate_empty_providers(&state, dir.path());
    let (openai_url, _gemini_url, mock_state) = spawn_mock_providers().await;
    mock_state.custom_reply_text.lock().unwrap().replace("TIER1_SSE_STREAM_OK".to_string());

    let (_uid, tok) = register_user(&router, "tier1_sse_user").await;

    let (st_save, _) = http_json(
        &router,
        "POST",
        "/api/v1/hermes/magpie/providers",
        Some(&tok),
        Some(json!({
            "preset_id": "deepseek",
            "api_key": "sk-mock-key-222",
            "base_url": openai_url,
            "default_model": "deepseek-chat",
            "activate": true
        })),
    )
    .await;
    assert_eq!(st_save, StatusCode::OK);

    // Call /api/v1/magpie/v1/chat/completions with stream=true
    let (st, ct, bytes) = http_raw(
        &router,
        "POST",
        "/api/v1/magpie/v1/chat/completions",
        Some(&tok),
        Some(json!({
            "model": "deepseek-chat",
            "messages": [{"role": "user", "content": "Stream please"}],
            "stream": true
        })),
    )
    .await;

    assert_eq!(st, StatusCode::OK);
    assert!(ct.contains("text/event-stream"), "content-type: {ct}");
    let body_str = String::from_utf8_lossy(&bytes);
    assert!(body_str.contains("data:"), "must contain SSE data frames: {body_str}");
    assert!(body_str.contains("TIER1_SSE_STREAM_OK"), "body: {body_str}");
}

#[tokio::test]
async fn test_tier1_proxy_quotas_endpoint() {
    let (dir, state, router) = boot_test_server();
    isolate_empty_providers(&state, dir.path());
    let (_uid, tok) = register_user(&router, "tier1_quotas_user").await;

    // Check magpie/state endpoint which exposes provider rest / quota status
    let (st, val) = http_json(
        &router,
        "GET",
        "/api/v1/hermes/magpie/state",
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "magpie/state: {val}");
    assert!(val.get("providers").is_some());
    assert!(val.get("presets").is_some());
}

#[tokio::test]
async fn test_tier1_proxy_usage_endpoint() {
    let (dir, state, router) = boot_test_server();
    isolate_empty_providers(&state, dir.path());
    let (_uid, tok) = register_user(&router, "tier1_usage_user").await;

    let (st, val) = http_json(&router, "GET", "/api/v1/hermes/usage", Some(&tok), None).await;
    assert_eq!(st, StatusCode::OK, "hermes/usage: {val}");
    assert!(val.get("total_tokens").is_some());
    assert!(val.get("prompt_tokens").is_some());
    assert!(val.get("completion_tokens").is_some());
}

// ============================================================================
// Feature Family 5: Usage Tracking (5 Tests)
// ============================================================================

#[test]
fn test_tier1_usage_token_counting() {
    let mut seat = RouteSeatMetrics::default();
    let now_ms = chrono::Utc::now().timestamp_millis();

    assert_eq!(seat.weighted_tokens, 0.0);
    assert_eq!(seat.total_requests, 0);

    seat.record_usage(150, Some(45), now_ms);
    assert_eq!(seat.weighted_tokens, 150.0);
    assert_eq!(seat.total_requests, 1);
    assert_eq!(seat.last_latency_ms, Some(45));
    assert_eq!(seat.last_status, Some(200));

    seat.record_usage(250, Some(55), now_ms + 1000);
    assert!(seat.weighted_tokens >= 399.0);
    assert_eq!(seat.total_requests, 2);
}

#[test]
fn test_tier1_usage_cost_calculation() {
    let mut m = RoutingGroupMember::new("p1/m1");
    m.input_cost_per_1m = Some(2.5);
    m.output_cost_per_1m = Some(10.0);

    let input_tokens = 200_000u64;
    let output_tokens = 50_000u64;

    let in_cost = (input_tokens as f64 / 1_000_000.0) * m.input_cost_per_1m.unwrap();
    let out_cost = (output_tokens as f64 / 1_000_000.0) * m.output_cost_per_1m.unwrap();
    let total_cost = in_cost + out_cost;

    assert!((in_cost - 0.50).abs() < 1e-6);
    assert!((out_cost - 0.50).abs() < 1e-6);
    assert!((total_cost - 1.00).abs() < 1e-6);
}

#[test]
fn test_tier1_usage_latency_metrics() {
    let mut seat = RouteSeatMetrics::default();
    let now_ms = chrono::Utc::now().timestamp_millis();

    seat.record_usage(50, Some(120), now_ms);
    assert_eq!(seat.last_latency_ms, Some(120));

    seat.record_usage(80, Some(85), now_ms + 500);
    assert_eq!(seat.last_latency_ms, Some(85));
}

#[test]
fn test_tier1_usage_session_affinity_ttl() {
    let mut groups = HashMap::new();
    let mut group = RoutingGroup::new("aff_grp", "Affinity Group", RoutingStrategy::Rotate);
    group.sticky_ttl_ms = 5_000; // 5 seconds
    group.members = vec![
        RoutingGroupMember::new("p1/m1"),
        RoutingGroupMember::new("p2/m2"),
    ];
    groups.insert("aff_grp".to_string(), group);

    let mut metrics = RouteRuntimeMetrics::default();
    let now_ms = 1_000_000i64;

    // Manually register an affinity entry for session "sess_abc" to p2/m2 expiring at now_ms + 5_000
    metrics.session_affinity.insert(
        "sess_abc::group/aff_grp".to_string(),
        magpie_gateway::SessionAffinityEntry {
            target: "p2/m2".to_string(),
            expires_at_ms: now_ms + 5_000,
        },
    );

    // Call before expiry -> should pick pinned target p2/m2
    let targets_pinned = magpie_gateway::routing::resolve_group_targets(
        "group/aff_grp",
        &groups,
        &mut metrics,
        "default_prov",
        Some("sess_abc"),
        "Turn",
        1,
        0,
        now_ms + 1_000,
    )
    .expect("resolve pinned");
    assert_eq!(targets_pinned[0].provider_id, "p2");
    assert_eq!(targets_pinned[0].reason, "session_affinity");

    // Call after expiry (now_ms + 6_000) -> should ignore expired affinity and fallback to group strategy
    let targets_expired = magpie_gateway::routing::resolve_group_targets(
        "group/aff_grp",
        &groups,
        &mut metrics,
        "default_prov",
        Some("sess_abc"),
        "Turn",
        1,
        0,
        now_ms + 6_000,
    )
    .expect("resolve expired");
    assert_ne!(targets_expired[0].reason, "session_affinity");
}

#[tokio::test]
async fn test_tier1_usage_jsonl_ledger_persistence() {
    let (_dir, state, _router) = boot_test_server();
    let store = state.hermes.store();

    // Create session and record message turns
    let session = store
        .create_session(gpui_telegram_agent::hermes::state_db::CreateSessionRequest {
            id: None,
            chat_id: None,
            bot_id: None,
            source: Some("ledger_test".to_string()),
            model: Some("gpt-4o".to_string()),
            api_mode: Some("openai".to_string()),
            title: Some("Ledger Test Session".to_string()),
            parent_session_id: None,
            branch_from_seq: None,
            system_prompt_hash: None,
            active_toolsets: None,
            metadata: None,
        })
        .expect("create session");

    store
        .append_message(gpui_telegram_agent::hermes::state_db::AppendMessageRequest {
            id: None,
            session_id: session.id.clone(),
            role: "user".to_string(),
            content: "Count tokens".to_string(),
            reasoning: None,
            tool_calls_json: None,
            tool_call_id: None,
            tool_name: None,
            token_count: 30,
            created_at: None,
        })
        .expect("user message");

    store
        .append_message(gpui_telegram_agent::hermes::state_db::AppendMessageRequest {
            id: None,
            session_id: session.id.clone(),
            role: "assistant".to_string(),
            content: "Tokens counted".to_string(),
            reasoning: None,
            tool_calls_json: None,
            tool_call_id: None,
            tool_name: None,
            token_count: 20,
            created_at: None,
        })
        .expect("assistant message");

    store
        .record_token_usage(&session.id, 30, 20, 0.0025)
        .expect("record token usage");

    let summary = store
        .usage_summary(Some(&session.id), None)
        .expect("usage summary");
    assert_eq!(summary.total_tokens, 50);
    assert_eq!(summary.total_input_tokens, 30);
    assert_eq!(summary.total_output_tokens, 20);
    assert!(summary.total_cost_usd >= 0.002);
}
