use gpui_telegram_client::config::{ClientConfig, ConfigSource, StoredSession};
use gpui_telegram_client::net::http_client::{
    AgentRunRequest, HttpClient, LoginRequest, RegisterRequest, SendMessageHttpRequest,
};
use gpui_telegram_client::sync::coordinator::ClientSyncCoordinator;
use gpui_telegram_client::sync::SyncOutcome;
use gpui_telegram_core::types::{ChatId, UserId};
use gpui_telegram_proto::sync::{PtsUpdate, TypingAction, TypingEvent, UpdateEvent};

#[tokio::test]
async fn test_live_cloud_backend_connectivity_and_flow() {
    let base_url = std::env::var("GPUI_TELEGRAM_SERVER_URL")
        .unwrap_or_else(|_| "http://34.46.77.133:8080".to_string());
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
        model_id: Some("deepseek-flash".to_string()),
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
        .unwrap_or_else(|_| "http://34.46.77.133:8080".to_string());
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

#[tokio::test]
async fn test_live_cloud_http_send_message_and_chats_and_history() {
    let base_url = std::env::var("GPUI_TELEGRAM_SERVER_URL")
        .unwrap_or_else(|_| "http://34.46.77.133:8080".to_string());
    let client = HttpClient::new(base_url.clone());

    let login_req = LoginRequest {
        username: "cloud_probe_user".to_string(),
        password: "probe_password_123".to_string(),
        device_name: "GPUI M3 Live HTTP Test".to_string(),
    };
    let auth = client
        .login(&login_req)
        .await
        .expect("Login to live backend failed");
    assert!(!auth.token.is_empty());

    // 1. Query GET /api/v1/chats and provision/select real chat
    let chats_res = client.get_chats(&auth.token).await;
    assert!(chats_res.is_ok(), "get_chats failed: {:?}", chats_res);
    let target_chat_id = if let Some(first) = chats_res.unwrap().first() {
        first.chat_id as u64
    } else {
        client
            .provision_welcome_chat(&auth.token)
            .await
            .expect("provision_welcome_chat failed")
            .chat_id as u64
    };

    // 2. Query POST /api/v1/messages/send
    let send_req = SendMessageHttpRequest {
        chat_id: target_chat_id,
        text: "M3 verification message from worker_m3 test suite".to_string(),
        reply_to: None,
    };
    let send_res = client.send_message(&auth.token, &send_req).await;
    assert!(send_res.is_ok(), "send_message failed: {:?}", send_res);
    let send_ack = send_res.unwrap();
    assert!(send_ack.message_id > 0);
    assert!(send_ack.pts > 0);

    // 3. Query GET /api/v1/messages/history?chat_id=<target_chat_id>
    let hist_res = client.get_chat_messages(&auth.token, target_chat_id, Some(20)).await;
    assert!(hist_res.is_ok(), "get_chat_messages failed: {:?}", hist_res);
}

#[test]
fn test_pts_delta_sync_gap_detection_and_pending_buffer() {
    let coordinator = ClientSyncCoordinator::new();
    let chat = ChatId(101);

    // Initial state: unseeded chat returns 0
    assert_eq!(coordinator.last_pts(chat), 0);

    // Seed PTS to 10
    coordinator.set_pts(chat, 10);
    assert_eq!(coordinator.last_pts(chat), 10);

    // 1. Contiguous update: PTS 11 (pts_count = 1)
    let update_11 = PtsUpdate::new(
        chat,
        11,
        1,
        UpdateEvent::UserTyping(TypingEvent::new(chat, UserId(1), TypingAction::Typing)),
        chrono::Utc::now().timestamp_millis(),
    );
    let outcome_11 = coordinator.evaluate_update(&update_11);
    assert_eq!(outcome_11, SyncOutcome::Contiguous);
    assert_eq!(coordinator.last_pts(chat), 11);

    // 2. Duplicate update: PTS 11 again
    let outcome_dup = coordinator.evaluate_update(&update_11);
    assert_eq!(outcome_dup, SyncOutcome::Duplicate);

    // 3. Gap detected: PTS 14 arrives when last_seen is 11
    let update_14 = PtsUpdate::new(
        chat,
        14,
        1,
        UpdateEvent::UserTyping(TypingEvent::new(chat, UserId(1), TypingAction::Typing)),
        chrono::Utc::now().timestamp_millis(),
    );
    let outcome_14 = coordinator.evaluate_update(&update_14);
    assert_eq!(
        outcome_14,
        SyncOutcome::GapDetected {
            last_seen: 11,
            incoming: 14,
            missing_range: 12..=13,
        }
    );

    // Buffer the out-of-order gap update 14
    coordinator.buffer_gap_update(update_14.clone());

    // Buffer update 13 as well
    let update_13 = PtsUpdate::new(
        chat,
        13,
        1,
        UpdateEvent::UserTyping(TypingEvent::new(chat, UserId(1), TypingAction::Typing)),
        chrono::Utc::now().timestamp_millis(),
    );
    coordinator.buffer_gap_update(update_13.clone());

    // Prior to filling PTS 12, drain returns empty because 12 is missing
    let early_drain = coordinator.drain_contiguous_buffer(chat);
    assert!(early_drain.is_empty());
    assert_eq!(coordinator.last_pts(chat), 11);

    // Now advance PTS to 12 (simulating sync difference fetching missing message 12)
    coordinator.set_pts(chat, 12);

    // Draining buffer should now yield 13, then 14 in contiguous sequence!
    let drained = coordinator.drain_contiguous_buffer(chat);
    assert_eq!(drained.len(), 2);
    assert_eq!(drained[0].pts, 13);
    assert_eq!(drained[1].pts, 14);
    assert_eq!(coordinator.last_pts(chat), 14);

    // Subsequent drain is empty
    assert!(coordinator.drain_contiguous_buffer(chat).is_empty());
}

#[test]
fn test_session_storage_atomic_0600_permissions_and_lifecycle() {
    let session = StoredSession {
        server_url: "http://34.46.77.133:8080".to_string(),
        user_id: 100150,
        username: "test_m3_secure".to_string(),
        token: "sess_secret_token_abc123".to_string(),
        session_id: "sess_id_xyz".to_string(),
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    };

    // Save session
    ClientConfig::save_session(&session).expect("Failed to save session");
    let session_file = ClientConfig::get_session_path();
    assert!(session_file.exists());

    // POSIX 0600 permission check
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(&session_file).expect("Metadata read failed");
        let permissions = metadata.permissions();
        let mode = permissions.mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "Session file must be strictly restricted to POSIX 0600 (read/write by owner only), found 0o{:o}",
            mode
        );
    }

    // Load session and verify exact round-trip match
    let loaded = ClientConfig::load_session().expect("Load session failed");
    assert_eq!(loaded, session);

    // Clear session and verify file removal
    ClientConfig::clear_session();
    assert!(!session_file.exists());
}

#[test]
fn test_cascading_four_tier_configuration_resolution() {
    // 1. Verify Tier 1 CLI arg overrides default
    let cli_args = vec![
        "gpui_telegram".to_string(),
        "--server-url".to_string(),
        "http://cli-override.local:8080/".to_string(),
    ];
    let resolved = ClientConfig::resolve_with_args(&cli_args);
    assert_eq!(resolved.server_url, "http://cli-override.local:8080");
    assert_eq!(resolved.config_source, ConfigSource::CliArgument);

    // 2. Verify short CLI flag `-s <url>`
    let cli_short = vec![
        "gpui_telegram".to_string(),
        "-s".to_string(),
        "http://cli-short.local:8080".to_string(),
    ];
    let resolved_short = ClientConfig::resolve_with_args(&cli_short);
    assert_eq!(resolved_short.server_url, "http://cli-short.local:8080");
    assert_eq!(resolved_short.config_source, ConfigSource::CliArgument);

    // 3. Verify key-value syntax `-s=<url>` and `--server-url=<url>`
    let cli_eq = vec![
        "gpui_telegram".to_string(),
        "-s=http://cli-eq.local:8080".to_string(),
    ];
    let resolved_eq = ClientConfig::resolve_with_args(&cli_eq);
    assert_eq!(resolved_eq.server_url, "http://cli-eq.local:8080");
    assert_eq!(resolved_eq.config_source, ConfigSource::CliArgument);
}

#[tokio::test]
async fn test_live_cloud_reaction_roundtrip_and_durable_sqlite_persistence() {
    let base_url = std::env::var("GPUI_TELEGRAM_SERVER_URL")
        .unwrap_or_else(|_| "http://34.46.77.133:8080".to_string());
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let cfg = ClientConfig {
        server_url: base_url.clone(),
        ..Default::default()
    };

    let client = gpui_telegram_client::TelegramClient::open_with_dir(cfg.clone(), temp_dir.path())
        .expect("open durable client");
    let login_req = LoginRequest {
        username: "cloud_probe_user".to_string(),
        password: "probe_password_123".to_string(),
        device_name: "GPUI Live Reaction E2E Test".to_string(),
    };
    let auth = client
        .http_client
        .login(&login_req)
        .await
        .expect("login to live cloud server");
    client.set_auth_session(auth.clone());

    let chats = client
        .http_client
        .get_chats(&auth.token)
        .await
        .expect("get_chats on live server");
    let target_chat_id = if let Some(first) = chats.first() {
        first.chat_id as u64
    } else {
        client
            .http_client
            .provision_welcome_chat(&auth.token)
            .await
            .expect("provision_welcome_chat")
            .chat_id as u64
    };

    let send_ack = client
        .http_client
        .send_message(
            &auth.token,
            &SendMessageHttpRequest {
                chat_id: target_chat_id,
                text: "Live server reaction roundtrip verification".to_string(),
                reply_to: None,
            },
        )
        .await
        .expect("send_message on live server");
    let mid = send_ack.message_id as u64;

    // React against the REAL live server (`http://34.46.77.133:8080`)
    let rx_resp = client
        .react_to_message(target_chat_id as i64, mid as i64, "👍", Some("add"))
        .await
        .expect("react_to_message against live server must succeed");
    assert!(rx_resp.pts > 0);

    // Reload history from live server and verify 👍 reaction is present
    let hist = client
        .get_chat_messages(target_chat_id, Some(20))
        .await
        .expect("get_chat_messages on live server");
    let target_hm = hist
        .iter()
        .find(|m| m.message_id == mid)
        .expect("sent message in history");
    assert!(
        target_hm
            .reactions
            .iter()
            .any(|r| r.emoji == "👍" && r.chosen),
        "Reloaded history from live server must retain 👍 reaction"
    );

    let tok_for_revoke = auth.token.clone();
    let sess_for_revoke = auth.session_id.clone();
    tokio::task::spawn_blocking(move || drop(client))
        .await
        .unwrap();

    // Reopen fresh `TelegramClient::open_with_dir` on same directory (app restart!) and reload history
    let reopened = gpui_telegram_client::TelegramClient::open_with_dir(cfg, temp_dir.path())
        .expect("reopen durable client");
    reopened.set_token(Some(tok_for_revoke.clone()));
    let hist_restarted = reopened
        .get_chat_messages(target_chat_id, Some(20))
        .await
        .expect("get_chat_messages after restart");
    let restarted_hm = hist_restarted
        .iter()
        .find(|m| m.message_id == mid)
        .expect("sent message after restart");
    assert!(
        restarted_hm
            .reactions
            .iter()
            .any(|r| r.emoji == "👍" && r.chosen),
        "Reaction against live default server must survive app restart via SQLite cache"
    );

    // Toggle OFF (`remove`) against the live default server and verify empty state persists across reload
    let rm_resp = reopened
        .react_to_message(target_chat_id as i64, mid as i64, "👍", Some("remove"))
        .await
        .expect("remove reaction against live server must succeed");
    assert!(rm_resp.pts >= rx_resp.pts);
    let hist_after_remove = reopened
        .get_chat_messages(target_chat_id, Some(20))
        .await
        .expect("get_chat_messages after remove");
    let removed_hm = hist_after_remove
        .iter()
        .find(|m| m.message_id == mid)
        .expect("sent message after remove");
    assert!(
        !removed_hm.reactions.iter().any(|r| r.emoji == "👍" && r.chosen),
        "Removing 👍 reaction against live server must clear chosen reaction on reload"
    );

    let _ = reopened
        .http_client
        .revoke_session(&tok_for_revoke, &sess_for_revoke)
        .await;
    tokio::task::spawn_blocking(move || drop(reopened))
        .await
        .unwrap();
}
