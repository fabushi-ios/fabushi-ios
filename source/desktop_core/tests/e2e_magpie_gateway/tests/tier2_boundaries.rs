#[path = "common/mod.rs"]
mod common;

use common::*;
use axum::http::StatusCode;
use gpui_telegram_agent::hermes::providers::{CredentialEntry, CredentialPool, ProviderRouter};
use magpie_gateway::{
    discover_subscriptions_in_home, parse_vertex_adc_credentials, scan_vertex_adc,
    RouteRuntimeMetrics, RouteSeatMetrics, RoutingGroup, RoutingGroupMember, RoutingStrategy,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

// ============================================================================
// Tier 2: Boundary & Corner Cases (8 Tests)
// ============================================================================

#[test]
fn test_tier2_missing_credential_files() {
    let empty_temp = new_temp_dir("missing");

    // Scanner against empty directory must return None/empty collection without panic
    let adc_res = scan_vertex_adc(empty_temp.path(), None);
    assert!(adc_res.is_none(), "expected None for missing vertex adc");

    let reg = discover_subscriptions_in_home(empty_temp.path(), None);
    assert!(
        reg.accounts.is_empty(),
        "expected empty registry for clean directory"
    );
    assert!(!reg.has_active_subscription());
}

#[test]
fn test_tier2_malformed_json_and_toml() {
    let temp = new_temp_dir("malformed");

    // Corrupted ADC JSON
    let gcloud_dir = temp.path().join(".config").join("gcloud");
    std::fs::create_dir_all(&gcloud_dir).unwrap();
    std::fs::write(
        gcloud_dir.join("application_default_credentials.json"),
        "{ \"client_id\": \"unclosed_string...",
    )
    .unwrap();

    // Corrupted Codex auth.json & config.toml
    let codex_dir = temp.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    std::fs::write(codex_dir.join("auth.json"), "{{{{corrupted_json").unwrap();
    std::fs::write(codex_dir.join("config.toml"), "[unclosed toml section\nkey =").unwrap();

    // Corrupted Claude settings.json
    let claude_dir = temp.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(claude_dir.join("settings.json"), "NOT_JSON_AT_ALL").unwrap();

    // Corrupted Gemini token
    let gemini_dir = temp.path().join(".gemini");
    std::fs::create_dir_all(&gemini_dir).unwrap();
    std::fs::write(gemini_dir.join("jetski-standalone-oauth-token"), "").unwrap();

    // Direct parser verification
    assert!(parse_vertex_adc_credentials("malformed json").is_none());

    // Directory scanner must gracefully skip corrupted files and return empty registry
    let reg = discover_subscriptions_in_home(temp.path(), None);
    assert!(
        reg.accounts.is_empty(),
        "malformed files should be skipped cleanly: {:?}",
        reg.accounts
    );
}

#[test]
fn test_tier2_expired_oauth_token_no_refresh() {
    let temp = new_temp_dir("expired");

    // Create an expired JWT token (expired 2 hours ago)
    let expired_jwt = create_mock_jwt("expired-user@example.com", "plus", "acct_exp_123", -7200);

    let codex_dir = temp.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let auth_file = codex_dir.join("auth.json");
    let val = json!({
        "tokens": {
            "access_token": expired_jwt,
            "account_id": "acct_exp_123"
        },
        "email": "expired-user@example.com",
        "plan": "plus"
    });
    std::fs::write(&auth_file, val.to_string()).unwrap();

    let reg = discover_subscriptions_in_home(temp.path(), None);
    let acc = reg
        .accounts
        .iter()
        .find(|a| a.email.as_deref() == Some("expired-user@example.com"))
        .expect("expired account present");
    // Expired token with no refresh token MUST be marked inactive
    assert!(!acc.active, "expired token without refresh must be inactive");
}

#[tokio::test]
async fn test_tier2_empty_and_whitespace_configs() {
    let (dir, state, router) = boot_test_server();
    isolate_empty_providers(&state, dir.path());
    let (_uid, tok) = register_user(&router, "tier2_validation_user").await;

    // 1. Empty preset_id and provider_id
    let (st_empty, val_empty) = http_json(
        &router,
        "POST",
        "/api/v1/hermes/magpie/providers",
        Some(&tok),
        Some(json!({
            "preset_id": "   ",
            "api_key": "sk-test"
        })),
    )
    .await;
    assert_ne!(
        st_empty,
        StatusCode::OK,
        "empty preset_id must be rejected: {val_empty}"
    );

    // 2. Whitespace-only group ID
    let (st_grp, val_grp) = http_json(
        &router,
        "POST",
        "/api/v1/hermes/magpie/groups",
        Some(&tok),
        Some(json!({
            "id": "   ",
            "name": "   ",
            "strategy": "rotate"
        })),
    )
    .await;
    assert_ne!(
        st_grp,
        StatusCode::OK,
        "whitespace group id must be rejected: {val_grp}"
    );
}

#[tokio::test]
async fn test_tier2_upstream_timeout_and_hang() {
    let mut seat = RouteSeatMetrics::default();
    let now_ms = 1_000_000i64;

    // Initial state: 0 failures, not in cooldown
    assert_eq!(seat.consecutive_failures, 0);
    assert!(!seat.is_in_cooldown(now_ms));

    // Simulate 504 Gateway Timeout
    seat.record_failure(Some(504), None, now_ms);
    assert_eq!(seat.consecutive_failures, 1);
    assert_eq!(seat.last_status, Some(504));
    assert!(seat.is_in_cooldown(now_ms));
    assert!(seat.cooldown_until_ms > now_ms);

    // Wire-level verification: real HTTP upstream returning 504 Gateway Timeout
    let (openai_url, _gemini_url, mock_prov_state) = spawn_mock_providers().await;
    mock_prov_state.set_force_status(StatusCode::GATEWAY_TIMEOUT, None);

    let (dir, state, router) = boot_test_server();
    isolate_empty_providers(&state, dir.path());
    let (_uid, tok) = register_user(&router, "tier2_timeout_user").await;

    let (st_save, _) = http_json(
        &router,
        "POST",
        "/api/v1/hermes/magpie/providers",
        Some(&tok),
        Some(json!({
            "preset_id": "deepseek",
            "api_key": "sk-timeout-key",
            "base_url": openai_url,
            "default_model": "deepseek-chat",
            "activate": true
        })),
    )
    .await;
    assert_eq!(st_save, StatusCode::OK);

    let (st_chat, val_chat) = http_json(
        &router,
        "POST",
        "/api/v1/magpie/v1/chat/completions",
        Some(&tok),
        Some(json!({
            "model": "deepseek-chat",
            "messages": [{"role": "user", "content": "Ping timeout"}],
            "stream": false
        })),
    )
    .await;
    assert_ne!(st_chat, StatusCode::OK, "504 upstream must fail: {val_chat}");
    assert_eq!(mock_prov_state.call_count.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Verify wire-level 504 registered in router runtime circuit-breaker metrics
    let metrics = state.hermes.providers().route_metrics_snapshot();
    let seat = metrics
        .seats
        .get("deepseek/deepseek-chat")
        .expect("seat recorded after 504");
    assert_eq!(seat.consecutive_failures, 1);
    assert_eq!(seat.last_status, Some(504));
    let now = chrono::Utc::now().timestamp_millis();
    assert!(seat.is_in_cooldown(now));
    assert!(seat.cooldown_until_ms > now);
}

#[tokio::test]
async fn test_tier2_upstream_circuit_breaker_ladders() {
    let now_ms = 1_000_000i64;

    // 1. RouteSeatMetrics 429 ladder (exponential: 5s, 10s, 20s...)
    let mut seat_429 = RouteSeatMetrics::default();
    seat_429.record_failure(Some(429), None, now_ms);
    assert_eq!(seat_429.consecutive_failures, 1);
    assert_eq!(seat_429.cooldown_until_ms - now_ms, 5_000);

    seat_429.record_failure(Some(429), None, now_ms);
    assert_eq!(seat_429.consecutive_failures, 2);
    assert_eq!(seat_429.cooldown_until_ms - now_ms, 10_000);

    seat_429.record_failure(Some(429), None, now_ms);
    assert_eq!(seat_429.consecutive_failures, 3);
    assert_eq!(seat_429.cooldown_until_ms - now_ms, 20_000);

    // 2. 500 server error ladder (exponential: 3s, 6s, 12s...)
    let mut seat_500 = RouteSeatMetrics::default();
    seat_500.record_failure(Some(500), None, now_ms);
    assert_eq!(seat_500.cooldown_until_ms - now_ms, 3_000);

    seat_500.record_failure(Some(500), None, now_ms);
    assert_eq!(seat_500.cooldown_until_ms - now_ms, 6_000);

    // 3. 402 payment required ladder (fixed 300s = 300,000ms)
    let mut seat_402 = RouteSeatMetrics::default();
    seat_402.record_failure(Some(402), None, now_ms);
    assert_eq!(seat_402.cooldown_until_ms - now_ms, 300_000);

    // 4. CredentialPool circuit breaker with explicit retry-after
    let mut pool = CredentialPool::new();
    pool.add_entry(CredentialEntry::new("cred_1", "sk-key-1", "primary"));
    assert_eq!(pool.active_count(now_ms), 1);
    assert_eq!(pool.cooldown_count(now_ms), 0);

    pool.report_rate_limit("cred_1", now_ms, 429, Some(15));
    assert_eq!(pool.active_count(now_ms), 0);
    assert_eq!(pool.cooldown_count(now_ms), 1);
    assert!(pool.acquire(now_ms).is_none());

    // After retry_after expires (now_ms + 16_000), credential becomes available again
    assert_eq!(pool.active_count(now_ms + 16_000), 1);
    assert!(pool.acquire(now_ms + 16_000).is_some());

    // 5. Wire-level verification: real HTTP upstream returning 429 Too Many Requests with Retry-After: 10
    let (openai_url, _gemini_url, mock_prov_state) = spawn_mock_providers().await;
    mock_prov_state.set_force_status(StatusCode::TOO_MANY_REQUESTS, Some(10));

    let (dir, state, router) = boot_test_server();
    isolate_empty_providers(&state, dir.path());
    let (_uid, tok) = register_user(&router, "tier2_cb_user").await;

    let (st_save, _) = http_json(
        &router,
        "POST",
        "/api/v1/hermes/magpie/providers",
        Some(&tok),
        Some(json!({
            "preset_id": "deepseek",
            "api_key": "sk-cb-ladder-key",
            "base_url": openai_url,
            "default_model": "deepseek-chat",
            "activate": true
        })),
    )
    .await;
    assert_eq!(st_save, StatusCode::OK);

    let (st_chat, val_chat) = http_json(
        &router,
        "POST",
        "/api/v1/magpie/v1/chat/completions",
        Some(&tok),
        Some(json!({
            "model": "deepseek-chat",
            "messages": [{"role": "user", "content": "Trigger 429 wire"}],
            "stream": false
        })),
    )
    .await;
    assert!(
        st_chat == StatusCode::TOO_MANY_REQUESTS || st_chat == StatusCode::BAD_GATEWAY,
        "expected 429 or 502 upstream error: {val_chat}"
    );
    assert!(
        val_chat.to_string().contains("429"),
        "error message must cite 429: {val_chat}"
    );
    assert_eq!(mock_prov_state.call_count.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Verify wire-level 429 registered in router runtime circuit-breaker metrics
    let metrics = state.hermes.providers().route_metrics_snapshot();
    let seat = metrics
        .seats
        .get("deepseek/deepseek-chat")
        .expect("seat recorded after 429");
    assert_eq!(seat.consecutive_failures, 1);
    assert_eq!(seat.last_status, Some(429));
    let now = chrono::Utc::now().timestamp_millis();
    assert!(seat.is_in_cooldown(now));
    // 10s cooldown from Retry-After header
    assert!(seat.cooldown_until_ms >= now + 8_000);
}

#[tokio::test]
async fn test_tier2_unauthorized_proxy_access() {
    let (dir, state, router) = boot_test_server();
    isolate_empty_providers(&state, dir.path());

    // Request with missing Authorization header to protected endpoint
    let (st_no_auth, _) = http_json(
        &router,
        "GET",
        "/api/v1/hermes/magpie/state",
        None,
        None,
    )
    .await;
    assert_eq!(st_no_auth, StatusCode::UNAUTHORIZED);

    // Request with invalid / forged Bearer token
    let (st_bad_auth, _) = http_json(
        &router,
        "GET",
        "/api/v1/hermes/magpie/state",
        Some("forged_invalid_token_999"),
        None,
    )
    .await;
    assert_eq!(st_bad_auth, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_tier2_concurrent_discovery_race() {
    let router_arc = Arc::new(ProviderRouter::new());
    let mut handles = Vec::new();

    // Spawn 20 concurrent tasks scanning and resolving group targets
    for i in 0..20 {
        let router = router_arc.clone();
        let handle = tokio::spawn(async move {
            let temp = new_temp_dir(&format!("race_{i}"));
            create_mock_vertex_adc_dir(
                temp.path(),
                &format!("client_{i}"),
                "sec",
                "rt",
                Some(&format!("proj_{i}")),
                None,
            );

            // Concurrent scan
            let reg = router.refresh_local_subscriptions_in_home(temp.path(), None);
            assert!(reg.has_active_subscription());

            // Concurrent metrics update
            let mut groups = HashMap::new();
            let mut grp = RoutingGroup::new("race_grp", "Race", RoutingStrategy::Rotate);
            grp.members = vec![
                RoutingGroupMember::new("p1/m1"),
                RoutingGroupMember::new("p2/m2"),
            ];
            groups.insert("race_grp".to_string(), grp);

            let mut metrics = RouteRuntimeMetrics::default();
            let now_ms = chrono::Utc::now().timestamp_millis();
            let targets = magpie_gateway::routing::resolve_group_targets(
                "group/race_grp",
                &groups,
                &mut metrics,
                "p1",
                None,
                "Hello race",
                1,
                0,
                now_ms,
            )
            .expect("resolve targets");
            assert!(!targets.is_empty());
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.expect("task join");
    }

    assert!(router_arc.has_active_subscription());
}
