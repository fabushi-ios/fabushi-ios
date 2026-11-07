use gpui_telegram_client::net::http_client::{
    AgentRunRequest, HttpClient, LoginRequest, RegisterRequest,
};

#[tokio::test]
async fn test_live_cloud_backend_connectivity_and_flow() {
    let base_url = std::env::var("GPUI_TELEGRAM_SERVER_URL")
        .unwrap_or_else(|_| "http://34.170.16.35:8080".to_string());
    let client = HttpClient::new(base_url.clone());

    // 1. Health check
    let health = client.check_health().await;
    assert!(health.is_ok(), "Health check failed: {:?}", health);
    assert!(health.unwrap(), "Health returned false");

    // 2. Status check
    let status = client.check_status().await;
    assert!(status.is_ok(), "Status check failed: {:?}", status);
    let status_val = status.unwrap();
    assert_eq!(status_val.status, "healthy");

    // 3. Register a unique test user
    let timestamp = chrono::Utc::now().timestamp_millis();
    let username = format!("m3_test_{}", timestamp);
    let password = "TestPassword123!".to_string();
    let reg_req = RegisterRequest {
        username: username.clone(),
        password: password.clone(),
        first_name: "Milestone3Tester".to_string(),
        device_name: "GPUI Desktop Client Test".to_string(),
    };

    let reg_res = client.register(&reg_req).await;
    assert!(reg_res.is_ok(), "Registration failed: {:?}", reg_res);
    let reg_auth = reg_res.unwrap();
    assert_eq!(reg_auth.username, username);
    assert!(!reg_auth.token.is_empty());
    assert!(!reg_auth.session_id.is_empty());

    // 4. Login with registered user
    let login_req = LoginRequest {
        username: username.clone(),
        password: password.clone(),
        device_name: "GPUI Desktop Client Test".to_string(),
    };
    let login_res = client.login(&login_req).await;
    assert!(login_res.is_ok(), "Login failed: {:?}", login_res);
    let login_auth = login_res.unwrap();
    assert_eq!(login_auth.username, username);
    assert!(!login_auth.token.is_empty());

    // 5. Get current profile (GET /api/v1/auth/me)
    let me_res = client.get_me(&login_auth.token).await;
    assert!(me_res.is_ok(), "get_me failed: {:?}", me_res);
    let profile = me_res.unwrap();
    assert_eq!(profile.user_id, login_auth.user_id);
    assert_eq!(profile.session_id, login_auth.session_id);

    // 6. Test AI Bot agent execution (POST /api/v1/agent/runs)
    let agent_req = AgentRunRequest {
        prompt: "Hello Autonomous Agent, compute 7 * 6.".to_string(),
        session_id: login_auth.session_id.clone(),
        model_id: Some("grok-3".to_string()),
        thinking_budget_tokens: Some(1024),
    };
    let agent_res = client.agent_runs(&login_auth.token, &agent_req).await;
    assert!(agent_res.is_ok(), "agent_runs failed: {:?}", agent_res);
    let run = agent_res.unwrap();
    assert_eq!(run.status, "completed");
    assert!(!run.response.is_empty(), "AI response should not be empty");
    println!("[Live Cloud Test] AI Response: {}", run.response);

    // 7. Revoke session (DELETE /api/v1/auth/sessions/{session_id})
    let revoke_res = client
        .revoke_session(&login_auth.token, &login_auth.session_id)
        .await;
    assert!(revoke_res.is_ok(), "revoke_session failed: {:?}", revoke_res);
    assert!(revoke_res.unwrap());
}

#[test]
fn test_non_tokio_executor_login_and_ws_start_no_panic() {
    // Regression test for GPUI ForegroundExecutor (non-Tokio main thread):
    // Ensures HttpClient async methods and WsTransport::start() never panic with
    // "there is no reactor running, must be called from the context of a Tokio 1.x runtime"
    // when invoked outside of a thread-local Tokio runtime context.
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "This test must run on a non-Tokio thread to simulate GPUI's ForegroundExecutor"
    );

    let base_url = std::env::var("GPUI_TELEGRAM_SERVER_URL")
        .unwrap_or_else(|_| "http://34.170.16.35:8080".to_string());
    let tg_client = gpui_telegram_client::TelegramClient::new_with_url(base_url);

    let login_req = LoginRequest {
        username: "cloud_probe_user".to_string(),
        password: "probe_password_123".to_string(),
        device_name: "GPUI ForegroundExecutor Test".to_string(),
    };

    let auth = futures::executor::block_on(tg_client.http_client.login(&login_req))
        .expect("Login from non-Tokio executor failed");
    assert_eq!(auth.username, "cloud_probe_user");
    assert!(!auth.token.is_empty());

    // Starting WsTransport from a non-Tokio thread must also succeed without panicking
    tg_client.ws_transport.set_token(Some(auth.token.clone()));
    tg_client.ws_transport.start();
    tg_client.ws_transport.stop();
}
