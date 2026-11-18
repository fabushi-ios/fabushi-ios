use gpui_telegram_client::config::{ClientConfig, StoredSession};
use gpui_telegram_client::net::http_client::{
    AlipayPollResponse, AlipayStartResponse, EmailRegisterRequest, EmailSendCodeRequest,
    EmailSendCodeResponse, HttpClient,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// --- Serialization & Model Tests ---

#[test]
fn test_alipay_start_response_serde_with_alias() {
    let raw = r#"{
        "attempt_id": "attempt_12345",
        "auth_url": "https://openauth.alipay.com/oauth2/publicAppAuthorize.htm?app_id=123",
        "expires_at_ms": 1780000000000
    }"#;
    let res: AlipayStartResponse = serde_json::from_str(raw).expect("Deserialization failed");
    assert_eq!(res.attempt_id, "attempt_12345");
    assert_eq!(
        res.login_url,
        "https://openauth.alipay.com/oauth2/publicAppAuthorize.htm?app_id=123"
    );
    assert_eq!(res.expires_at_ms, 1780000000000);
}

#[test]
fn test_alipay_poll_response_lifecycle_states() {
    // 1. Pending state
    let pending_json = r#"{"status": "pending"}"#;
    let poll_pending: AlipayPollResponse = serde_json::from_str(pending_json).unwrap();
    assert!(poll_pending.is_pending());
    assert!(!poll_pending.is_completed());
    assert!(!poll_pending.is_expired());
    assert!(poll_pending.token.is_none());
    assert!(poll_pending.user.is_none());

    // 2. Expired state
    let expired_json = r#"{"status": "expired"}"#;
    let poll_expired: AlipayPollResponse = serde_json::from_str(expired_json).unwrap();
    assert!(poll_expired.is_expired());
    assert!(!poll_expired.is_completed());

    // 3. Completed state
    let complete_json = r#"{
        "status": "complete",
        "token": "sess_token_alipay_999",
        "user": {
            "user_id": 200001,
            "username": "alipay_user_200001",
            "first_name": "支付宝用户",
            "last_name": null,
            "phone": null,
            "photo_url": null
        }
    }"#;
    let poll_complete: AlipayPollResponse = serde_json::from_str(complete_json).unwrap();
    assert!(poll_complete.is_completed());
    assert!(!poll_complete.is_pending());
    assert_eq!(poll_complete.token.as_deref(), Some("sess_token_alipay_999"));
    let user = poll_complete.user.expect("Expected user to be present");
    assert_eq!(user.user_id, 200001);
    assert_eq!(user.first_name, "支付宝用户");
}

#[test]
fn test_email_challenge_request_response_serde() {
    let req = EmailSendCodeRequest {
        email: "alice@example.com".to_string(),
    };
    let json_str = serde_json::to_string(&req).unwrap();
    let decoded: EmailSendCodeRequest = serde_json::from_str(&json_str).unwrap();
    assert_eq!(req, decoded);

    let res_json = r#"{
        "sent": true,
        "cooldown_seconds": 60,
        "expires_in_seconds": 600
    }"#;
    let res: EmailSendCodeResponse = serde_json::from_str(res_json).unwrap();
    assert!(res.sent);
    assert_eq!(res.cooldown_seconds, 60);
    assert_eq!(res.expires_in_seconds, 600);
}

#[test]
fn test_email_register_request_serde() {
    let reg = EmailRegisterRequest {
        email: "bob@example.com".to_string(),
        code: "654321".to_string(),
        username: "bob_master".to_string(),
        password: "SecurePassword123!".to_string(),
        first_name: "Bob".to_string(),
    };
    let reg_json = serde_json::to_string(&reg).unwrap();
    let reg_decoded: EmailRegisterRequest = serde_json::from_str(&reg_json).unwrap();
    assert_eq!(reg, reg_decoded);
}

// --- Live In-Memory Mock Server Integration Tests ---

#[tokio::test]
async fn test_http_client_auth_methods_against_mock_server() {
    let scanned_flag = Arc::new(AtomicBool::new(false));
    let scanned_clone = scanned_flag.clone();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };
            let flag = scanned_clone.clone();

            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = match stream.read(&mut buf).await {
                    Ok(n) if n > 0 => n,
                    _ => return,
                };
                let req_text = String::from_utf8_lossy(&buf[..n]);
                let first_line = req_text.lines().next().unwrap_or("");

                let (status_line, body) = if first_line.starts_with("POST /api/v1/auth/alipay/start") {
                    (
                        "HTTP/1.1 200 OK",
                        r#"{"attempt_id":"att_mock_001","login_url":"https://auth.example.com/qr","expires_at_ms":1780000000000}"#,
                    )
                } else if first_line.starts_with("POST /api/v1/auth/alipay/simulate-scan") {
                    flag.store(true, Ordering::SeqCst);
                    (
                        "HTTP/1.1 200 OK",
                        r#"{"status":"ok"}"#,
                    )
                } else if first_line.starts_with("GET /api/v1/auth/alipay/poll/att_mock_001") {
                    (
                        "HTTP/1.1 200 OK",
                        r#"{"status":"complete","token":"token_secret_123","user":{"user_id":300001,"username":"alipay_hero","first_name":"Hero"}}"#,
                    )
                } else if first_line.starts_with("POST /api/v1/auth/email/send-code") {
                    if req_text.contains("dup@example.com") {
                        (
                            "HTTP/1.1 409 Conflict",
                            r#"{"error":"email_already_registered"}"#,
                        )
                    } else {
                        (
                            "HTTP/1.1 200 OK",
                            r#"{"sent":true,"cooldown_seconds":60,"expires_in_seconds":600}"#,
                        )
                    }
                } else if first_line.starts_with("POST /api/v1/auth/email/register") {
                    if req_text.contains("123456") {
                        (
                            "HTTP/1.1 201 Created",
                            r#"{"user_id":400001,"username":"email_tester","session_id":"sess_email_400001","token":"token_email_secret","first_name":"Tester","created_at":"2026-11-08T00:00:00Z","expires_at":"2026-12-08T00:00:00Z"}"#,
                        )
                    } else {
                        (
                            "HTTP/1.1 400 Bad Request",
                            r#"{"error":"invalid_code"}"#,
                        )
                    }
                } else {
                    (
                        "HTTP/1.1 404 Not Found",
                        r#"{"error":"not_found"}"#,
                    )
                };

                let resp = format!(
                    "{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status_line,
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.flush().await;
            });
        }
    });

    let client = HttpClient::new(format!("http://{}", addr));

    // 1. Alipay Start
    let start_res = client.alipay_start().await.expect("alipay_start failed");
    assert_eq!(start_res.attempt_id, "att_mock_001");
    assert_eq!(start_res.login_url, "https://auth.example.com/qr");

    // 2. Alipay Simulate Scan
    client
        .alipay_simulate_scan("att_mock_001")
        .await
        .expect("alipay_simulate_scan failed");
    assert!(scanned_flag.load(Ordering::SeqCst));

    // 3. Alipay Poll Completed
    let poll_res = client
        .alipay_poll("att_mock_001")
        .await
        .expect("alipay_poll failed");
    assert!(poll_res.is_completed());
    assert_eq!(poll_res.token.as_deref(), Some("token_secret_123"));
    let u = poll_res.user.expect("expected user");
    assert_eq!(u.user_id, 300001);
    assert_eq!(u.first_name, "Hero");

    // 4. Email Send Code Success
    let send_res = client
        .email_send_code("valid@example.com")
        .await
        .expect("email_send_code failed");
    assert!(send_res.sent);
    assert_eq!(send_res.cooldown_seconds, 60);

    // 5. Email Send Code Duplicate (409 Conflict)
    let dup_err = client.email_send_code("dup@example.com").await;
    assert!(dup_err.is_err(), "Expected 409 conflict error");

    // 6. Email Register
    let reg_req = EmailRegisterRequest {
        email: "valid@example.com".to_string(),
        code: "123456".to_string(),
        username: "email_tester".to_string(),
        password: "PassWord123!".to_string(),
        first_name: "Tester".to_string(),
    };
    let reg_res = client.email_register(&reg_req).await.expect("email_register failed");
    assert_eq!(reg_res.username, "email_tester");
    assert_eq!(reg_res.token, "token_email_secret");
}

// --- Non-Tokio Context & Session POSIX 0600 Mode ---

#[test]
fn test_auth_client_save_session_0600_mode() {
    let temp_dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", temp_dir.path());
    }

    let session = StoredSession {
        server_url: "http://127.0.0.1:8080".to_string(),
        user_id: 500001,
        username: "saved_user".to_string(),
        token: "secret_session_token_xyz".to_string(),
        session_id: "sess_500001".to_string(),
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    };

    ClientConfig::save_session(&session).expect("save_session failed");
    let session_path = ClientConfig::get_session_path();
    assert!(session_path.exists());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(&session_path).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "Saved session must be strictly 0600");
    }

    let loaded = ClientConfig::load_session().expect("Failed to load saved session");
    assert_eq!(loaded, session);
}

#[tokio::test]
async fn test_fabushi_alipay_web_auth_bridge_and_cli_session_sync() {
    use std::sync::atomic::AtomicUsize;

    let synced_uid = Arc::new(parking_lot::Mutex::new(String::new()));
    let synced_uid_clone = synced_uid.clone();
    let start_call_count = Arc::new(AtomicUsize::new(0));
    let start_call_clone = start_call_count.clone();
    let retry_cli_consumed = Arc::new(AtomicBool::new(false));
    let retry_cli_clone = retry_cli_consumed.clone();
    let retry_sync_attempts = Arc::new(AtomicUsize::new(0));
    let retry_sync_clone = retry_sync_attempts.clone();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };
            let uid_store = synced_uid_clone.clone();
            let start_counter = start_call_clone.clone();
            let cli_consumed = retry_cli_clone.clone();
            let sync_attempts = retry_sync_clone.clone();

            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = match stream.read(&mut buf).await {
                    Ok(n) if n > 0 => n,
                    _ => return,
                };
                let req_text = String::from_utf8_lossy(&buf[..n]);
                let first_line = req_text.lines().next().unwrap_or("");

                let (status_line, body) = if first_line.starts_with("POST /api/v1/auth/alipay/start") {
                    let seq = start_counter.fetch_add(1, Ordering::SeqCst);
                    match seq {
                        0 => (
                            "HTTP/1.1 200 OK",
                            r#"{"attempt_id":"att_fb_999","auth_url":"https://openauth.alipay.com/oauth2/publicAppAuthorize.htm?app_id=20210001&state=placeholder","expires_at_ms":1790000000000}"#,
                        ),
                        1 => (
                            "HTTP/1.1 200 OK",
                            r#"{"attempt_id":"att_fb_expired_404","auth_url":"https://openauth.alipay.com/oauth2/publicAppAuthorize.htm?app_id=20210001&state=placeholder","expires_at_ms":1790000000000}"#,
                        ),
                        _ => (
                            "HTTP/1.1 200 OK",
                            r#"{"attempt_id":"att_fb_retry_once","auth_url":"https://openauth.alipay.com/oauth2/publicAppAuthorize.htm?app_id=20210001&state=placeholder","expires_at_ms":1790000000000}"#,
                        ),
                    }
                } else if first_line.starts_with("GET /api/auth/alipay/login-url?platform=cli") {
                    let seq = start_counter.load(Ordering::SeqCst);
                    match seq {
                        1 => (
                            "HTTP/1.1 200 OK",
                            r#"{"authUrl":"https://openauth.alipay.com/oauth2/publicAppAuthorize.htm?app_id=2021005193647715&scope=auth_user&redirect_uri=https%3A%2F%2Fapi.ombhrum.com%2Fapi%2Fauth%2Falipay%2Fcallback&state=fb_real_state_777","state":"fb_real_state_777","appId":"2021005193647715","platform":"web"}"#,
                        ),
                        2 => (
                            "HTTP/1.1 200 OK",
                            r#"{"authUrl":"https://openauth.alipay.com/oauth2/publicAppAuthorize.htm?app_id=2021005193647715&state=fb_state_missing_404","state":"fb_state_missing_404","appId":"2021005193647715","platform":"web"}"#,
                        ),
                        _ => (
                            "HTTP/1.1 200 OK",
                            r#"{"authUrl":"https://openauth.alipay.com/oauth2/publicAppAuthorize.htm?app_id=2021005193647715&state=fb_state_onetime_888","state":"fb_state_onetime_888","appId":"2021005193647715","platform":"web"}"#,
                        ),
                    }
                } else if first_line.starts_with("GET /api/auth/alipay/cli-session?state=fb_real_state_777") {
                    (
                        "HTTP/1.1 200 OK",
                        r#"{"success":true,"status":"complete","accessToken":"fb_jwt_token","username":"fabushi_alipay_master","user":{"id":88801,"username":"fabushi_alipay_master","alipayUserId":"2088102199998888","alipayNickname":"大乘行者"}}"#,
                    )
                } else if first_line.starts_with("GET /api/auth/alipay/cli-session?state=fb_state_missing_404") {
                    (
                        "HTTP/1.1 404 Not Found",
                        r#"{"error":"登录状态不存在或已过期"}"#,
                    )
                } else if first_line.starts_with("GET /api/auth/alipay/cli-session?state=fb_state_onetime_888") {
                    // Simulate Fabushi D1 atomic one-time state deletion: first call returns 200 complete, subsequent calls return 404
                    if !cli_consumed.swap(true, Ordering::SeqCst) {
                        (
                            "HTTP/1.1 200 OK",
                            r#"{"success":true,"status":"complete","accessToken":"fb_jwt_onetime","username":"onetime_user","user":{"id":88802,"username":"onetime_user","alipayUserId":"2088102177776666","alipayNickname":"般若行者"}}"#,
                        )
                    } else {
                        (
                            "HTTP/1.1 404 Not Found",
                            r#"{"error":"登录状态不存在或已过期"}"#,
                        )
                    }
                } else if first_line.starts_with("POST /api/v1/auth/alipay/simulate-scan") {
                    if req_text.contains("2088102199998888") {
                        *uid_store.lock() = "2088102199998888".to_string();
                        ("HTTP/1.1 200 OK", r#"{"status":"completed"}"#)
                    } else if req_text.contains("2088102177776666") {
                        let attempt_no = sync_attempts.fetch_add(1, Ordering::SeqCst);
                        if attempt_no == 0 {
                            // First simulate-scan fails transiently
                            ("HTTP/1.1 503 Service Unavailable", r#"{"error":"transient_failure"}"#)
                        } else {
                            ("HTTP/1.1 200 OK", r#"{"status":"completed"}"#)
                        }
                    } else {
                        ("HTTP/1.1 200 OK", r#"{"status":"completed"}"#)
                    }
                } else if first_line.starts_with("GET /api/v1/auth/alipay/poll/att_fb_999") {
                    (
                        "HTTP/1.1 200 OK",
                        r#"{"status":"completed","session":{"user_id":88801,"username":"alipay_20881021","first_name":"大乘行者","session_id":"sess_fb_88801","token":"gpui_token_real_88801"}}"#,
                    )
                } else if first_line.starts_with("GET /api/v1/auth/alipay/poll/att_fb_retry_once") {
                    if sync_attempts.load(Ordering::SeqCst) >= 2 {
                        (
                            "HTTP/1.1 200 OK",
                            r#"{"status":"completed","session":{"user_id":88802,"username":"alipay_20881021","first_name":"般若行者","session_id":"sess_fb_88802","token":"gpui_token_onetime_88802"}}"#,
                        )
                    } else {
                        ("HTTP/1.1 200 OK", r#"{"status":"pending"}"#)
                    }
                } else {
                    ("HTTP/1.1 404 Not Found", r#"{"error":"not_found"}"#)
                };

                let resp = format!(
                    "{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status_line,
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.flush().await;
            });
        }
    });

    let base = format!("http://{}", addr);
    unsafe {
        std::env::set_var("FABUSHI_AUTH_BASE_URL", &base);
    }
    let client = HttpClient::new(base);

    // 1. Normal Fabushi Alipay OAuth URL upgrade + cli-session completion sync
    let start_res = client.alipay_start().await.expect("alipay_start must succeed");
    assert_eq!(start_res.attempt_id, "att_fb_999");
    assert!(
        start_res.login_url.contains("app_id=2021005193647715")
            && start_res.login_url.contains("state=fb_real_state_777"),
        "Must upgrade login_url to real Fabushi Alipay OAuth URL, got: {}",
        start_res.login_url
    );

    let poll_res = client
        .alipay_poll("att_fb_999")
        .await
        .expect("alipay_poll must succeed");
    assert!(poll_res.is_completed());
    assert_eq!(synced_uid.lock().as_str(), "2088102199998888");
    assert_eq!(poll_res.token.as_deref(), Some("gpui_token_real_88801"));

    // 2. Expired / 404 Fabushi cli-session state must return is_expired() immediately
    let start_expired = client.alipay_start().await.expect("second alipay_start must succeed");
    assert_eq!(start_expired.attempt_id, "att_fb_expired_404");
    let poll_expired = client
        .alipay_poll("att_fb_expired_404")
        .await
        .expect("alipay_poll on 404 state must return expired response");
    assert!(
        poll_expired.is_expired(),
        "HTTP 404 from Fabushi cli-session must be surfaced as expired, got status={}",
        poll_expired.status
    );

    // 3. One-time Fabushi cli-session token resilience when simulate-scan fails transiently on first try
    let start_retry = client.alipay_start().await.expect("third alipay_start must succeed");
    assert_eq!(start_retry.attempt_id, "att_fb_retry_once");
    let poll_retry = client
        .alipay_poll("att_fb_retry_once")
        .await
        .expect("alipay_poll with transient simulate-scan error must recover");
    unsafe {
        std::env::remove_var("FABUSHI_AUTH_BASE_URL");
    }
    assert!(
        poll_retry.is_completed(),
        "One-time Fabushi cli-session must not be lost on transient simulate-scan failure"
    );
    assert_eq!(
        poll_retry.token.as_deref(),
        Some("gpui_token_onetime_88802")
    );
    assert!(
        retry_sync_attempts.load(Ordering::SeqCst) >= 2,
        "Expected simulate-scan retry on transient 503 failure"
    );
}

#[tokio::test]
async fn test_email_send_code_handles_server_epoch_ms_fields_without_overflow() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = vec![0u8; 2048];
            let _ = stream.read(&mut buf).await;
            let body = r#"{"ok":true,"expires_at_ms":1794179710238,"resend_after_ms":1794179170238,"debug_code":"654321"}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes()).await;
        }
    });

    let client = HttpClient::new(format!("http://{}", addr));
    let res = client
        .email_send_code("user@example.com")
        .await
        .expect("email_send_code must not overflow u32 when server returns expires_at_ms/resend_after_ms");
    assert!(res.sent);
    assert_eq!(res.cooldown_seconds, 60);
    assert_eq!(res.expires_in_seconds, 600);
    assert_eq!(res.debug_code.as_deref(), Some("654321"));
}
