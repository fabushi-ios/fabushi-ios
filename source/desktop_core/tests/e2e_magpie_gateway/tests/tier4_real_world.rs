#[path = "common/mod.rs"]
mod common;

use common::*;
use axum::http::StatusCode;
use serde_json::json;
use std::time::Duration;

// ============================================================================
// Tier 4: Real-World Application Scenarios (4 Tests)
// ============================================================================

#[tokio::test]
async fn test_tier4_chat_companion_bot_turn_without_provider_error() {
    let (_dir, _state, router) = boot_test_server();
    let (openai_url, _gemini_url, mock_prov_state) = spawn_mock_providers().await;
    mock_prov_state
        .custom_reply_text
        .lock()
        .unwrap()
        .replace("COMPANION_BOT_SMOOTH_REPLY_2026".to_string());

    let (_uid, tok) = register_user(&router, "tier4_bot_user").await;

    // 1. Configure and activate provider
    let (st_save, save_val) = http_json(
        &router,
        "POST",
        "/api/v1/hermes/magpie/providers",
        Some(&tok),
        Some(json!({
            "preset_id": "deepseek",
            "api_key": "sk-mock-key-bot",
            "base_url": openai_url,
            "default_model": "deepseek-chat",
            "activate": true
        })),
    )
    .await;
    assert_eq!(st_save, StatusCode::OK, "save provider: {save_val}");
    assert_eq!(save_val["state"]["needs_provider_selection"].as_bool(), Some(false));

    // 2. Create AI Companion Bot
    let (st_bot, bot_val) = http_json(
        &router,
        "POST",
        "/api/v1/bots",
        Some(&tok),
        Some(json!({
            "display_name": "Companion AI Bot",
            "username": "companion_ai_bot",
            "description": "Companion bot for chat turn test",
            "ai_persona": "Helpful AI companion"
        })),
    )
    .await;
    assert_eq!(st_bot, StatusCode::CREATED, "create bot: {bot_val}");
    let cid = bot_val["companion_chat_id"]
        .as_i64()
        .expect("companion_chat_id");

    // 3. Post user message to companion bot chat
    let (st_send, send_val) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{cid}/messages"),
        Some(&tok),
        Some(json!({ "text": "Tell me about quantum computing" })),
    )
    .await;
    assert!(st_send.is_success(), "send message: {send_val}");

    // 4. Poll companion chat timeline: verify reply arrives with NO PROVIDER_ERROR
    let mut found_bot_reply = false;
    let mut encountered_provider_error = false;

    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (st_h, hist) = http_json(
            &router,
            "GET",
            &format!("/api/v1/chats/{cid}/messages?limit=20"),
            Some(&tok),
            None,
        )
        .await;
        assert_eq!(st_h, StatusCode::OK);

        if let Some(msgs) = hist["messages"].as_array() {
            for m in msgs {
                let text = m["text"].as_str().unwrap_or("");
                if text.contains("PROVIDER_ERROR") || text.contains("未检测到已配置的 AI 订阅") {
                    encountered_provider_error = true;
                }
                if text.contains("COMPANION_BOT_SMOOTH_REPLY_2026") {
                    found_bot_reply = true;
                }
            }
        }
        if found_bot_reply {
            break;
        }
    }

    assert!(
        !encountered_provider_error,
        "companion bot MUST NOT throw PROVIDER_ERROR when provider is configured and active"
    );
    assert!(
        found_bot_reply,
        "companion bot reply MUST arrive smoothly in chat timeline"
    );
}

#[tokio::test]
async fn test_tier4_settings_ui_activation_flow() {
    let (dir, state, router) = boot_test_server();
    isolate_empty_providers(&state, dir.path());
    let (openai_url, _gemini_url, _mock_prov_state) = spawn_mock_providers().await;

    let (_uid, tok) = register_user(&router, "tier4_ui_user").await;

    // 1. Initial status: needs_provider_selection = true
    let (st1, val1) = http_json(&router, "GET", "/api/v1/hermes/magpie/state", Some(&tok), None).await;
    assert_eq!(st1, StatusCode::OK);
    assert_eq!(val1["needs_provider_selection"].as_bool(), Some(true));

    // 2. Scan subscriptions
    let (st_scan, val_scan) = http_json(
        &router,
        "POST",
        "/api/v1/hermes/magpie/subscriptions/scan",
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st_scan, StatusCode::OK, "scan: {val_scan}");

    // 3. User activates DeepSeek provider in settings UI
    let (st_act, val_act) = http_json(
        &router,
        "POST",
        "/api/v1/hermes/magpie/providers",
        Some(&tok),
        Some(json!({
            "preset_id": "deepseek",
            "api_key": "sk-active-key-444",
            "base_url": openai_url,
            "default_model": "deepseek-chat",
            "activate": true
        })),
    )
    .await;
    assert_eq!(st_act, StatusCode::OK, "activate provider: {val_act}");
    assert_eq!(val_act["provider"]["provider_id"].as_str(), Some("deepseek"));

    // 4. Client state sync: verify needs_provider_selection is now false and active_provider is deepseek
    let (st2, val2) = http_json(&router, "GET", "/api/v1/hermes/magpie/state", Some(&tok), None).await;
    assert_eq!(st2, StatusCode::OK);
    assert_eq!(val2["needs_provider_selection"].as_bool(), Some(false));
    assert_eq!(val2["active_provider"].as_str(), Some("deepseek"));
}

#[tokio::test]
async fn test_tier4_loopback_proxy_external_client() {
    let (dir, state, router) = boot_test_server();
    isolate_empty_providers(&state, dir.path());
    let (openai_url, _gemini_url, mock_prov_state) = spawn_mock_providers().await;
    mock_prov_state
        .custom_reply_text
        .lock()
        .unwrap()
        .replace("LOOPBACK_PROXY_CLIENT_TURN_OK".to_string());

    let (_uid, tok) = register_user(&router, "tier4_proxy_user").await;

    // Save active provider
    let (st_save, _) = http_json(
        &router,
        "POST",
        "/api/v1/hermes/magpie/providers",
        Some(&tok),
        Some(json!({
            "preset_id": "deepseek",
            "api_key": "sk-proxy-client-key",
            "base_url": openai_url,
            "default_model": "deepseek-chat",
            "activate": true
        })),
    )
    .await;
    assert_eq!(st_save, StatusCode::OK);

    // External client issues OpenAI-compatible request to local gateway endpoint
    let (st_req, val_resp) = http_json(
        &router,
        "POST",
        "/api/v1/magpie/v1/chat/completions",
        Some(&tok),
        Some(json!({
            "model": "deepseek-chat",
            "messages": [{"role": "user", "content": "Loopback client test"}],
            "stream": false
        })),
    )
    .await;
    assert_eq!(st_req, StatusCode::OK, "proxy completions: {val_resp}");
    let reply = val_resp["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
    assert!(
        reply.contains("LOOPBACK_PROXY_CLIENT_TURN_OK"),
        "expected mock reply: {reply}"
    );

    // Verify usage statistics endpoint records turns
    let (st_usage, val_usage) = http_json(&router, "GET", "/api/v1/hermes/usage", Some(&tok), None).await;
    assert_eq!(st_usage, StatusCode::OK);
    assert!(val_usage["total_tokens"].as_u64().is_some());
}

#[tokio::test]
async fn test_tier4_server_restart_persistence_resiliency() {
    let temp = new_temp_dir("restart_resiliency");
    let (openai_url, _gemini_url, _mock_prov_state) = spawn_mock_providers().await;

    let tok: String;
    {
        // Boot server run 1
        let (state1, router1) = boot_test_server_in(temp.path());
        isolate_empty_providers(&state1, temp.path());
        let (_uid, t) = register_user(&router1, "restart_user").await;
        tok = t;

        // Save and activate DeepSeek provider
        let (st_save, val_save) = http_json(
            &router1,
            "POST",
            "/api/v1/hermes/magpie/providers",
            Some(&tok),
            Some(json!({
                "preset_id": "deepseek",
                "api_key": "sk-persist-test-key",
                "base_url": openai_url,
                "default_model": "deepseek-chat",
                "activate": true
            })),
        )
        .await;
        assert_eq!(st_save, StatusCode::OK, "save run 1: {val_save}");
        assert_eq!(val_save["state"]["active_provider"].as_str(), Some("deepseek"));
    } // Dropping state1 and router1 (simulating graceful shutdown)

    {
        // Boot server run 2 on SAME directory and database
        let (_state2, router2) = boot_test_server_in(temp.path());

        // Query magpie state immediately post-restart
        let (st_state, val_state) = http_json(
            &router2,
            "GET",
            "/api/v1/hermes/magpie/state",
            Some(&tok),
            None,
        )
        .await;
        assert_eq!(st_state, StatusCode::OK, "query post restart: {val_state}");

        // Active provider MUST persist across server restarts
        assert_eq!(
            val_state["active_provider"].as_str(),
            Some("deepseek"),
            "active provider must persist across restarts"
        );
        assert_eq!(
            val_state["needs_provider_selection"].as_bool(),
            Some(false),
            "needs_provider_selection must remain false after restart"
        );

        let provs = val_state["providers"].as_array().expect("providers");
        assert!(
            provs.iter().any(|p| p["provider_id"].as_str() == Some("deepseek")),
            "deepseek provider must exist in persisted state"
        );
    }
}
