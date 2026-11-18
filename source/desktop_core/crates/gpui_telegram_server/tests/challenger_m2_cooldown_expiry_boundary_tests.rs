//! Challenger M2 Empirical Stress & Boundary Condition Tests (`crates/gpui_telegram_server`).
//!
//! Objectives:
//! 1. Cooldown Enforcement: Request code twice within 60s -> 429 Too Many Requests.
//! 2. Max Attempt Limiting: Submit 5 wrong 6-digit codes -> 6th submission fails with max attempts reached.
//! 3. Expiry Boundaries:
//!    - Challenge code older than 600s strictly rejected as expired.
//!    - Alipay attempt older than 600s strictly transitions to "expired".

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use chrono::Utc;
use gpui_telegram_server::{
    config::ServerConfig,
    server::{build_router_with_state, AppState},
};
use rusqlite::params;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn test_cooldown_enforcement_60s() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = build_router_with_state(state.clone());
    let email = "cooldown_test_user@example.com";

    // 1. Initial code request must succeed with 200 OK
    let req1 = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/send-code")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "email": email }).to_string()))
        .unwrap();

    let res1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(res1.status(), StatusCode::OK, "First send-code request must succeed with 200 OK");
    let body1 = to_bytes(res1.into_body(), usize::MAX).await.unwrap();
    let json1: Value = serde_json::from_slice(&body1).unwrap();
    assert_eq!(json1["ok"], true);
    assert!(json1["resend_after_ms"].is_number());
    assert!(json1["expires_at_ms"].is_number());

    // 2. Immediate second request within 60s MUST be rejected with HTTP 429 Too Many Requests
    let req2 = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/send-code")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "email": email }).to_string()))
        .unwrap();

    let res2 = app.clone().oneshot(req2).await.unwrap();
    assert_eq!(
        res2.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "Second send-code request within 60s must be rejected with 429 Too Many Requests"
    );
    let body2 = to_bytes(res2.into_body(), usize::MAX).await.unwrap();
    let json2: Value = serde_json::from_slice(&body2).unwrap();
    assert_eq!(json2["error"], "rate_limited");
    assert_eq!(json2["message"], "验证码发送过于频繁，请稍后再试");

    // 3. Independent email should NOT be blocked by the first email's cooldown
    let req_other = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/send-code")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "email": "other_independent_user@example.com" }).to_string()))
        .unwrap();

    let res_other = app.clone().oneshot(req_other).await.unwrap();
    assert_eq!(
        res_other.status(),
        StatusCode::OK,
        "Different email should not be blocked by first email's cooldown"
    );

    // 4. Fast-forward time past 60s cooldown in DB
    let now_ms = Utc::now().timestamp_millis();
    state.db.pool().with_conn(|conn| {
        conn.execute(
            "UPDATE account_email_challenges SET cooldown_until_ms = ?1 WHERE email = ?2",
            params![now_ms - 1000, email],
        )
        .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
    }).unwrap();

    // 5. After cooldown expires, request should succeed again
    let req3 = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/send-code")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "email": email }).to_string()))
        .unwrap();

    let res3 = app.clone().oneshot(req3).await.unwrap();
    assert_eq!(res3.status(), StatusCode::OK, "Send-code after cooldown expiry must succeed with 200 OK");
}

#[tokio::test]
async fn test_max_attempt_limiting_5_failures_then_invalidation() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = build_router_with_state(state.clone());
    let email = "attempt_limit_user@example.com";

    // 1. Request verification code
    let req_send = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/send-code")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "email": email }).to_string()))
        .unwrap();

    let res_send = app.clone().oneshot(req_send).await.unwrap();
    assert_eq!(res_send.status(), StatusCode::OK);
    let body_send = to_bytes(res_send.into_body(), usize::MAX).await.unwrap();
    let json_send: Value = serde_json::from_slice(&body_send).unwrap();
    let correct_code = json_send["debug_code"]
        .as_str()
        .expect("debug_code must be provided in test builds")
        .to_string();

    // 2. Submit 5 wrong 6-digit codes consecutively
    let wrong_codes = ["111111", "222222", "333333", "444444", "555555"];
    for (idx, wrong_code) in wrong_codes.iter().enumerate() {
        let attempt_num = idx + 1;
        let req_wrong = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/email/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "email": email,
                    "code": wrong_code,
                    "username": format!("attempt_user_{}", attempt_num),
                    "password": "Password123456!",
                    "first_name": "AttemptUser",
                    "device_name": "TestDevice"
                })
                .to_string(),
            ))
            .unwrap();

        let res_wrong = app.clone().oneshot(req_wrong).await.unwrap();
        assert_eq!(
            res_wrong.status(),
            StatusCode::BAD_REQUEST,
            "Wrong code submission #{} must return 400 Bad Request",
            attempt_num
        );
        let body_wrong = to_bytes(res_wrong.into_body(), usize::MAX).await.unwrap();
        let json_wrong: Value = serde_json::from_slice(&body_wrong).unwrap();
        assert_eq!(
            json_wrong["error"], "invalid_code",
            "Attempts 1..=5 must return error=invalid_code"
        );
        assert_eq!(
            json_wrong["message"], "验证码不正确",
            "Attempts 1..=5 must indicate code is incorrect"
        );

        // Verify remaining attempts in database decrements monotonically
        let challenge = state.db.get_latest_email_challenge(email).unwrap().unwrap();
        let expected_remaining = (5 - attempt_num) as i32;
        assert_eq!(
            challenge.attempts_left, expected_remaining,
            "After attempt #{}, attempts_left must be {}",
            attempt_num, expected_remaining
        );
    }

    // 3. The 6th submission with wrong code MUST fail with challenge invalidated / max attempts reached
    let req_6th = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "email": email,
                "code": "666666",
                "username": "attempt_user_6",
                "password": "Password123456!",
                "first_name": "AttemptUser",
                "device_name": "TestDevice"
            })
            .to_string(),
        ))
        .unwrap();

    let res_6th = app.clone().oneshot(req_6th).await.unwrap();
    assert_eq!(
        res_6th.status(),
        StatusCode::BAD_REQUEST,
        "6th submission must return 400 Bad Request"
    );
    let body_6th = to_bytes(res_6th.into_body(), usize::MAX).await.unwrap();
    let json_6th: Value = serde_json::from_slice(&body_6th).unwrap();
    assert_eq!(
        json_6th["error"], "code_expired",
        "6th attempt must report code_expired due to max attempts exhausted"
    );
    assert_eq!(
        json_6th["message"], "验证码尝试次数过多，请重新获取",
        "6th attempt message must notify that max attempts have been exceeded"
    );

    // 4. Even submitting the CORRECT code after exhaustion MUST be rejected
    let req_correct_after_exhaust = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "email": email,
                "code": correct_code,
                "username": "attempt_user_correct",
                "password": "Password123456!",
                "first_name": "AttemptUser",
                "device_name": "TestDevice"
            })
            .to_string(),
        ))
        .unwrap();

    let res_correct_after_exhaust = app.clone().oneshot(req_correct_after_exhaust).await.unwrap();
    assert_eq!(
        res_correct_after_exhaust.status(),
        StatusCode::BAD_REQUEST,
        "Correct code after attempt exhaustion must still be rejected"
    );
    let body_exhaust = to_bytes(res_correct_after_exhaust.into_body(), usize::MAX).await.unwrap();
    let json_exhaust: Value = serde_json::from_slice(&body_exhaust).unwrap();
    assert_eq!(
        json_exhaust["error"], "code_expired",
        "Challenge must remain permanently invalidated after max attempts exceeded"
    );
    assert_eq!(json_exhaust["message"], "验证码尝试次数过多，请重新获取");
}

#[tokio::test]
async fn test_challenge_code_older_than_600s_rejected_as_expired() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = build_router_with_state(state.clone());
    let email = "expired_user@example.com";

    // 1. Request verification code
    let req_send = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/send-code")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "email": email }).to_string()))
        .unwrap();

    let res_send = app.clone().oneshot(req_send).await.unwrap();
    assert_eq!(res_send.status(), StatusCode::OK);
    let body_send = to_bytes(res_send.into_body(), usize::MAX).await.unwrap();
    let json_send: Value = serde_json::from_slice(&body_send).unwrap();
    let correct_code = json_send["debug_code"]
        .as_str()
        .expect("debug_code must be provided in test builds")
        .to_string();

    // 2. Age challenge code past 600s TTL (expires_at_ms in the past)
    let now_ms = Utc::now().timestamp_millis();
    state.db.pool().with_conn(|conn| {
        conn.execute(
            "UPDATE account_email_challenges SET expires_at_ms = ?1 WHERE email = ?2",
            params![now_ms - 1000, email],
        )
        .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
    }).unwrap();

    // 3. Registration attempt with the correct code must be strictly rejected as expired
    let req_register = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "email": email,
                "code": correct_code,
                "username": "expired_user_test",
                "password": "Password123456!",
                "first_name": "ExpiredUser",
                "device_name": "TestDevice"
            })
            .to_string(),
        ))
        .unwrap();

    let res_register = app.clone().oneshot(req_register).await.unwrap();
    assert_eq!(
        res_register.status(),
        StatusCode::BAD_REQUEST,
        "Registration with expired challenge code (>600s) must return 400 Bad Request"
    );
    let body_reg = to_bytes(res_register.into_body(), usize::MAX).await.unwrap();
    let json_reg: Value = serde_json::from_slice(&body_reg).unwrap();
    assert_eq!(
        json_reg["error"], "code_expired",
        "Error code must be code_expired"
    );
    assert_eq!(
        json_reg["message"], "验证码已失效，请重新获取",
        "Error message must indicate challenge code is expired"
    );
}

#[tokio::test]
async fn test_alipay_attempt_older_than_600s_transitions_to_expired() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = build_router_with_state(state.clone());

    // 1. Start an Alipay OAuth attempt
    let req_start = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/alipay/start")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"device_name": "Test Client"}"#))
        .unwrap();

    let res_start = app.clone().oneshot(req_start).await.unwrap();
    assert_eq!(res_start.status(), StatusCode::OK);
    let body_start = to_bytes(res_start.into_body(), usize::MAX).await.unwrap();
    let json_start: Value = serde_json::from_slice(&body_start).unwrap();
    let attempt_id = json_start["attempt_id"].as_str().expect("attempt_id must be returned").to_string();

    // 2. Initial poll shows "pending" status
    let req_poll_initial = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/auth/alipay/poll/{}", attempt_id))
        .body(Body::empty())
        .unwrap();

    let res_poll_initial = app.clone().oneshot(req_poll_initial).await.unwrap();
    assert_eq!(res_poll_initial.status(), StatusCode::OK);
    let body_init = to_bytes(res_poll_initial.into_body(), usize::MAX).await.unwrap();
    let json_init: Value = serde_json::from_slice(&body_init).unwrap();
    assert_eq!(json_init["status"], "pending");

    // 3. Fast-forward expiration: set expires_at_ms to past (>600s old)
    let now_ms = Utc::now().timestamp_millis();
    state.db.pool().with_conn(|conn| {
        conn.execute(
            "UPDATE account_oauth_attempts SET expires_at_ms = ?1 WHERE attempt_id = ?2",
            params![now_ms - 1000, attempt_id],
        )
        .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
    }).unwrap();

    // 4. Poll again: must transition to "expired"
    let req_poll_expired = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/auth/alipay/poll/{}", attempt_id))
        .body(Body::empty())
        .unwrap();

    let res_poll_expired = app.clone().oneshot(req_poll_expired).await.unwrap();
    assert_eq!(
        res_poll_expired.status(),
        StatusCode::OK,
        "Polling an expired attempt returns 200 with status=expired"
    );
    let body_exp = to_bytes(res_poll_expired.into_body(), usize::MAX).await.unwrap();
    let json_exp: Value = serde_json::from_slice(&body_exp).unwrap();
    assert_eq!(
        json_exp["status"], "expired",
        "OAuth attempt older than 600s must report status: expired"
    );

    // 5. Verify the SQLite table status was persisted as "expired"
    let attempt_record = state.db.get_oauth_attempt(&attempt_id).unwrap().unwrap();
    assert_eq!(
        attempt_record.status, "expired",
        "OAuth attempt status in SQLite must transition to 'expired'"
    );
}
