#[path = "common/mod.rs"]
mod common;

use axum::{
    body::Body,
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use common::*;
use futures::{stream, StreamExt};
use gpui_telegram_agent::hermes::providers::{
    ApiMode, CodexTokenManager, ProviderChatMessage, ProviderConfig, ProviderRequest,
    ProviderRouter, ProviderStreamEvent, VertexTokenManager,
};
use gpui_telegram_server::search::schema::{SearchDocument, SearchQueryParams};
use gpui_telegram_server::search::tantivy_indexer::TantivyIndexer;
use magpie_gateway::{
    parse_magpie_providers, parse_retry_after_header, GatewayServer, GatewayState, MaskingAction,
    MaskingConfig, MaskingEngine, ProtocolKind, RoutingGroup, RoutingGroupMember, RoutingStrategy,
    TokenRefreshError, VertexAdcCredentials,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

// ============================================================================
// Tier 5: Adversarial Hardening & Defect Remediation Regression Suite
// ============================================================================

#[tokio::test]
async fn test_tier5_gateway_server_real_upstream_streaming_and_failover() {
    // 1. Real upstream OpenAI-compatible SSE server for GatewayServer stream=true pass-through
    let sse_app = Router::new().route(
        "/v1/chat/completions",
        post(|Json(payload): Json<Value>| async move {
            assert_eq!(payload["stream"].as_bool(), Some(true));
            let chunks = vec![
                Ok::<_, Infallible>(axum::body::Bytes::from(
                    "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"REAL_SSE_\"}}]}\n\n",
                )),
                Ok::<_, Infallible>(axum::body::Bytes::from(
                    "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"STREAM_OK\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":7,\"total_tokens\":12}}\n\n",
                )),
                Ok::<_, Infallible>(axum::body::Bytes::from("data: [DONE]\n\n")),
            ];
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
                .body(Body::from_stream(stream::iter(chunks)))
                .unwrap()
        }),
    );
    let sse_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sse_addr = sse_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(sse_listener, sse_app).await;
    });

    // 2. Failover upstreams: p1 returns 429 with Retry-After: 60; p2 succeeds
    let p1_calls = Arc::new(AtomicUsize::new(0));
    let p2_calls = Arc::new(AtomicUsize::new(0));
    let p1_c = Arc::clone(&p1_calls);
    let p2_c = Arc::clone(&p2_calls);

    let p1_app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let c = Arc::clone(&p1_c);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    [("retry-after", "60")],
                    Json(json!({ "error": { "message": "quota exceeded" } })),
                )
            }
        }),
    );
    let p1_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p1_addr = p1_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(p1_listener, p1_app).await;
    });

    let p2_app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let c = Arc::clone(&p2_c);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Json(json!({
                    "id": "cmpl_p2",
                    "object": "chat.completion",
                    "model": "m2",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "FAILOVER_P2_OK" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 4, "completion_tokens": 6, "total_tokens": 10 }
                }))
            }
        }),
    );
    let p2_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p2_addr = p2_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(p2_listener, p2_app).await;
    });

    let mut groups = HashMap::new();
    let mut grp = RoutingGroup::new("mix", "Mix", RoutingStrategy::Order);
    grp.members = vec![
        RoutingGroupMember::new("p1/m1"),
        RoutingGroupMember::new("p2/m2"),
    ];
    groups.insert("mix".to_string(), grp);

    let gw_state = GatewayState::new("mock_sse")
        .with_groups(groups)
        .with_provider_endpoint(
            "mock_sse",
            format!("http://{sse_addr}/v1"),
            ProtocolKind::ChatCompletions,
            None,
        )
        .with_provider_endpoint(
            "p1",
            format!("http://{p1_addr}/v1"),
            ProtocolKind::ChatCompletions,
            None,
        )
        .with_provider_endpoint(
            "p2",
            format!("http://{p2_addr}/v1"),
            ProtocolKind::ChatCompletions,
            None,
        );

    let gw_server = GatewayServer::bind_loopback(gw_state, 0).await.unwrap();
    let gw_addr = gw_server.local_addr().unwrap();
    let handle = gw_server.run_background();
    let client = reqwest::Client::new();

    // Test real SSE stream pass-through
    let sse_resp = client
        .post(format!("http://{gw_addr}/v1/chat/completions"))
        .json(&json!({
            "model": "mock_sse/qwen3:8b",
            "messages": [{ "role": "user", "content": "Stream hello" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(sse_resp.status(), StatusCode::OK);
    let ct = sse_resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(ct.contains("text/event-stream"), "ct={ct}");
    let sse_body = sse_resp.text().await.unwrap();
    assert!(sse_body.contains("REAL_SSE_"), "body={sse_body}");
    assert!(sse_body.contains("STREAM_OK"), "body={sse_body}");
    assert!(sse_body.contains("[DONE]"), "body={sse_body}");

    // Test candidate failover across RoutingGroup on 429 + real status preservation in quotas
    let fail_resp = client
        .post(format!("http://{gw_addr}/v1/chat/completions"))
        .json(&json!({
            "model": "group/mix",
            "messages": [{ "role": "user", "content": "Failover please" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(fail_resp.status(), StatusCode::OK);
    let fail_json: Value = fail_resp.json().await.unwrap();
    assert_eq!(
        fail_json["choices"][0]["message"]["content"].as_str(),
        Some("FAILOVER_P2_OK")
    );
    assert_eq!(p1_calls.load(Ordering::SeqCst), 1);
    assert_eq!(p2_calls.load(Ordering::SeqCst), 1);

    let quotas: Value = client
        .get(format!("http://{gw_addr}/v1/magpie/quotas"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(quotas["rests"]["p1/m1"]["status"].as_u64(), Some(429));

    handle.shutdown().await;
}

#[tokio::test]
async fn test_tier5_gateway_isolated_masking_google_patterns_and_oauth_redaction() {
    // 1. Verify MaskingEngine detects 1//, ya29., and GOCSPX-
    let engine = MaskingEngine::new(MaskingConfig {
        enabled: true,
        ..Default::default()
    });
    let r_rt = engine.mask_text("refresh_token=1//04gK9xMockRefreshToken1234567890abcdef");
    assert_eq!(r_rt.hits.len(), 1);
    assert!(!r_rt.masked_text.contains("1//04gK9x"));

    let r_at = engine.mask_text("token=ya29.a0AfH6SMMockAccessToken1234567890abcdef");
    assert_eq!(r_at.hits.len(), 1);
    assert!(!r_at.masked_text.contains("ya29.a0AfH6SM"));

    let r_cs = engine.mask_text("client_secret=GOCSPX-MockClientSecret1234567890");
    assert_eq!(r_cs.hits.len(), 1);
    assert!(!r_cs.masked_text.contains("GOCSPX-MockClientSecret"));

    // 2. Verify TokenRefreshError redacts echoed secrets in OAuth error JSON bodies
    let oauth_app = Router::new().route(
        "/token",
        post(|| async move {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "invalid_grant",
                    "echo_secret": "GOCSPX-secret-echoed-by-oauth",
                    "echo_rt": "1//04echoed-rt-secret-value"
                })),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, oauth_app).await;
    });

    let mgr = VertexTokenManager::new(VertexAdcCredentials {
        client_id: "cid".to_string(),
        client_secret: "GOCSPX-secret-echoed-by-oauth".to_string(),
        refresh_token: "1//04echoed-rt-secret-value".to_string(),
        quota_project_id: None,
        account: None,
    })
    .with_token_url(format!("http://{addr}/token"));

    let err = mgr
        .get_or_refresh_token(&reqwest::Client::new(), 1_000_000)
        .await
        .expect_err("expected oauth error");
    let err_str = err.to_string();
    assert!(!err_str.contains("GOCSPX-secret-echoed-by-oauth"), "leaked secret: {err_str}");
    assert!(!err_str.contains("1//04echoed-rt-secret-value"), "leaked rt: {err_str}");
    assert!(matches!(err, TokenRefreshError::OAuth { .. }));

    // 3. Cross-tenant vault isolation in GatewayState (even when initialized via .with_masking)
    let captured_placeholder = Arc::new(Mutex::new(String::new()));
    let cap_clone = Arc::clone(&captured_placeholder);
    let gw_state = GatewayState::new("mock")
        .with_masking(Arc::new(MaskingEngine::new(MaskingConfig {
            enabled: true,
            ..Default::default()
        })))
        .with_custom_handler(Arc::new(move |req, _sess| {
            let cap = Arc::clone(&cap_clone);
            Box::pin(async move {
                let txt = req.messages.last().map(|m| m.content.clone()).unwrap_or_default();
                if txt.contains("{{API_KEY_") {
                    *cap.lock().unwrap() = txt.clone();
                }
                Ok(magpie_gateway::UnifiedResponse {
                    provider_id: "mock".to_string(),
                    model: req.model,
                    content: format!("Echoed: {txt}"),
                    reasoning: None,
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    usage: magpie_gateway::UnifiedUsage::default(),
                })
            })
        }));

    let gw_server = GatewayServer::bind_loopback(gw_state, 0).await.unwrap();
    let gw_addr = gw_server.local_addr().unwrap();
    let handle = gw_server.run_background();
    let client = reqwest::Client::new();

    // Tenant A sends secret
    let resp_a: Value = client
        .post(format!("http://{gw_addr}/v1/chat/completions"))
        .json(&json!({
            "model": "mock/m",
            "messages": [{ "role": "user", "content": "My key is sk-tenantA-supersecret-99999999" }]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(resp_a["choices"][0]["message"]["content"]
        .as_str()
        .unwrap()
        .contains("sk-tenantA-supersecret-99999999"));

    let placeholder_seen = captured_placeholder
        .lock()
        .unwrap()
        .replace("My key is ", "");
    assert!(placeholder_seen.starts_with("{{API_KEY_"));

    // Tenant B replays Tenant A's placeholder -> MUST NOT unmask Tenant A's secret!
    let resp_b: Value = client
        .post(format!("http://{gw_addr}/v1/chat/completions"))
        .json(&json!({
            "model": "mock/m",
            "messages": [{ "role": "user", "content": format!("Repeat this: {placeholder_seen}") }]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let reply_b = resp_b["choices"][0]["message"]["content"].as_str().unwrap();
    assert!(
        !reply_b.contains("sk-tenantA-supersecret-99999999"),
        "cross-tenant secret leaked to Tenant B: {reply_b}"
    );
    assert!(reply_b.contains(&placeholder_seen));

    handle.shutdown().await;
}

#[tokio::test]
async fn test_tier5_exact_subscription_activation_and_atomic_concurrency() {
    let temp = new_temp_dir("tier5_sub_exact");
    let jwt1 = create_mock_jwt("user1@example.com", "plus", "acct_u1", 3600);
    let jwt2 = create_mock_jwt("beta@example.com", "team", "acct_beta", 3600);

    let logins = json!([
        {
            "id": "user1",
            "agent": "codex",
            "user": "user1@example.com",
            "plan": "plus",
            "on": true,
            "auth": { "tokens": { "access_token": jwt1, "account_id": "acct_u1" } }
        },
        {
            "id": "beta",
            "agent": "codex",
            "user": "beta@example.com",
            "plan": "team",
            "on": true,
            "auth": { "tokens": { "access_token": jwt2, "account_id": "acct_beta" } }
        }
    ]);
    create_mock_magpie_dir(temp.path(), json!({ "providers": [] }), logins);

    let (_dir, state, router) = boot_test_server();
    state
        .hermes
        .providers()
        .refresh_local_subscriptions_in_home(temp.path(), None);

    // 1. Exact ID matching: activating magpie_codex:user10 MUST return Err/404, NOT match user1!
    let err_res = state
        .hermes
        .providers()
        .activate_subscription("magpie_codex:user10", None);
    assert!(
        err_res.is_err(),
        "magpie_codex:user10 must NOT prefix-match magpie_codex:user1"
    );

    let (_uid, tok) = register_user(&router, "tier5_sub_user").await;
    let (st_404, _) = http_json(
        &router,
        "POST",
        "/api/v1/hermes/magpie/subscriptions/magpie_codex:user10/activate",
        Some(&tok),
        Some(json!({})),
    )
    .await;
    assert_eq!(st_404, StatusCode::NOT_FOUND);

    // 2. Concurrent activation race test: 24 tasks x 4 rounds alternating (user1, model_A) and (beta, model_B)
    let mut handles = Vec::new();
    for task_idx in 0..24 {
        let r = router.clone();
        let t = tok.clone();
        handles.push(tokio::spawn(async move {
            for round in 0..4 {
                let is_a = (task_idx + round) % 2 == 0;
                let (sub_id, expected_model) = if is_a {
                    ("magpie_codex:user1", "model_FOR_USER1")
                } else {
                    ("magpie_codex:beta", "model_FOR_BETA")
                };
                let (st, val) = http_json(
                    &r,
                    "POST",
                    &format!("/api/v1/hermes/magpie/subscriptions/{sub_id}/activate"),
                    Some(&t),
                    Some(json!({ "model": expected_model })),
                )
                .await;
                assert_eq!(st, StatusCode::OK, "activate failed: {val}");
                let got_pid = val["active_provider_id"].as_str().unwrap();
                let got_mdl = val["active_model"].as_str().unwrap();
                assert_eq!(got_pid, sub_id);
                assert_eq!(
                    got_mdl, expected_model,
                    "torn activation response: pid={got_pid}, mdl={got_mdl}"
                );
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
}

#[tokio::test]
async fn test_tier5_keyless_vertex_429_cooldown_skip_and_http_date_retry_after() {
    // 1. RFC 7231 HTTP-date Retry-After parsing
    let parsed_date = parse_retry_after_header("Wed, 21 Oct 2030 07:28:00 GMT");
    assert!(parsed_date.is_some_and(|s| s > 0), "parsed_date={parsed_date:?}");
    assert_eq!(parse_retry_after_header("45"), Some(45));

    // 2. Keyless GoogleVertex 429 cooldown skip in non-group routing
    let vertex_calls = Arc::new(AtomicUsize::new(0));
    let backup_calls = Arc::new(AtomicUsize::new(0));
    let vc = Arc::clone(&vertex_calls);
    let bc = Arc::clone(&backup_calls);

    let vertex_app = Router::new().route(
        "/v1/projects/{p}/locations/{l}/publishers/google/models/{m}",
        post(move || {
            let c = Arc::clone(&vc);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    [("retry-after", "Wed, 21 Oct 2030 07:28:00 GMT")],
                    Json(json!({ "error": { "message": "Rate limited" } })),
                )
            }
        }),
    );
    let v_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let v_addr = v_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(v_listener, vertex_app).await;
    });

    let backup_app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let c = Arc::clone(&bc);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Json(json!({
                    "id": "cmpl_backup",
                    "object": "chat.completion",
                    "model": "gpt-4o",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "BACKUP_OK" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 2, "completion_tokens": 2, "total_tokens": 4 }
                }))
            }
        }),
    );
    let b_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let b_addr = b_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(b_listener, backup_app).await;
    });

    let (oauth_url, _oauth_state) = spawn_mock_oauth().await;
    let router = ProviderRouter::new();
    router.set_allow_local_fallback(false);

    let mut v_headers = HashMap::new();
    v_headers.insert("x-goog-user-project".to_string(), "proj-1".to_string());
    router.register_provider(ProviderConfig {
        id: "google_vertex:adc".to_string(),
        name: "Vertex ADC".to_string(),
        api_mode: ApiMode::GoogleVertex,
        base_url: format!("http://{v_addr}/v1"),
        default_model: "gemini-2.5-flash".to_string(),
        headers: v_headers,
        credentials: Default::default(),
        fallback_models: Vec::new(),
        timeout_secs: 15,
        enabled: true,
        preset_id: Some("google-vertex".to_string()),
        region_id: Some("global".to_string()),
        is_subscription: true,
        no_key_required: true,
        routing_strategy: None,
    });
    router.register_vertex_token_manager(
        "google_vertex:adc",
        VertexTokenManager::new(VertexAdcCredentials {
            client_id: "cid".to_string(),
            client_secret: "csec".to_string(),
            refresh_token: "1//rt".to_string(),
            quota_project_id: Some("proj-1".to_string()),
            account: None,
        })
        .with_token_url(oauth_url),
    );
    router.register_provider(ProviderConfig {
        id: "backup".to_string(),
        name: "Backup".to_string(),
        api_mode: ApiMode::ChatCompletions,
        base_url: format!("http://{b_addr}/v1"),
        default_model: "gpt-4o".to_string(),
        headers: HashMap::new(),
        credentials: gpui_telegram_agent::hermes::providers::CredentialPool::with_single_key(
            "bk_1",
            "sk-backup",
        ),
        fallback_models: Vec::new(),
        timeout_secs: 15,
        enabled: true,
        preset_id: None,
        region_id: None,
        is_subscription: false,
        no_key_required: false,
        routing_strategy: None,
    });
    router.set_active("google_vertex:adc", Some("gemini-2.5-flash")).unwrap();
    router.set_fallback_chain(vec![gpui_telegram_agent::hermes::providers::FallbackTarget {
        provider_id: "backup".to_string(),
        model: "gpt-4o".to_string(),
    }]);

    // 3 consecutive turns: turn 1 hits Vertex (gets 429 + HTTP-date Retry-After), falls back to backup;
    // turns 2 and 3 MUST skip cooled Vertex and hit backup directly!
    for _ in 0..3 {
        let mut req = ProviderRequest::new("", vec![ProviderChatMessage::user("hi")]);
        req.stream = false;
        let resp = router.complete(req, None).await.expect("complete ok");
        assert_eq!(resp.content, "BACKUP_OK");
    }
    assert_eq!(vertex_calls.load(Ordering::SeqCst), 1, "cooled keyless Vertex must not be re-hit");
    assert_eq!(backup_calls.load(Ordering::SeqCst), 3);

    let metrics = router.route_metrics_snapshot();
    assert!(
        !metrics.circuit_breaker.rests.is_empty(),
        "ProviderRouter::record_route_failure must populate CircuitBreaker.rests"
    );
    let seat = metrics.seats.get("google_vertex:adc/gemini-2.5-flash").unwrap();
    assert!(seat.cooldown_until_ms > chrono::Utc::now().timestamp_millis() + 60_000);
}

#[tokio::test]
async fn test_tier5_vertex_401_invalidation_refresh_and_fabushi_bot_propagation() {
    // 1. OAuth server returns tok_1 on call 1, tok_2 on call 2
    let oauth_calls = Arc::new(AtomicUsize::new(0));
    let oc = Arc::clone(&oauth_calls);
    let oauth_app = Router::new().route(
        "/token",
        post(move || {
            let c = Arc::clone(&oc);
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst) + 1;
                Json(json!({
                    "access_token": format!("ya29.tok_{n}"),
                    "expires_in": 3600,
                    "token_type": "Bearer"
                }))
            }
        }),
    );
    let oauth_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let oauth_addr = oauth_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(oauth_listener, oauth_app).await;
    });

    // 2. Vertex mock rejects tok_1 with 401 and accepts tok_2 with SSE reply
    let vertex_calls = Arc::new(AtomicUsize::new(0));
    let vc = Arc::clone(&vertex_calls);
    let vertex_app = Router::new().route(
        "/v1/projects/{p}/locations/{l}/publishers/google/models/{m}",
        post(move |headers: HeaderMap| {
            let c = Arc::clone(&vc);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                let auth = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                if auth == "Bearer ya29.tok_1" {
                    return (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({ "error": { "message": "Token revoked" } })),
                    )
                        .into_response();
                }
                let body = format!(
                    "data: {}\n\n",
                    json!({
                        "candidates": [{
                            "content": { "role": "model", "parts": [{ "text": format!("VERTEX_BOT_OK auth={auth}") }] },
                            "finishReason": "STOP"
                        }],
                        "usageMetadata": { "promptTokenCount": 5, "candidatesTokenCount": 5, "totalTokenCount": 10 }
                    })
                );
                axum::response::Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(body))
                    .unwrap()
            }
        }),
    );
    let v_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let v_addr = v_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(v_listener, vertex_app).await;
    });

    // 3. Test VertexTokenManager propagation into FabushiBotInstance + 401 refresh retry
    let temp_home = new_temp_dir("tier5_vertex_bot_home");
    create_mock_vertex_adc_dir(
        temp_home.path(),
        "cid",
        "csec",
        "1//rt",
        Some("bot-proj"),
        Some("bot@vertex.test"),
    );

    let (_dir, state, app_router) = boot_test_server();
    isolate_empty_providers(&state, temp_home.path());
    state
        .hermes
        .providers()
        .refresh_local_subscriptions_in_home(temp_home.path(), None);

    // Override base_url on the discovered vertex provider to point at our mock Vertex server
    let mut v_cfg = state
        .hermes
        .providers()
        .snapshot()
        .providers
        .get("google_vertex:bot@vertex.test")
        .cloned()
        .expect("vertex provider discovered");
    v_cfg.base_url = format!("http://{v_addr}/v1");
    state.hermes.providers().register_provider(v_cfg);
    state.hermes.providers().register_vertex_token_manager(
        "google_vertex:bot@vertex.test",
        VertexTokenManager::new(VertexAdcCredentials {
            client_id: "cid".to_string(),
            client_secret: "csec".to_string(),
            refresh_token: "1//rt".to_string(),
            quota_project_id: Some("bot-proj".to_string()),
            account: Some("bot@vertex.test".to_string()),
        })
        .with_token_url(format!("http://{oauth_addr}/token")),
    );
    state
        .hermes
        .providers()
        .activate_subscription("google_vertex:bot@vertex.test", None)
        .unwrap();
    state.hermes.persist_providers_state();
    state.fabushi_bots.sync_all_providers();

    let (_uid, tok) = register_user(&app_router, "tier5_vertex_bot_user").await;
    let (st_bot, bot_val) = http_json(
        &app_router,
        "POST",
        "/api/v1/bots",
        Some(&tok),
        Some(json!({
            "display_name": "Vertex Bot",
            "username": "vertex_t5_bot",
            "description": "Vertex bot test"
        })),
    )
    .await;
    assert_eq!(st_bot, StatusCode::CREATED);
    let cid = bot_val["companion_chat_id"].as_i64().unwrap();

    let (st_msg, _) = http_json(
        &app_router,
        "POST",
        &format!("/api/v1/chats/{cid}/messages"),
        Some(&tok),
        Some(json!({ "text": "Hello Vertex bot" })),
    )
    .await;
    assert!(st_msg.is_success());

    let mut found_reply = false;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (_st, hist) = http_json(
            &app_router,
            "GET",
            &format!("/api/v1/chats/{cid}/messages?limit=20"),
            Some(&tok),
            None,
        )
        .await;
        if let Some(msgs) = hist["messages"].as_array() {
            for m in msgs {
                let txt = m["text"].as_str().unwrap_or("");
                assert!(!txt.contains("PROVIDER_ERROR"), "unexpected PROVIDER_ERROR: {txt}");
                if txt.contains("VERTEX_BOT_OK auth=Bearer ya29.tok_2") {
                    found_reply = true;
                }
            }
        }
        if found_reply {
            break;
        }
    }
    assert!(found_reply, "FabushiBot must share VertexTokenManager and recover from 401 via refresh");
    assert_eq!(oauth_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_tier5_codex_expired_jwt_refresh_and_midstream_fallback_isolation() {
    // 1. Codex expired JWT + refresh_token -> preemptive refresh via CodexTokenManager
    let oauth_app = Router::new().route(
        "/oauth/token",
        post(|| async move {
            Json(json!({
                "access_token": "fresh_codex_token_2026",
                "expires_in": 3600,
                "token_type": "Bearer"
            }))
        }),
    );
    let oa_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let oa_addr = oa_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(oa_listener, oauth_app).await;
    });

    let codex_app = Router::new().route(
        "/responses",
        post(|headers: HeaderMap| async move {
            let auth = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if auth != "Bearer fresh_codex_token_2026" {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({ "error": { "message": "Token expired" } })),
                )
                    .into_response();
            }
            Json(json!({
                "id": "resp_1",
                "output_text": "CODEX_REFRESHED_OK",
                "usage": { "input_tokens": 3, "output_tokens": 3, "total_tokens": 6 }
            }))
            .into_response()
        }),
    );
    let cx_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cx_addr = cx_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(cx_listener, codex_app).await;
    });

    let temp_home = new_temp_dir("tier5_codex_refresh");
    let expired_jwt = create_mock_jwt("codex@example.com", "plus", "acct_cx", -3600);
    let codex_dir = temp_home.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    std::fs::write(
        codex_dir.join("auth.json"),
        json!({
            "tokens": {
                "access_token": expired_jwt,
                "refresh_token": "rt_codex_valid",
                "account_id": "acct_cx"
            }
        })
        .to_string(),
    )
    .unwrap();

    let router = ProviderRouter::new();
    router.set_allow_local_fallback(false);
    router.refresh_local_subscriptions_in_home(temp_home.path(), Some(&codex_dir));
    let mut cx_cfg = router
        .snapshot()
        .providers
        .get("codex:codex@example.com")
        .cloned()
        .unwrap();
    cx_cfg.base_url = format!("http://{cx_addr}");
    router.register_provider(cx_cfg);
    router.register_codex_token_manager(
        "codex:codex@example.com",
        CodexTokenManager::new("rt_codex_valid").with_token_url(format!("http://{oa_addr}/oauth/token")),
    );
    router
        .activate_subscription("codex:codex@example.com", None)
        .unwrap();

    let mut req = ProviderRequest::new("", vec![ProviderChatMessage::user("hi codex")]);
    req.stream = false;
    let cx_resp = router.complete(req, None).await.expect("codex refresh ok");
    assert_eq!(cx_resp.content, "CODEX_REFRESHED_OK");

    // 2. Mid-stream SSE error fallback must NOT concatenate broken prefix onto fallback stream
    let p1_app = Router::new().route(
        "/v1/chat/completions",
        post(|| async move {
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"BAD_PREFIX_\"}}]}\n\n",
                "data: {\"error\":{\"message\":\"upstream mid-stream reset\"}}\n\n"
            );
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        }),
    );
    let p1_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p1_addr = p1_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(p1_listener, p1_app).await;
    });

    let p2_app = Router::new().route(
        "/v1/chat/completions",
        post(|| async move {
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"CLEAN_REPLY\"},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            );
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        }),
    );
    let p2_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p2_addr = p2_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(p2_listener, p2_app).await;
    });

    let stream_router = ProviderRouter::new();
    stream_router.set_allow_local_fallback(false);
    stream_router.register_provider(ProviderConfig {
        id: "p1".to_string(),
        name: "P1".to_string(),
        api_mode: ApiMode::ChatCompletions,
        base_url: format!("http://{p1_addr}/v1"),
        default_model: "m1".to_string(),
        headers: HashMap::new(),
        credentials: gpui_telegram_agent::hermes::providers::CredentialPool::with_single_key("k1", "sk-1"),
        fallback_models: Vec::new(),
        timeout_secs: 15,
        enabled: true,
        preset_id: None,
        region_id: None,
        is_subscription: false,
        no_key_required: false,
        routing_strategy: None,
    });
    stream_router.register_provider(ProviderConfig {
        id: "p2".to_string(),
        name: "P2".to_string(),
        api_mode: ApiMode::ChatCompletions,
        base_url: format!("http://{p2_addr}/v1"),
        default_model: "m2".to_string(),
        headers: HashMap::new(),
        credentials: gpui_telegram_agent::hermes::providers::CredentialPool::with_single_key("k2", "sk-2"),
        fallback_models: Vec::new(),
        timeout_secs: 15,
        enabled: true,
        preset_id: None,
        region_id: None,
        is_subscription: false,
        no_key_required: false,
        routing_strategy: None,
    });
    stream_router.set_active("p1", Some("m1")).unwrap();
    stream_router.set_fallback_chain(vec![gpui_telegram_agent::hermes::providers::FallbackTarget {
        provider_id: "p2".to_string(),
        model: "m2".to_string(),
    }]);

    let (tx, mut rx) = mpsc::channel::<ProviderStreamEvent>(32);
    let mut sreq = ProviderRequest::new("m1", vec![ProviderChatMessage::user("stream")]);
    sreq.stream = true;
    let sres = stream_router.complete(sreq, Some(tx)).await;
    let mut streamed = String::new();
    while let Some(ev) = rx.recv().await {
        if let ProviderStreamEvent::TextDelta(t) = ev {
            streamed.push_str(&t);
        }
    }
    assert!(
        sres.is_err(),
        "once partial output was emitted on stream_tx, mid-stream failure must abort cleanly instead of concatenating fallback: {sres:?}"
    );
    assert_eq!(
        streamed, "BAD_PREFIX_",
        "fallback CLEAN_REPLY must NEVER be concatenated onto BAD_PREFIX_: {streamed}"
    );

    // 2b. When p1 errors BEFORE emitting any delta (e.g. error event on first SSE frame), fallback to p2 succeeds cleanly
    let p1_early_err = Router::new().route(
        "/v1/chat/completions",
        post(|| async move {
            let body = "data: {\"error\":{\"message\":\"upstream early stream error\"}}\n\n";
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        }),
    );
    let p1e_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p1e_addr = p1e_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(p1e_listener, p1_early_err).await;
    });
    stream_router.register_provider(ProviderConfig {
        id: "p1_early".to_string(),
        name: "P1Early".to_string(),
        api_mode: ApiMode::ChatCompletions,
        base_url: format!("http://{p1e_addr}/v1"),
        default_model: "m1".to_string(),
        headers: HashMap::new(),
        credentials: gpui_telegram_agent::hermes::providers::CredentialPool::with_single_key("k1e", "sk-1e"),
        fallback_models: Vec::new(),
        timeout_secs: 15,
        enabled: true,
        preset_id: None,
        region_id: None,
        is_subscription: false,
        no_key_required: false,
        routing_strategy: None,
    });
    stream_router.set_active("p1_early", Some("m1")).unwrap();
    stream_router.set_fallback_chain(vec![gpui_telegram_agent::hermes::providers::FallbackTarget {
        provider_id: "p2".to_string(),
        model: "m2".to_string(),
    }]);

    let (tx2, mut rx2) = mpsc::channel::<ProviderStreamEvent>(32);
    let mut sreq2 = ProviderRequest::new("m1", vec![ProviderChatMessage::user("stream")]);
    sreq2.stream = true;
    let sresp2 = stream_router.complete(sreq2, Some(tx2)).await.expect("early stream err fallback ok");
    let mut streamed2 = String::new();
    while let Some(ev) = rx2.recv().await {
        if let ProviderStreamEvent::TextDelta(t) = ev {
            streamed2.push_str(&t);
        }
    }
    assert_eq!(sresp2.content, "CLEAN_REPLY");
    assert_eq!(streamed2, "CLEAN_REPLY");
}

#[tokio::test]
async fn test_tier5_gateway_sse_formatting_and_rapid_client_disconnect_abort() {
    // 1. Verify ThinkingDelta, ToolCallReady, Usage, role=assistant, and actual finish_reason
    let rich_app = Router::new().route(
        "/v1/chat/completions",
        post(|| async move {
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"THINKING_STEP\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"VISIBLE_TEXT\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_t1\",\"function\":{\"name\":\"search\",\"arguments\":\"{}\"}}]}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":20,\"total_tokens\":30}}\n\n",
                "data: [DONE]\n\n"
            );
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        }),
    );
    let rich_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rich_addr = rich_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(rich_listener, rich_app).await;
    });

    let (_dir, state, app_router) = boot_test_server();
    isolate_empty_providers(&state, _dir.path());
    let (_uid, tok) = register_user(&app_router, "tier5_sse_user").await;

    let (st_save, _) = http_json(
        &app_router,
        "POST",
        "/api/v1/hermes/magpie/providers",
        Some(&tok),
        Some(json!({
            "preset_id": "deepseek",
            "api_key": "sk-rich-sse",
            "base_url": format!("http://{rich_addr}/v1"),
            "default_model": "deepseek-chat",
            "activate": true
        })),
    )
    .await;
    assert_eq!(st_save, StatusCode::OK);

    let (st_sse, ct, bytes) = http_raw(
        &app_router,
        "POST",
        "/api/v1/magpie/v1/chat/completions",
        Some(&tok),
        Some(json!({
            "model": "deepseek-chat",
            "messages": [{ "role": "user", "content": "rich sse" }],
            "stream": true
        })),
    )
    .await;
    assert_eq!(st_sse, StatusCode::OK);
    assert!(ct.contains("text/event-stream"));
    let sse_str = String::from_utf8_lossy(&bytes);
    assert!(sse_str.contains("\"role\":\"assistant\""), "missing role: {sse_str}");
    assert!(sse_str.contains("THINKING_STEP"), "missing thinking: {sse_str}");
    assert!(sse_str.contains("VISIBLE_TEXT"), "missing text: {sse_str}");
    assert!(sse_str.contains("call_t1"), "missing tool call: {sse_str}");
    assert!(sse_str.contains("\"finish_reason\":\"length\""), "missing finish_reason length: {sse_str}");
    assert!(sse_str.contains("\"total_tokens\":30"), "missing usage: {sse_str}");

    // 2. Rapid client disconnect abort: slow upstream emitting 20 chunks @ 50ms
    let chunks_sent = Arc::new(AtomicUsize::new(0));
    let cs = Arc::clone(&chunks_sent);
    let slow_app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let c = Arc::clone(&cs);
            async move {
                let s = stream::unfold(0usize, move |i| {
                    let c2 = Arc::clone(&c);
                    async move {
                        if i >= 20 {
                            return None;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        c2.fetch_add(1, Ordering::SeqCst);
                        let line = format!(
                            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"chunk_{i} \"}}}}]}}\n\n"
                        );
                        Some((Ok::<_, Infallible>(axum::body::Bytes::from(line)), i + 1))
                    }
                });
                axum::response::Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(s))
                    .unwrap()
            }
        }),
    );
    let slow_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let slow_addr = slow_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(slow_listener, slow_app).await;
    });

    let _ = http_json(
        &app_router,
        "POST",
        "/api/v1/hermes/magpie/providers",
        Some(&tok),
        Some(json!({
            "preset_id": "deepseek",
            "api_key": "sk-slow-sse",
            "base_url": format!("http://{slow_addr}/v1"),
            "default_model": "deepseek-chat",
            "activate": true
        })),
    )
    .await;

    let srv_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let srv_addr = srv_listener.local_addr().unwrap();
    let srv_router = app_router.clone();
    let (sd_tx, sd_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(srv_listener, srv_router)
            .with_graceful_shutdown(async {
                let _ = sd_rx.await;
            })
            .await;
    });

    let http_client = reqwest::Client::new();
    for _ in 0..3 {
        let mut resp = http_client
            .post(format!("http://{srv_addr}/api/v1/magpie/v1/chat/completions"))
            .bearer_auth(&tok)
            .json(&json!({
                "model": "deepseek-chat",
                "messages": [{ "role": "user", "content": "disconnect" }],
                "stream": true
            }))
            .send()
            .await
            .unwrap();
        let _first_chunk = resp.chunk().await.unwrap();
        drop(resp); // Immediately disconnect!
    }

    tokio::time::sleep(Duration::from_millis(600)).await;
    let total_sent = chunks_sent.load(Ordering::SeqCst);
    assert!(
        total_sent < 30,
        "upstream drained too many chunks after disconnect: {total_sent}/60"
    );
    let _ = sd_tx.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_tier5_header_redaction_tantivy_drop_persistence_and_ws_close_frame() {
    // 1. Sensitive header redaction in MagpieProviderView
    let temp_dir = new_temp_dir("tier5_t5_combo");
    let (state, app_router) = boot_test_server_in(temp_dir.path());
    let (_uid, tok) = register_user(&app_router, "tier5_hdr_user").await;

    let (st_save, save_val) = http_json(
        &app_router,
        "POST",
        "/api/v1/hermes/magpie/providers",
        Some(&tok),
        Some(json!({
            "provider_id": "custom_hdr_prov",
            "name": "Custom Header Prov",
            "api_mode": "chat_completions",
            "base_url": "http://127.0.0.1:9999/v1",
            "default_model": "gpt-4o",
            "custom_headers": {
                "Authorization": "Bearer sk-raw-header-secret-999",
                "x-api-key": "raw-x-api-key-secret-888",
                "x-goog-api-key": "AIzaRawGoogleHeaderSecret777",
                "X-Custom-Safe": "public-value"
            },
            "activate": false
        })),
    )
    .await;
    assert_eq!(st_save, StatusCode::OK, "save: {save_val}");
    let save_str = save_val.to_string();
    assert!(!save_str.contains("sk-raw-header-secret-999"), "Authorization leaked: {save_str}");
    assert!(!save_str.contains("raw-x-api-key-secret-888"), "x-api-key leaked: {save_str}");
    assert!(!save_str.contains("AIzaRawGoogleHeaderSecret777"), "x-goog-api-key leaked: {save_str}");
    assert!(save_str.contains("public-value"));

    let (st_list, list_val) = http_json(
        &app_router,
        "GET",
        "/api/v1/hermes/magpie/providers",
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st_list, StatusCode::OK);
    let list_str = list_val.to_string();
    assert!(!list_str.contains("sk-raw-header-secret-999"));
    assert!(!list_str.contains("raw-x-api-key-secret-888"));
    assert!(!list_str.contains("AIzaRawGoogleHeaderSecret777"));

    // 2. Tantivy uncommitted documents persistence across clean TantivyIndexer drop
    let tantivy_dir = temp_dir.path().join("tantivy_drop_test");
    {
        let idx = TantivyIndexer::open_or_create(&tantivy_dir).expect("open tantivy");
        for i in 0..5 {
            idx.index_message(SearchDocument {
                message_id: format!("m_{i}"),
                chat_id: "100".to_string(),
                sender_id: "1".to_string(),
                date: 1_700_000_000 + i,
                content: format!("uncommitted_tantivy_token_{i}"),
                media_type: None,
            })
            .await
            .unwrap();
        }
        // Drop WITHOUT calling idx.commit()!
    }
    {
        let idx2 = TantivyIndexer::open_or_create(&tantivy_dir).expect("reopen tantivy");
        let hits = idx2
            .search_hits(&SearchQueryParams {
                query: "uncommitted_tantivy_token_3".to_string(),
                chat_id: None,
                sender_id: None,
                media_type: None,
                from_date: None,
                to_date: None,
                limit: 10,
            })
            .unwrap();
        assert_eq!(hits.len(), 1, "expected uncommitted doc to be flushed on Drop");
    }

    // 3. WebSocket Close frame on state.shutdown()
    let ws_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_addr = ws_listener.local_addr().unwrap();
    let srv_router = app_router.clone();
    let (sd_tx, sd_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(ws_listener, srv_router)
            .with_graceful_shutdown(async {
                let _ = sd_rx.await;
            })
            .await;
    });

    let ws_url = format!("ws://{ws_addr}/api/v1/ws?token={tok}");
    let (mut ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("connect ws");

    state.shutdown();

    let close_msg = tokio::time::timeout(Duration::from_secs(2), ws_stream.next())
        .await
        .expect("timeout waiting for WS Close frame");
    assert!(
        matches!(close_msg, Some(Ok(WsMessage::Close(_)))),
        "expected clean WebSocket Close frame on shutdown, got: {close_msg:?}"
    );
    let _ = sd_tx.send(());
}

static TIER5_ENV_MUTEX: std::sync::LazyLock<Mutex<()>> =
    std::sync::LazyLock::new(|| Mutex::new(()));

struct EnvVarGuard {
    saved: Vec<(String, Option<std::ffi::OsString>)>,
}

impl EnvVarGuard {
    fn new() -> Self {
        Self { saved: Vec::new() }
    }

    fn set(&mut self, key: &str, val: impl AsRef<std::ffi::OsStr>) {
        if !self.saved.iter().any(|(k, _)| k == key) {
            self.saved.push((key.to_string(), std::env::var_os(key)));
        }
        unsafe {
            std::env::set_var(key, val);
        }
    }

    fn remove(&mut self, key: &str) {
        if !self.saved.iter().any(|(k, _)| k == key) {
            self.saved.push((key.to_string(), std::env::var_os(key)));
        }
        unsafe {
            std::env::remove_var(key);
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        for (key, old) in self.saved.iter().rev() {
            unsafe {
                match old {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

#[tokio::test]
async fn test_tier5_magpie_codex_logins_json_refresh_and_isolation() {
    // 1. Unit check: CodexTokenManager::from_logins_json_str matches exact login_id and skips gemini
    let logins_arr = json!([
        {
            "id": "mcx1",
            "agent": "gemini",
            "auth": { "refresh_token": "rt_wrong_gemini" }
        },
        {
            "id": "mcx1",
            "agent": "codex",
            "user": "mcx1@example.com",
            "plan": "plus",
            "on": true,
            "auth": {
                "tokens": {
                    "access_token": create_mock_jwt("mcx1@example.com", "plus", "acct_mcx1", -3600),
                    "refresh_token": "rt_logins_mcx1",
                    "account_id": "acct_mcx1"
                }
            }
        },
        {
            "id": "mcx2",
            "agent": "codex",
            "user": "mcx2@example.com",
            "plan": "team",
            "on": true,
            "auth": {
                "tokens": {
                    "access_token": create_mock_jwt("mcx2@example.com", "team", "acct_mcx2", -3600),
                    "refresh_token": "rt_logins_mcx2",
                    "account_id": "acct_mcx2"
                }
            }
        },
        {
            "id": "mcx_no_rt",
            "agent": "codex",
            "user": "nort@example.com",
            "on": true,
            "auth": {
                "tokens": {
                    "access_token": create_mock_jwt("nort@example.com", "plus", "acct_nort", -3600)
                }
            }
        }
    ]);
    let raw_logins = logins_arr.to_string();
    let mgr1 = CodexTokenManager::from_logins_json_str(&raw_logins, "mcx1").expect("mcx1 parsed");
    assert_eq!(mgr1.refresh_token(), "rt_logins_mcx1");
    let mgr2 = CodexTokenManager::from_logins_json_str(&raw_logins, "mcx2").expect("mcx2 parsed");
    assert_eq!(mgr2.refresh_token(), "rt_logins_mcx2");
    assert!(CodexTokenManager::from_logins_json_str(&raw_logins, "mcx_no_rt").is_none());
    assert!(CodexTokenManager::from_logins_json_str(&raw_logins, "nonexistent").is_none());

    // 2. End-to-end isolated refresh without manual register_codex_token_manager + decoy ~/.codex/auth.json
    let seen_rts = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen_clone = Arc::clone(&seen_rts);
    let oauth_app = Router::new().route(
        "/oauth/token",
        post(move |body: String| {
            let s = Arc::clone(&seen_clone);
            async move {
                let mut rt = String::new();
                for pair in body.split('&') {
                    if let Some((k, v)) = pair.split_once('=') {
                        if k == "refresh_token" {
                            rt = v.to_string();
                        }
                    }
                }
                s.lock().unwrap().push(rt.clone());
                if rt != "rt_logins_mcx1" {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": "wrong_refresh_token" })),
                    )
                        .into_response();
                }
                Json(json!({
                    "access_token": "fresh_mcx1_oauth_tok",
                    "expires_in": 3600,
                    "token_type": "Bearer"
                }))
                .into_response()
            }
        }),
    );
    let oa_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let oa_addr = oa_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(oa_listener, oauth_app).await;
    });

    let codex_app = Router::new().route(
        "/responses",
        post(|headers: HeaderMap| async move {
            let auth = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if auth != "Bearer fresh_mcx1_oauth_tok" {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({ "error": { "message": "Token expired" } })),
                )
                    .into_response();
            }
            Json(json!({
                "id": "resp_mcx1",
                "output_text": "MCX1_LOGINS_REFRESH_OK",
                "usage": { "input_tokens": 3, "output_tokens": 4, "total_tokens": 7 }
            }))
            .into_response()
        }),
    );
    let cx_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cx_addr = cx_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(cx_listener, codex_app).await;
    });

    let temp_home = new_temp_dir("tier5_logins_codex");
    create_mock_magpie_dir(temp_home.path(), json!({ "providers": [] }), logins_arr);
    // Decoy ~/.codex/auth.json for a completely different account
    let decoy_codex = temp_home.path().join("decoy_codex");
    std::fs::create_dir_all(&decoy_codex).unwrap();
    std::fs::write(
        decoy_codex.join("auth.json"),
        json!({
            "tokens": {
                "access_token": create_mock_jwt("decoy@example.com", "pro", "acct_decoy", 3600),
                "refresh_token": "rt_decoy_must_never_be_used",
                "account_id": "acct_decoy"
            }
        })
        .to_string(),
    )
    .unwrap();

    let _env_lock = TIER5_ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let mut env_guard = EnvVarGuard::new();
    env_guard.set("HOME", temp_home.path());
    env_guard.set("CODEX_HOME", &decoy_codex);

    let router = ProviderRouter::new();
    router.set_allow_local_fallback(false);
    router.refresh_local_subscriptions_in_home(temp_home.path(), Some(&decoy_codex));

    let mut mcx1_cfg = router
        .snapshot()
        .providers
        .get("magpie_codex:mcx1")
        .cloned()
        .expect("magpie_codex:mcx1 discovered");
    mcx1_cfg.base_url = format!("http://{cx_addr}");
    mcx1_cfg.headers.insert(
        "x-codex-token-url".to_string(),
        format!("http://{oa_addr}/oauth/token"),
    );
    router.register_provider(mcx1_cfg);
    router
        .activate_subscription("magpie_codex:mcx1", None)
        .unwrap();

    let mut req = ProviderRequest::new("", vec![ProviderChatMessage::user("hello mcx1")]);
    req.stream = false;
    let resp = router
        .complete(req, None)
        .await
        .expect("magpie_codex:mcx1 should refresh from logins.json");
    assert_eq!(resp.content, "MCX1_LOGINS_REFRESH_OK");
    assert_eq!(seen_rts.lock().unwrap().as_slice(), &["rt_logins_mcx1"]);

    // Verify magpie_codex:mcx_no_rt does NOT fall back to decoy CODEX_HOME/auth.json
    let no_rt_cfg = router
        .snapshot()
        .providers
        .get("magpie_codex:mcx_no_rt")
        .cloned()
        .expect("mcx_no_rt in providers");
    assert!(
        router
            .get_or_create_codex_manager("magpie_codex:mcx_no_rt", &no_rt_cfg)
            .is_none(),
        "magpie_codex:<id> must NEVER fall back to ~/.codex/auth.json"
    );
}

#[tokio::test]
async fn test_tier5_gateway_server_midstream_sse_error_and_chunk_failure() {
    // 1. Upstream sends partial chunk followed by SSE data: {"error": ...} and trailing [DONE]
    let err_sse_app = Router::new().route(
        "/v1/chat/completions",
        post(|| async move {
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"PARTIAL_CHUNK_\"}}]}\n\n",
                "data: {\"error\":{\"message\":\"upstream_sse_error_event\"}}\n\n",
                "data: [DONE]\n\n"
            );
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        }),
    );
    let err_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let err_addr = err_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(err_listener, err_sse_app).await;
    });

    // 2. Upstream returns 200 OK SSE with 1 valid chunk, then corrupts/drops TCP mid-chunk so resp.chunk() returns Err
    let drop_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let drop_addr = drop_listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut sock, _)) = drop_listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let chunk1 = "data: {\"choices\":[{\"delta\":{\"content\":\"BEFORE_DROP_\"}}]}\n\n";
                let resp_head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{chunk1}\r\n",
                    chunk1.len()
                );
                let _ = sock.write_all(resp_head.as_bytes()).await;
                let _ = sock.flush().await;
                tokio::time::sleep(Duration::from_millis(20)).await;
                // Write truncated chunk header/body (claims 16 bytes, sends 4, then closes without 0\r\n\r\n)
                let _ = sock.write_all(b"10\r\ndata").await;
                let _ = sock.flush().await;
                drop(sock);
            });
        }
    });

    let gw_state = GatewayState::new("err_sse")
        .with_provider_endpoint(
            "err_sse",
            format!("http://{err_addr}/v1"),
            ProtocolKind::ChatCompletions,
            None,
        )
        .with_provider_endpoint(
            "drop_sse",
            format!("http://{drop_addr}/v1"),
            ProtocolKind::ChatCompletions,
            None,
        );

    let gw_server = GatewayServer::bind_loopback(gw_state, 0).await.unwrap();
    let gw_addr = gw_server.local_addr().unwrap();
    let handle = gw_server.run_background();
    let client = reqwest::Client::new();

    // Case A: Mid-stream SSE error event
    let resp_a = client
        .post(format!("http://{gw_addr}/v1/chat/completions"))
        .json(&json!({
            "model": "err_sse/m1",
            "messages": [{ "role": "user", "content": "hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_a.status(), StatusCode::OK);
    let body_a = resp_a.text().await.unwrap();
    assert!(body_a.contains("PARTIAL_CHUNK_"), "body_a={body_a}");
    assert!(
        body_a.contains("upstream_sse_error_event"),
        "expected SSE error event in body_a={body_a}"
    );
    assert!(
        !body_a.contains("[DONE]"),
        "must NOT emit [DONE] after mid-stream SSE error: {body_a}"
    );

    // Case B: Mid-stream chunk read Err
    let resp_b = client
        .post(format!("http://{gw_addr}/v1/chat/completions"))
        .json(&json!({
            "model": "drop_sse/m2",
            "messages": [{ "role": "user", "content": "hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_b.status(), StatusCode::OK);
    let body_b = resp_b.text().await.unwrap();
    assert!(body_b.contains("BEFORE_DROP_"), "body_b={body_b}");
    assert!(
        body_b.contains("\"error\"") && body_b.contains("Upstream SSE stream read failed"),
        "expected SSE error event on chunk failure in body_b={body_b}"
    );
    assert!(
        !body_b.contains("[DONE]"),
        "must NOT emit [DONE] after chunk read error: {body_b}"
    );

    // Verify usage ledger did NOT record successful completions and quotas recorded failures
    let usage: Value = client
        .get(format!("http://{gw_addr}/v1/magpie/usage"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        usage["summary"]["total_records"].as_u64(),
        Some(0),
        "failed SSE streams must not record successful usage: {usage}"
    );

    let quotas: Value = client
        .get(format!("http://{gw_addr}/v1/magpie/quotas"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        quotas["rests"]["err_sse/m1"]["failures"].as_u64().is_some_and(|n| n >= 1),
        "expected failure recorded for err_sse/m1: {quotas}"
    );
    assert!(
        quotas["rests"]["drop_sse/m2"]["failures"].as_u64().is_some_and(|n| n >= 1),
        "expected failure recorded for drop_sse/m2: {quotas}"
    );

    handle.shutdown().await;
}

#[tokio::test]
async fn test_tier5_subscription_account_header_redaction_in_state_and_scan() {
    let temp_home = new_temp_dir("tier5_sub_hdr_redact");
    let codex_dir = temp_home.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let jwt = create_mock_jwt("codex_hdr@test.com", "plus", "acct_hdr", 3600);
    std::fs::write(
        codex_dir.join("auth.json"),
        json!({
            "tokens": {
                "access_token": jwt,
                "refresh_token": "rt_hdr_secret",
                "account_id": "acct_hdr"
            }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        codex_dir.join("config.toml"),
        concat!(
            "model = \"codex-mini-latest\"\n",
            "[model_providers.magpie]\n",
            "base_url = \"http://127.0.0.1:9999/v1\"\n",
            "[model_providers.magpie.http_headers]\n",
            "Authorization = \"Bearer sk-codex-toml-secret-leak-999\"\n",
            "x-api-key = \"sk-codex-toml-xapikey-888\"\n",
            "x-goog-api-key = \"AIzaCodexGoogleSecret777\"\n",
            "Cookie = \"session=codex-cookie-secret-666\"\n",
            "X-Auth-Token = \"tok-codex-secret-555\"\n",
            "X-Access-Key = \"ak-secret-123\"\n",
            "X-Auth = \"auth-secret-456\"\n",
            "X-Public-Header = \"public-safe-value\"\n",
        ),
    )
    .unwrap();

    // 1. Direct SubscriptionAccount serialization redacts sensitive headers while preserving in-memory values
    let reg = magpie_gateway::discover_subscriptions_in_home(temp_home.path(), None);
    let acc = reg
        .accounts
        .iter()
        .find(|a| a.id == "codex:codex_hdr@test.com")
        .expect("codex account discovered");
    assert_eq!(
        acc.headers.get("Authorization").map(String::as_str),
        Some("Bearer sk-codex-toml-secret-leak-999"),
        "in-memory headers must remain intact for execution"
    );
    let serialized = serde_json::to_value(acc).unwrap();
    let serialized_str = serialized.to_string();
    for secret in [
        "sk-codex-toml-secret-leak-999",
        "sk-codex-toml-xapikey-888",
        "AIzaCodexGoogleSecret777",
        "codex-cookie-secret-666",
        "tok-codex-secret-555",
        "ak-secret-123",
        "auth-secret-456",
    ] {
        assert!(
            !serialized_str.contains(secret),
            "SubscriptionAccount serialized leaked {secret}: {serialized_str}"
        );
    }
    assert_eq!(
        serialized["headers"]["Authorization"].as_str(),
        Some("Bearer ***")
    );
    assert_eq!(serialized["headers"]["x-api-key"].as_str(), Some("***"));
    assert_eq!(serialized["headers"]["X-Access-Key"].as_str(), Some("***"));
    assert_eq!(serialized["headers"]["X-Auth"].as_str(), Some("***"));
    assert_eq!(
        serialized["headers"]["X-Public-Header"].as_str(),
        Some("public-safe-value")
    );

    // 2. Server /api/v1/hermes/magpie/state and /api/v1/hermes/magpie/subscriptions/scan
    let (_dir, state, app_router) = boot_test_server();
    isolate_empty_providers(&state, temp_home.path());
    state
        .hermes
        .providers()
        .refresh_local_subscriptions_in_home(temp_home.path(), None);
    let (_uid, tok) = register_user(&app_router, "tier5_sub_hdr_user").await;

    let (st_state, state_val) = http_json(
        &app_router,
        "GET",
        "/api/v1/hermes/magpie/state",
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st_state, StatusCode::OK);
    let state_str = state_val.to_string();
    for secret in [
        "sk-codex-toml-secret-leak-999",
        "sk-codex-toml-xapikey-888",
        "AIzaCodexGoogleSecret777",
        "codex-cookie-secret-666",
        "tok-codex-secret-555",
        "ak-secret-123",
        "auth-secret-456",
    ] {
        assert!(
            !state_str.contains(secret),
            "/magpie/state leaked {secret}: {state_str}"
        );
    }
    assert!(state_str.contains("public-safe-value"));

    let (st_scan, scan_val) = {
        let _env_lock = TIER5_ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let mut env_guard = EnvVarGuard::new();
        env_guard.set("HOME", temp_home.path());
        env_guard.remove("CODEX_HOME");
        env_guard.set("HERMES_FORCE_LIVE_PROVIDERS", "1");
        http_json(
            &app_router,
            "POST",
            "/api/v1/hermes/magpie/subscriptions/scan",
            Some(&tok),
            None,
        )
        .await
    };
    assert_eq!(st_scan, StatusCode::OK);
    let scan_str = scan_val.to_string();
    for secret in [
        "sk-codex-toml-secret-leak-999",
        "sk-codex-toml-xapikey-888",
        "AIzaCodexGoogleSecret777",
        "codex-cookie-secret-666",
        "tok-codex-secret-555",
        "ak-secret-123",
        "auth-secret-456",
    ] {
        assert!(
            !scan_str.contains(secret),
            "/subscriptions/scan leaked {secret}: {scan_str}"
        );
    }
    assert!(scan_str.contains("public-safe-value"));
}

#[tokio::test]
async fn test_tier5_vertex_token_manager_env_and_default_adc_disk_staleness() {
    let temp_dir = new_temp_dir("tier5_vertex_env_stale");
    let adc_t1 = temp_dir.path().join("adc_t1.json");
    let adc_t2 = temp_dir.path().join("adc_t2.json");

    std::fs::write(
        &adc_t1,
        json!({
            "client_id": "cid_env",
            "client_secret": "sec_env_1",
            "refresh_token": "rt_env_t1",
            "quota_project_id": "proj_env_1"
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        &adc_t2,
        json!({
            "client_id": "cid_env",
            "client_secret": "sec_env_2",
            "refresh_token": "rt_env_t2",
            "quota_project_id": "proj_env_2"
        })
        .to_string(),
    )
    .unwrap();

    let _env_lock = TIER5_ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let mut env_guard = EnvVarGuard::new();
    env_guard.set("GOOGLE_APPLICATION_CREDENTIALS", &adc_t1);

    let router = ProviderRouter::new();
    let cfg = router
        .apply_magpie_preset(gpui_telegram_agent::hermes::providers::MagpiePresetApplyInput::new(
            "google-vertex",
        ))
        .expect("apply google-vertex preset");
    assert!(
        !cfg.headers.contains_key("x-adc-source-path"),
        "preset apply has no x-adc-source-path"
    );

    let mgr1 = router
        .get_or_create_vertex_manager("google-vertex", &cfg)
        .expect("mgr1 from GOOGLE_APPLICATION_CREDENTIALS");
    assert_eq!(mgr1.credentials().refresh_token, "rt_env_t1");
    mgr1.set_cached_token(magpie_gateway::CachedVertexToken::new(
        "ya29.env_t1".to_string(),
        3600,
        chrono::Utc::now().timestamp_millis(),
    ));

    // Switch GOOGLE_APPLICATION_CREDENTIALS to tenant 2
    env_guard.set("GOOGLE_APPLICATION_CREDENTIALS", &adc_t2);
    let mgr2 = router
        .get_or_create_vertex_manager("google-vertex", &cfg)
        .expect("mgr2 after env switch");
    assert_eq!(mgr2.credentials().refresh_token, "rt_env_t2");
    assert_eq!(mgr2.credentials().client_secret, "sec_env_2");
    assert!(
        mgr2.cached_token().is_none(),
        "stale cached token must be evicted when GOOGLE_APPLICATION_CREDENTIALS changes"
    );

    // Populate token on mgr2, then rotate only client_secret in adc_t2 and re-register provider
    mgr2.set_cached_token(magpie_gateway::CachedVertexToken::new(
        "ya29.env_t2".to_string(),
        3600,
        chrono::Utc::now().timestamp_millis(),
    ));
    std::fs::write(
        &adc_t2,
        json!({
            "client_id": "cid_env",
            "client_secret": "sec_env_2_rotated",
            "refresh_token": "rt_env_t2",
            "quota_project_id": "proj_env_2"
        })
        .to_string(),
    )
    .unwrap();
    router.register_provider(cfg.clone());
    let mgr2_rot = router
        .get_or_create_vertex_manager("google-vertex", &cfg)
        .expect("mgr2_rot");
    assert_eq!(mgr2_rot.credentials().client_secret, "sec_env_2_rotated");
    assert!(mgr2_rot.cached_token().is_none());

    // Also verify default ~/.config/gcloud/application_default_credentials.json when env var is unset
    env_guard.remove("GOOGLE_APPLICATION_CREDENTIALS");
    env_guard.set("HOME", temp_dir.path());
    let default_adc = create_mock_vertex_adc_dir(
        temp_dir.path(),
        "cid_def",
        "sec_def_1",
        "rt_def_1",
        Some("proj_def_1"),
        None,
    );
    let cfg_def = ProviderConfig::google_vertex(
        "vertex_default_env_test",
        "Vertex Default",
        "https://aiplatform.googleapis.com/v1",
        "gemini-2.5-flash",
        None,
    );
    router.register_provider(cfg_def.clone());
    let mgr_def1 = router
        .get_or_create_vertex_manager("vertex_default_env_test", &cfg_def)
        .expect("mgr_def1");
    assert_eq!(mgr_def1.credentials().refresh_token, "rt_def_1");
    mgr_def1.set_cached_token(magpie_gateway::CachedVertexToken::new(
        "ya29.def_1".to_string(),
        3600,
        chrono::Utc::now().timestamp_millis(),
    ));

    std::fs::write(
        &default_adc,
        json!({
            "client_id": "cid_def",
            "client_secret": "sec_def_2",
            "refresh_token": "rt_def_2",
            "quota_project_id": "proj_def_2"
        })
        .to_string(),
    )
    .unwrap();
    let mgr_def2 = router
        .get_or_create_vertex_manager("vertex_default_env_test", &cfg_def)
        .expect("mgr_def2");
    assert_eq!(mgr_def2.credentials().refresh_token, "rt_def_2");
    assert!(mgr_def2.cached_token().is_none());
}

#[tokio::test]
async fn test_tier5_responses_and_messages_masking_and_block() {
    let seen_upstream = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen_clone = Arc::clone(&seen_upstream);

    let gw_state = GatewayState::new("mock")
        .with_masking(Arc::new(MaskingEngine::new(MaskingConfig {
            enabled: true,
            ..Default::default()
        })))
        .with_custom_handler(Arc::new(move |req, _sess| {
            let cap = Arc::clone(&seen_clone);
            Box::pin(async move {
                let mut parts = Vec::new();
                if !req.system_prompt.is_empty() {
                    parts.push(req.system_prompt.clone());
                }
                for m in &req.messages {
                    if !m.content.is_empty() {
                        parts.push(m.content.clone());
                    }
                    for tc in &m.tool_calls {
                        if !tc.arguments.is_empty() {
                            parts.push(tc.arguments.clone());
                        }
                    }
                }
                let joined = parts.join(" | ");
                cap.lock().unwrap().push(joined.clone());
                Ok(magpie_gateway::UnifiedResponse {
                    provider_id: "mock".to_string(),
                    model: req.model,
                    content: format!("Echoed: {joined}"),
                    reasoning: None,
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    usage: magpie_gateway::UnifiedUsage::default(),
                })
            })
        }));

    let gw_server = GatewayServer::bind_loopback(gw_state, 0).await.unwrap();
    let gw_addr = gw_server.local_addr().unwrap();
    let handle = gw_server.run_background();
    let client = reqwest::Client::new();

    // 1. POST /v1/responses with secret in input text and function_call arguments
    let secret_resp_text = "sk-responses-text-secret-11111111";
    let secret_resp_tool = "sk-responses-tool-secret-22222222";
    let resp_r = client
        .post(format!("http://{gw_addr}/v1/responses"))
        .json(&json!({
            "model": "mock/m",
            "input": [
                { "role": "user", "content": format!("Use key {secret_resp_text}") },
                {
                    "type": "function_call",
                    "call_id": "call_r1",
                    "name": "auth_tool",
                    "arguments": json!({ "api_key": secret_resp_tool }).to_string()
                }
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_r.status(), StatusCode::OK);
    let resp_r_json: Value = resp_r.json().await.unwrap();
    let r_out = resp_r_json["output"][0]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        r_out.contains(secret_resp_text) && r_out.contains(secret_resp_tool),
        "client must receive unmasked secrets from /v1/responses: {resp_r_json}"
    );

    // 2. POST /v1/messages with secret in text and tool_use input
    let secret_msg_text = "sk-messages-text-secret-33333333";
    let secret_msg_tool = "sk-messages-tool-secret-44444444";
    let resp_m = client
        .post(format!("http://{gw_addr}/v1/messages"))
        .json(&json!({
            "model": "mock/m",
            "max_tokens": 64,
            "messages": [
                { "role": "user", "content": format!("Key is {secret_msg_text}") },
                {
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_m1",
                        "name": "auth_tool",
                        "input": { "token": secret_msg_tool }
                    }]
                }
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_m.status(), StatusCode::OK);
    let resp_m_json: Value = resp_m.json().await.unwrap();
    let m_out = resp_m_json["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        m_out.contains(secret_msg_text) && m_out.contains(secret_msg_tool),
        "client must receive unmasked secrets from /v1/messages: {resp_m_json}"
    );

    // Upstream must NEVER have seen raw secrets
    let captured = seen_upstream.lock().unwrap();
    assert_eq!(captured.len(), 2);
    for (idx, seen) in captured.iter().enumerate() {
        assert!(
            seen.contains("{{API_KEY_"),
            "upstream call {idx} missing {{API_KEY_ placeholder: {seen}"
        );
        for raw in [
            secret_resp_text,
            secret_resp_tool,
            secret_msg_text,
            secret_msg_tool,
        ] {
            assert!(
                !seen.contains(raw),
                "upstream call {idx} leaked raw secret {raw}: {seen}"
            );
        }
    }
    drop(captured);
    handle.shutdown().await;

    // 3. MaskingAction::Block returns 403 content_blocked on both /v1/responses and /v1/messages
    let mut block_cfg = MaskingConfig {
        enabled: true,
        ..Default::default()
    };
    block_cfg.default_action = MaskingAction::Block;
    let block_state = GatewayState::new("mock").with_masking(Arc::new(MaskingEngine::new(block_cfg)));
    let block_server = GatewayServer::bind_loopback(block_state, 0).await.unwrap();
    let block_addr = block_server.local_addr().unwrap();
    let block_handle = block_server.run_background();

    let blk_r = client
        .post(format!("http://{block_addr}/v1/responses"))
        .json(&json!({
            "model": "mock/m",
            "input": "sk-block-responses-secret-99999999"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(blk_r.status(), StatusCode::FORBIDDEN);
    let blk_r_json: Value = blk_r.json().await.unwrap();
    assert_eq!(blk_r_json["error"]["code"].as_str(), Some("content_blocked"));

    let blk_m = client
        .post(format!("http://{block_addr}/v1/messages"))
        .json(&json!({
            "model": "mock/m",
            "max_tokens": 32,
            "messages": [{ "role": "user", "content": "sk-block-messages-secret-99999999" }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(blk_m.status(), StatusCode::FORBIDDEN);
    let blk_m_json: Value = blk_m.json().await.unwrap();
    assert_eq!(blk_m_json["error"]["code"].as_str(), Some("content_blocked"));

    block_handle.shutdown().await;
}

#[tokio::test]
async fn test_tier5_gateway_truncated_sse_json_and_anthropic_keepalive() {
    // 1. Truncated JSON inside loop (with trailing newline)
    let trunc_nl_app = Router::new().route(
        "/v1/chat/completions",
        post(|| async move {
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"PARTIAL_NL_\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":\n\n"
            );
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        }),
    );
    let nl_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let nl_addr = nl_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(nl_listener, trunc_nl_app).await;
    });

    // 2. Truncated JSON at EOF without trailing newline
    let trunc_eof_app = Router::new().route(
        "/v1/chat/completions",
        post(|| async move {
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"PARTIAL_EOF_\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":"
            );
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        }),
    );
    let eof_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let eof_addr = eof_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(eof_listener, trunc_eof_app).await;
    });

    // 3. Anthropic upstream with SSE `: keep-alive` comment + `id:`/`retry:` control lines
    let anth_keepalive_app = Router::new().route(
        "/v1/messages",
        post(|| async move {
            let body = concat!(
                ": keep-alive\n\n",
                "id: evt_1\n",
                "retry: 1000\n",
                "event: content_block_delta\n",
                "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"ANTH_KEEPALIVE_OK\"}}\n\n",
                "event: message_stop\n",
                "data: {\"type\":\"message_stop\"}\n\n"
            );
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        }),
    );
    let anth_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let anth_addr = anth_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(anth_listener, anth_keepalive_app).await;
    });

    let gw_state = GatewayState::new("trunc_nl")
        .with_provider_endpoint(
            "trunc_nl",
            format!("http://{nl_addr}/v1"),
            ProtocolKind::ChatCompletions,
            None,
        )
        .with_provider_endpoint(
            "trunc_eof",
            format!("http://{eof_addr}/v1"),
            ProtocolKind::ChatCompletions,
            None,
        )
        .with_provider_endpoint(
            "anth_ka",
            format!("http://{anth_addr}/v1"),
            ProtocolKind::AnthropicMessages,
            None,
        );

    let gw_server = GatewayServer::bind_loopback(gw_state, 0).await.unwrap();
    let gw_addr = gw_server.local_addr().unwrap();
    let handle = gw_server.run_background();
    let client = reqwest::Client::new();

    // Case A: Truncated JSON with trailing newline
    let resp_nl = client
        .post(format!("http://{gw_addr}/v1/chat/completions"))
        .json(&json!({
            "model": "trunc_nl/m1",
            "messages": [{ "role": "user", "content": "hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_nl.status(), StatusCode::OK);
    let body_nl = resp_nl.text().await.unwrap();
    assert!(body_nl.contains("PARTIAL_NL_"), "body_nl={body_nl}");
    assert!(body_nl.contains("\"error\""), "expected error on truncated JSON: {body_nl}");
    assert!(!body_nl.contains("[DONE]"), "must NOT emit [DONE] on truncated JSON: {body_nl}");

    // Case B: Truncated JSON at EOF without trailing newline
    let resp_eof = client
        .post(format!("http://{gw_addr}/v1/chat/completions"))
        .json(&json!({
            "model": "trunc_eof/m2",
            "messages": [{ "role": "user", "content": "hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_eof.status(), StatusCode::OK);
    let body_eof = resp_eof.text().await.unwrap();
    assert!(body_eof.contains("PARTIAL_EOF_"), "body_eof={body_eof}");
    assert!(body_eof.contains("\"error\""), "expected error on EOF truncated JSON: {body_eof}");
    assert!(!body_eof.contains("[DONE]"), "must NOT emit [DONE] on EOF truncated JSON: {body_eof}");

    let usage_before: Value = client
        .get(format!("http://{gw_addr}/v1/magpie/usage"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(usage_before["summary"]["total_records"].as_u64(), Some(0));

    let quotas: Value = client
        .get(format!("http://{gw_addr}/v1/magpie/quotas"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(quotas["rests"]["trunc_nl/m1"]["failures"].as_u64().is_some_and(|n| n >= 1));
    assert!(quotas["rests"]["trunc_eof/m2"]["failures"].as_u64().is_some_and(|n| n >= 1));

    // Case C: Anthropic keep-alive + SSE control lines succeed cleanly
    let resp_ka = client
        .post(format!("http://{gw_addr}/v1/chat/completions"))
        .json(&json!({
            "model": "anth_ka/claude-3-5-sonnet",
            "messages": [{ "role": "user", "content": "hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_ka.status(), StatusCode::OK);
    let body_ka = resp_ka.text().await.unwrap();
    assert!(body_ka.contains("ANTH_KEEPALIVE_OK"), "body_ka={body_ka}");
    assert!(body_ka.contains("[DONE]"), "keep-alive stream should complete with [DONE]: {body_ka}");
    assert!(!body_ka.contains("\"error\""), "keep-alive must not trigger error: {body_ka}");

    handle.shutdown().await;
}

#[tokio::test]
async fn test_tier5_server_sse_midstream_error_no_done_and_send_as_auth_order() {
    // 1. Mock upstream returning partial delta + mid-stream SSE error + trailing [DONE]
    let upstream_app = Router::new().route(
        "/v1/chat/completions",
        post(|| async move {
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"SRV_PARTIAL_\"}}]}\n\n",
                "data: {\"error\":{\"message\":\"upstream midstream failure\"}}\n\n",
                "data: [DONE]\n\n"
            );
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        }),
    );
    let up_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up_addr = up_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(up_listener, upstream_app).await;
    });

    let (_dir, state, app_router) = boot_test_server();
    isolate_empty_providers(&state, _dir.path());
    let (user_a_id, tok_a) = register_user(&app_router, "tier5_sse_err_a").await;
    let (user_b_id, tok_b) = register_user(&app_router, "tier5_sse_err_b").await;

    let (st_save, _) = http_json(
        &app_router,
        "POST",
        "/api/v1/hermes/magpie/providers",
        Some(&tok_a),
        Some(json!({
            "preset_id": "deepseek",
            "api_key": "sk-srv-sse-err",
            "base_url": format!("http://{up_addr}/v1"),
            "default_model": "deepseek-chat",
            "activate": true
        })),
    )
    .await;
    assert_eq!(st_save, StatusCode::OK);

    // Check all four server SSE streaming endpoints emit no [DONE] on mid-stream error
    for (uri, payload) in [
        (
            "/api/v1/magpie/v1/chat/completions",
            json!({
                "model": "deepseek-chat",
                "messages": [{ "role": "user", "content": "hi" }],
                "stream": true
            }),
        ),
        (
            "/api/v1/magpie/v1/responses",
            json!({
                "model": "deepseek-chat",
                "input": "hi",
                "stream": true
            }),
        ),
        (
            "/api/v1/magpie/v1/messages",
            json!({
                "model": "deepseek-chat",
                "max_tokens": 32,
                "messages": [{ "role": "user", "content": "hi" }],
                "stream": true
            }),
        ),
        (
            "/v1/chat/completions",
            json!({
                "model": "deepseek-chat",
                "messages": [{ "role": "user", "content": "hi" }],
                "stream": true
            }),
        ),
    ] {
        let (st, _ct, bytes) = http_raw(&app_router, "POST", uri, Some(&tok_a), Some(payload)).await;
        assert_eq!(st, StatusCode::OK, "uri={uri}");
        let body = String::from_utf8_lossy(&bytes);
        assert!(
            !body.contains("[DONE]"),
            "endpoint {uri} emitted [DONE] on mid-stream error: {body}"
        );
        assert!(
            body.contains("upstream midstream failure"),
            "endpoint {uri} missing error message: {body}"
        );
        if uri == "/api/v1/magpie/v1/messages" {
            assert!(
                body.contains("event: error"),
                "AnthropicMessages missing event: error: {body}"
            );
            let data_line = body
                .lines()
                .rev()
                .find(|l| l.starts_with("data:"))
                .expect("data line in Anthropic error SSE");
            let err_val: Value =
                serde_json::from_str(data_line.strip_prefix("data:").unwrap().trim()).unwrap();
            assert_eq!(err_val["type"].as_str(), Some("error"));
            assert_eq!(err_val["error"]["type"].as_str(), Some("api_error"));
        }
    }

    // 2. send_as_peer_id authorization ordering: 403 BEFORE process_incoming_message_strict
    let (st_c1, c1_val) = http_json(
        &app_router,
        "POST",
        "/api/v1/chats",
        Some(&tok_a),
        Some(json!({
            "title": "Chat1 Shared",
            "chat_type": 1,
            "member_ids": [user_b_id]
        })),
    )
    .await;
    assert_eq!(st_c1, StatusCode::CREATED);
    let chat1_id = c1_val["chat_id"].as_i64().unwrap();

    let (st_c2, c2_val) = http_json(
        &app_router,
        "POST",
        "/api/v1/chats",
        Some(&tok_b),
        Some(json!({
            "title": "Chat2 B Only",
            "chat_type": 2,
            "member_ids": []
        })),
    )
    .await;
    assert_eq!(st_c2, StatusCode::CREATED);
    let chat2_id = c2_val["chat_id"].as_i64().unwrap();

    let (_st_h0, hist_before) = http_json(
        &app_router,
        "GET",
        &format!("/api/v1/chats/{chat1_id}/messages?limit=50"),
        Some(&tok_a),
        None,
    )
    .await;
    let count_before = hist_before["messages"].as_array().map(|a| a.len()).unwrap_or(0);
    let (_st_chats0, chats_before) = http_json(&app_router, "GET", "/api/v1/chats", Some(&tok_a), None).await;
    let pts_before = chats_before["chats"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["chat_id"].as_i64() == Some(chat1_id))
        .unwrap()["pts"]
        .as_i64()
        .unwrap();

    let (st_forbid, forbid_val) = http_json(
        &app_router,
        "POST",
        &format!("/api/v1/chats/{chat1_id}/messages"),
        Some(&tok_a),
        Some(json!({
            "text": "SHOULD_NOT_PERSIST_UNAUTHORIZED_SEND_AS",
            "send_as_peer_id": chat2_id,
            "client_nonce": 99901
        })),
    )
    .await;
    assert_eq!(st_forbid, StatusCode::FORBIDDEN);
    assert_eq!(forbid_val["error"].as_str(), Some("invalid_send_as"));

    let (_st_h1, hist_after) = http_json(
        &app_router,
        "GET",
        &format!("/api/v1/chats/{chat1_id}/messages?limit=50"),
        Some(&tok_a),
        None,
    )
    .await;
    let msgs_after = hist_after["messages"].as_array().unwrap();
    assert_eq!(
        msgs_after.len(),
        count_before,
        "unauthorized send_as_peer_id must NOT persist any message"
    );
    assert!(
        !hist_after
            .to_string()
            .contains("SHOULD_NOT_PERSIST_UNAUTHORIZED_SEND_AS"),
        "unauthorized message leaked into chat history"
    );
    let (_st_chats1, chats_after) = http_json(&app_router, "GET", "/api/v1/chats", Some(&tok_a), None).await;
    let pts_after = chats_after["chats"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["chat_id"].as_i64() == Some(chat1_id))
        .unwrap()["pts"]
        .as_i64()
        .unwrap();
    assert_eq!(pts_after, pts_before, "unauthorized send_as_peer_id must NOT advance PTS");

    // Positive control: sending as self succeeds
    let (st_ok, ok_val) = http_json(
        &app_router,
        "POST",
        &format!("/api/v1/chats/{chat1_id}/messages"),
        Some(&tok_a),
        Some(json!({
            "text": "AUTHORIZED_SELF_SEND_AS",
            "send_as_peer_id": user_a_id,
            "client_nonce": 99902
        })),
    )
    .await;
    assert_eq!(st_ok, StatusCode::CREATED);
    assert_eq!(ok_val["send_as_peer_id"].as_u64(), Some(user_a_id));
}

#[test]
fn test_tier5_vertex_adc_deletion_eviction_and_missing_providers_credentials() {
    let temp_dir = new_temp_dir("tier5_vertex_delete");
    let adc_path = temp_dir.path().join("adc_deletable.json");
    std::fs::write(
        &adc_path,
        json!({
            "client_id": "cid_del",
            "client_secret": "sec_del",
            "refresh_token": "rt_del_1",
            "quota_project_id": "proj_del"
        })
        .to_string(),
    )
    .unwrap();

    let mut cfg = ProviderConfig::google_vertex(
        "google_vertex:deletable",
        "Vertex Deletable",
        "https://aiplatform.googleapis.com/v1",
        "gemini-2.5-flash",
        Some("proj_del".to_string()),
    );
    cfg.headers.insert(
        "x-adc-source-path".to_string(),
        adc_path.to_string_lossy().to_string(),
    );

    let router = ProviderRouter::new();
    let mgr = router
        .get_or_create_vertex_manager("google_vertex:deletable", &cfg)
        .expect("initial ADC manager created");
    mgr.set_cached_token(magpie_gateway::CachedVertexToken::new(
        "ya29.cached_before_delete".to_string(),
        3600,
        chrono::Utc::now().timestamp_millis(),
    ));

    // Delete ADC file on disk -> cached manager MUST be evicted and None returned
    std::fs::remove_file(&adc_path).unwrap();
    let _env_lock = TIER5_ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let mut env_guard = EnvVarGuard::new();
    env_guard.remove("GOOGLE_APPLICATION_CREDENTIALS");
    env_guard.set("HOME", temp_dir.path());

    assert!(
        router
            .get_or_create_vertex_manager("google_vertex:deletable", &cfg)
            .is_none(),
        "deleting ADC file on disk must evict cached VertexTokenManager"
    );

    // parse_magpie_providers with nonexistent vertex.credentials must set active: false
    let missing_cred_path = temp_dir.path().join("does_not_exist_adc.json");
    let providers_json = json!({
        "providers": [
            {
                "id": "google-vertex",
                "preset": "google-vertex",
                "name": "Missing Creds Vertex",
                "off": false,
                "vertex": {
                    "project": "proj-missing",
                    "credentials": missing_cred_path.to_string_lossy()
                }
            },
            {
                "id": "google-vertex-ambient",
                "preset": "google-vertex",
                "name": "Ambient Vertex",
                "off": false,
                "vertex": {
                    "project": "proj-ambient"
                }
            }
        ]
    })
    .to_string();

    let parsed = parse_magpie_providers(&providers_json);
    let missing_acc = parsed
        .iter()
        .find(|a| a.id == "magpie_provider:google-vertex")
        .expect("missing creds account present");
    assert!(
        !missing_acc.active,
        "google-vertex with nonexistent credentials file must be marked active=false"
    );

    let ambient_acc = parsed
        .iter()
        .find(|a| a.id == "magpie_provider:google-vertex-ambient")
        .expect("ambient account present");
    assert!(
        ambient_acc.active,
        "google-vertex with omitted credentials field should remain active=true"
    );
}
