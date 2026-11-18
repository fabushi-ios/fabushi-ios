//! Adversarial Challenge Test Suite: Milestone 2 Auth Security & Concurrency Verification
//!
//! Objectives:
//! 1. Token Single-Read Destruction & Replay Prevention
//! 2. Constant-Time Verification (`subtle::ConstantTimeEq`) & OTP Attempt Exhaustion
//! 3. Argon2id Password Hashing Strict Enforcement & Zero Password Hash Leakage Audit
//! 4. 100 Concurrent HTTP Poll Requests Race Condition Stress Harness
//! 5. 100 Concurrent Direct DB Claims Race Condition Stress Harness

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use gpui_telegram_server::{
    auth::{
        hash_password,
        session_manager::{
            generate_verification_code, hash_verification_code, verify_verification_code,
        },
        verify_password,
    },
    db::{OAuthAttemptRecord, ServerDatabase},
    server::{build_router_with_state, AppState},
    ServerConfig,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Barrier;
use tower::ServiceExt;

// ============================================================================
// 1. TOKEN SINGLE-READ DESTRUCTION & REPLAY PREVENTION
// ============================================================================

#[tokio::test]
async fn test_challenge_token_single_read_destruction_and_replay_prevention() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = build_router_with_state(state.clone());

    // Step 1: Initialize Alipay OAuth attempt
    let start_req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/alipay/start")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"device_name": "Adversarial Client"}"#))
        .unwrap();

    let start_res = app.clone().oneshot(start_req).await.unwrap();
    assert_eq!(start_res.status(), StatusCode::OK);
    let start_body = to_bytes(start_res.into_body(), usize::MAX).await.unwrap();
    let start_json: Value = serde_json::from_slice(&start_body).unwrap();
    let attempt_id = start_json["attempt_id"]
        .as_str()
        .expect("attempt_id must be present")
        .to_string();

    // Step 2: Poll while pending - must NOT deliver token
    let poll_pending_req = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/auth/alipay/poll/{}", attempt_id))
        .body(Body::empty())
        .unwrap();
    let poll_pending_res = app.clone().oneshot(poll_pending_req).await.unwrap();
    assert_eq!(poll_pending_res.status(), StatusCode::OK);
    let pending_body = to_bytes(poll_pending_res.into_body(), usize::MAX).await.unwrap();
    let pending_json: Value = serde_json::from_slice(&pending_body).unwrap();
    assert_eq!(pending_json["status"], "pending");
    assert!(pending_json.get("session").is_none());

    // Step 3: Simulate scan to transition to completed
    let sim_req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/alipay/simulate-scan")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "attempt_id": attempt_id,
                "mock_alipay_user_id": "adversarial_user_001",
                "mock_nickname": "Adversary"
            })
            .to_string(),
        ))
        .unwrap();
    let sim_res = app.clone().oneshot(sim_req).await.unwrap();
    assert_eq!(sim_res.status(), StatusCode::OK);

    // Step 4: First poll - MUST retrieve the token
    let poll1_req = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/auth/alipay/poll/{}", attempt_id))
        .body(Body::empty())
        .unwrap();
    let poll1_res = app.clone().oneshot(poll1_req).await.unwrap();
    assert_eq!(poll1_res.status(), StatusCode::OK);
    let poll1_body = to_bytes(poll1_res.into_body(), usize::MAX).await.unwrap();
    let poll1_json: Value = serde_json::from_slice(&poll1_body).unwrap();
    assert_eq!(poll1_json["status"], "completed");
    let token = poll1_json["session"]["token"]
        .as_str()
        .expect("token must be present in first poll")
        .to_string();
    assert!(!token.is_empty(), "Token must not be empty");

    // Step 5: Direct DB check - session_json MUST be wiped to NULL and delivered_at_ms set
    let db_attempt = state
        .db
        .get_oauth_attempt(&attempt_id)
        .expect("DB query must succeed")
        .expect("OAuth attempt must exist in DB");
    assert!(
        db_attempt.session_json.is_none(),
        "session_json in database MUST be wiped (NULL) after first read!"
    );
    assert!(
        db_attempt.delivered_at_ms.is_some(),
        "delivered_at_ms in database MUST be set after first read!"
    );

    // Step 6: Second poll (replay / interception attempt) - MUST return 410 GONE
    let poll2_req = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/auth/alipay/poll/{}", attempt_id))
        .body(Body::empty())
        .unwrap();
    let poll2_res = app.clone().oneshot(poll2_req).await.unwrap();
    assert_eq!(
        poll2_res.status(),
        StatusCode::GONE,
        "Subsequent poll MUST return HTTP 410 GONE"
    );
    let poll2_body = to_bytes(poll2_res.into_body(), usize::MAX).await.unwrap();
    let poll2_json: Value = serde_json::from_slice(&poll2_body).unwrap();
    assert_eq!(poll2_json["error"], "session_already_delivered");

    // Step 7: Third poll - MUST still return 410 GONE
    let poll3_req = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/auth/alipay/poll/{}", attempt_id))
        .body(Body::empty())
        .unwrap();
    let poll3_res = app.clone().oneshot(poll3_req).await.unwrap();
    assert_eq!(poll3_res.status(), StatusCode::GONE);

    // Step 8: Verify the token harvested on first poll is fully valid
    let me_req = Request::builder()
        .method("GET")
        .uri("/api/v1/auth/me")
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let me_res = app.clone().oneshot(me_req).await.unwrap();
    assert_eq!(me_res.status(), StatusCode::OK);
}

// ============================================================================
// 2. CONSTANT-TIME VERIFICATION & OTP ATTEMPTS EXHAUSTION
// ============================================================================

#[tokio::test]
async fn test_challenge_constant_time_otp_verification() {
    // 2.1 Direct verification unit tests
    let code = "741852";
    let hash = hash_verification_code(code);

    // Constant-time check: exact match
    assert!(verify_verification_code("741852", &hash));
    // Whitespace trimming
    assert!(verify_verification_code(" 741852 ", &hash));

    // Adversarial mutations: 1-character differences
    assert!(!verify_verification_code("741853", &hash)); // Differs by 1 in last digit
    assert!(!verify_verification_code("841852", &hash)); // Differs by 1 in first digit
    assert!(!verify_verification_code("741952", &hash)); // Differs in middle
    assert!(!verify_verification_code("041852", &hash));

    // Adversarial length boundaries
    assert!(!verify_verification_code("", &hash));
    assert!(!verify_verification_code("74185", &hash));   // 5 digits
    assert!(!verify_verification_code("7418520", &hash)); // 7 digits
    assert!(!verify_verification_code("abcdef", &hash));
    assert!(!verify_verification_code("741852\0", &hash));
    assert!(!verify_verification_code("741852' OR '1'='1", &hash));

    // Random generation verification
    for _ in 0..100 {
        let gen_code = generate_verification_code();
        assert_eq!(gen_code.len(), 6);
        assert!(gen_code.chars().all(|c| c.is_ascii_digit()));
        let num: u32 = gen_code.parse().unwrap();
        assert!((100_000..=999_999).contains(&num));
    }

    // 2.2 Exhaustion of attempts (Max 5 attempts allowed)
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = build_router_with_state(state.clone());

    let email = "adversarial_exhaust@example.com";

    // Request code
    let send_req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/send-code")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "email": email }).to_string()))
        .unwrap();
    let send_res = app.clone().oneshot(send_req).await.unwrap();
    assert_eq!(send_res.status(), StatusCode::OK);
    let send_body = to_bytes(send_res.into_body(), usize::MAX).await.unwrap();
    let send_json: Value = serde_json::from_slice(&send_body).unwrap();
    let valid_code = send_json["debug_code"]
        .as_str()
        .expect("debug_code available in tests")
        .to_string();

    // Fire 5 incorrect attempts
    for i in 1..=5 {
        let wrong_code = format!("{:06}", i * 11111);
        let bad_req = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/email/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "email": email,
                    "code": wrong_code,
                    "username": "exhaust_target",
                    "password": "Password123#Stress",
                    "first_name": "Stress"
                })
                .to_string(),
            ))
            .unwrap();
        let bad_res = app.clone().oneshot(bad_req).await.unwrap();
        assert_eq!(
            bad_res.status(),
            StatusCode::BAD_REQUEST,
            "Attempt {} should be rejected with 400 Bad Request",
            i
        );
    }

    // 6th attempt with the CORRECT code - MUST be rejected because attempts are exhausted!
    let sixth_req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "email": email,
                "code": valid_code,
                "username": "exhaust_target",
                "password": "Password123#Stress",
                "first_name": "Stress"
            })
            .to_string(),
        ))
        .unwrap();
    let sixth_res = app.clone().oneshot(sixth_req).await.unwrap();
    assert_eq!(
        sixth_res.status(),
        StatusCode::BAD_REQUEST,
        "6th attempt must be rejected even with correct code due to attempt exhaustion"
    );
    let sixth_body = to_bytes(sixth_res.into_body(), usize::MAX).await.unwrap();
    let sixth_json: Value = serde_json::from_slice(&sixth_body).unwrap();
    assert_eq!(sixth_json["error"], "code_expired");
}

// ============================================================================
// 3. ARGON2ID ENFORCEMENT & ZERO PASSWORD HASH LEAKAGE AUDIT
// ============================================================================

#[tokio::test]
async fn test_challenge_argon2id_enforcement_and_zero_password_hash_leakage() {
    let password = "SuperSecureAdversarialPassword!2026";

    // 3.1 Strict Argon2id PHC format
    let hash1 = hash_password(password).expect("Hashing should succeed");
    assert!(
        hash1.starts_with("$argon2id$v=19$"),
        "Hash format MUST strictly be Argon2id v=19"
    );
    assert!(verify_password(password, &hash1));
    assert!(!verify_password("WrongPassword123!", &hash1));
    assert!(!verify_password(&password.to_lowercase(), &hash1));

    // Unique random salt verification
    let hash2 = hash_password(password).expect("Hashing should succeed");
    assert_ne!(
        hash1, hash2,
        "Two hashes of identical passwords must generate distinct salts"
    );

    // Empty password rejection
    assert!(hash_password("").is_err(), "Empty password must be rejected");
    assert!(hash_password("   ").is_err(), "Whitespace-only password must be rejected");

    // 3.2 Full API Responses Leakage Audit
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = build_router_with_state(state.clone());

    // Audit Endpoint 1: POST /api/v1/auth/register
    let reg_req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "username": "audit_user_1",
                "password": password,
                "first_name": "Audit1"
            })
            .to_string(),
        ))
        .unwrap();
    let reg_res = app.clone().oneshot(reg_req).await.unwrap();
    assert_eq!(reg_res.status(), StatusCode::CREATED);
    let reg_body = to_bytes(reg_res.into_body(), usize::MAX).await.unwrap();
    let reg_json: Value = serde_json::from_slice(&reg_body).unwrap();
    assert_no_hash_leakage(&reg_json, "POST /api/v1/auth/register");

    let token = reg_json["token"].as_str().unwrap().to_string();

    // Audit Endpoint 2: POST /api/v1/auth/login
    let login_req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "username": "audit_user_1",
                "password": password
            })
            .to_string(),
        ))
        .unwrap();
    let login_res = app.clone().oneshot(login_req).await.unwrap();
    assert_eq!(login_res.status(), StatusCode::OK);
    let login_body = to_bytes(login_res.into_body(), usize::MAX).await.unwrap();
    let login_json: Value = serde_json::from_slice(&login_body).unwrap();
    assert_no_hash_leakage(&login_json, "POST /api/v1/auth/login");

    // Audit Endpoint 3: GET /api/v1/auth/me
    let me_req = Request::builder()
        .method("GET")
        .uri("/api/v1/auth/me")
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let me_res = app.clone().oneshot(me_req).await.unwrap();
    assert_eq!(me_res.status(), StatusCode::OK);
    let me_body = to_bytes(me_res.into_body(), usize::MAX).await.unwrap();
    let me_json: Value = serde_json::from_slice(&me_body).unwrap();
    assert_no_hash_leakage(&me_json, "GET /api/v1/auth/me");

    // Audit Endpoint 4: POST /api/v1/auth/email/register
    let email = "audit_email@example.com";
    let send_req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/send-code")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "email": email }).to_string()))
        .unwrap();
    let send_res = app.clone().oneshot(send_req).await.unwrap();
    let send_body = to_bytes(send_res.into_body(), usize::MAX).await.unwrap();
    let send_json: Value = serde_json::from_slice(&send_body).unwrap();
    let debug_code = send_json["debug_code"].as_str().unwrap();

    let email_reg_req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "email": email,
                "code": debug_code,
                "username": "audit_email_user",
                "password": password,
                "first_name": "AuditEmail"
            })
            .to_string(),
        ))
        .unwrap();
    let email_reg_res = app.clone().oneshot(email_reg_req).await.unwrap();
    assert_eq!(email_reg_res.status(), StatusCode::CREATED);
    let email_reg_body = to_bytes(email_reg_res.into_body(), usize::MAX).await.unwrap();
    let email_reg_json: Value = serde_json::from_slice(&email_reg_body).unwrap();
    assert_no_hash_leakage(&email_reg_json, "POST /api/v1/auth/email/register");

    // Audit Endpoint 5: GET /api/v1/auth/alipay/poll/:attempt_id
    let alipay_start_req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/alipay/start")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{}"#))
        .unwrap();
    let alipay_start_res = app.clone().oneshot(alipay_start_req).await.unwrap();
    let alipay_start_body = to_bytes(alipay_start_res.into_body(), usize::MAX).await.unwrap();
    let alipay_start_json: Value = serde_json::from_slice(&alipay_start_body).unwrap();
    let attempt_id = alipay_start_json["attempt_id"].as_str().unwrap();

    let scan_req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/alipay/simulate-scan")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "attempt_id": attempt_id }).to_string()))
        .unwrap();
    let _ = app.clone().oneshot(scan_req).await.unwrap();

    let poll_req = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/auth/alipay/poll/{}", attempt_id))
        .body(Body::empty())
        .unwrap();
    let poll_res = app.clone().oneshot(poll_req).await.unwrap();
    let poll_body = to_bytes(poll_res.into_body(), usize::MAX).await.unwrap();
    let poll_json: Value = serde_json::from_slice(&poll_body).unwrap();
    assert_no_hash_leakage(&poll_json, "GET /api/v1/auth/alipay/poll/:attempt_id");
}

fn assert_no_hash_leakage(val: &Value, context: &str) {
    match val {
        Value::Object(map) => {
            for (k, v) in map {
                assert!(
                    k != "password_hash" && k != "password" && k != "secret_hash",
                    "Security violation in {}: key '{}' exposed in JSON response!",
                    context,
                    k
                );
                assert_no_hash_leakage(v, context);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                assert_no_hash_leakage(v, context);
            }
        }
        Value::String(s) => {
            assert!(
                !s.starts_with("$argon2id$"),
                "Security violation in {}: raw Argon2id hash leaked as string value '{}'!",
                context,
                s
            );
        }
        _ => {}
    }
}

// ============================================================================
// 4. 100 CONCURRENT HTTP POLL REQUESTS RACE CONDITION STRESS HARNESS
// ============================================================================

#[tokio::test]
async fn test_challenge_100_concurrent_polls_race_condition() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = build_router_with_state(state.clone());

    // 1. Initiate Alipay OAuth attempt
    let start_req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/alipay/start")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{}"#))
        .unwrap();
    let start_res = app.clone().oneshot(start_req).await.unwrap();
    assert_eq!(start_res.status(), StatusCode::OK);
    let start_body = to_bytes(start_res.into_body(), usize::MAX).await.unwrap();
    let start_json: Value = serde_json::from_slice(&start_body).unwrap();
    let attempt_id = start_json["attempt_id"].as_str().unwrap().to_string();

    // 2. Transition attempt to completed
    let sim_req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/alipay/simulate-scan")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "attempt_id": attempt_id,
                "mock_alipay_user_id": "concurrent_alipay_100",
                "mock_nickname": "Concurrent Winner"
            })
            .to_string(),
        ))
        .unwrap();
    let sim_res = app.clone().oneshot(sim_req).await.unwrap();
    assert_eq!(sim_res.status(), StatusCode::OK);

    // 3. Fire 100 concurrent poll requests using a synchronized barrier
    const CONCURRENCY: usize = 100;
    let barrier = Arc::new(Barrier::new(CONCURRENCY + 1));
    let mut handles = Vec::with_capacity(CONCURRENCY);

    for task_idx in 0..CONCURRENCY {
        let app_clone = app.clone();
        let attempt_id_clone = attempt_id.clone();
        let barrier_clone = barrier.clone();

        let handle = tokio::spawn(async move {
            // Synchronize all tasks to hit at the exact same millisecond
            barrier_clone.wait().await;

            let req = Request::builder()
                .method("GET")
                .uri(format!("/api/v1/auth/alipay/poll/{}", attempt_id_clone))
                .body(Body::empty())
                .unwrap();

            let res = app_clone.oneshot(req).await.unwrap();
            let status = res.status();
            let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
            let json_body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);

            (task_idx, status, json_body)
        });

        handles.push(handle);
    }

    // Release all 100 tasks simultaneously
    barrier.wait().await;

    // 4. Collect and analyze all 100 responses
    let mut ok_responses = Vec::new();
    let mut gone_responses = Vec::new();
    let mut other_responses = Vec::new();

    for handle in handles {
        let (task_idx, status, body) = handle.await.expect("Task must not panic");
        if status == StatusCode::OK {
            ok_responses.push((task_idx, body));
        } else if status == StatusCode::GONE {
            gone_responses.push((task_idx, body));
        } else {
            other_responses.push((task_idx, status, body));
        }
    }

    // Empirical Assertions:
    assert_eq!(
        other_responses.len(),
        0,
        "No unexpected error statuses or server panics allowed, got {:?}",
        other_responses
    );

    assert_eq!(
        ok_responses.len(),
        1,
        "CRITICAL RACE CONDITION: Exactly 1 concurrent request must obtain the token, but {} succeeded!",
        ok_responses.len()
    );

    assert_eq!(
        gone_responses.len(),
        99,
        "CRITICAL RACE CONDITION: Exactly 99 concurrent requests must receive 410 GONE, but {} received GONE!",
        gone_responses.len()
    );

    // 5. Verify the winner's token
    let (_winner_idx, winner_body) = &ok_responses[0];
    assert_eq!(winner_body["status"], "completed");
    let winner_token = winner_body["session"]["token"]
        .as_str()
        .expect("Winner must receive a valid token string");
    assert!(!winner_token.is_empty());

    // Verify token can authenticate
    let me_req = Request::builder()
        .method("GET")
        .uri("/api/v1/auth/me")
        .header(header::AUTHORIZATION, format!("Bearer {}", winner_token))
        .body(Body::empty())
        .unwrap();
    let me_res = app.clone().oneshot(me_req).await.unwrap();
    assert_eq!(me_res.status(), StatusCode::OK);

    // 6. Verify subsequent 101st request is also 410 GONE
    let poll_101_req = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/auth/alipay/poll/{}", attempt_id))
        .body(Body::empty())
        .unwrap();
    let poll_101_res = app.clone().oneshot(poll_101_req).await.unwrap();
    assert_eq!(poll_101_res.status(), StatusCode::GONE);
}

// ============================================================================
// 5. 100 CONCURRENT DIRECT DB CLAIMS RACE CONDITION STRESS HARNESS
// ============================================================================

#[tokio::test]
async fn test_challenge_100_concurrent_db_claims_direct() {
    let db = Arc::new(ServerDatabase::open_in_memory().expect("In-memory SQLite setup"));
    let attempt_id = "test_concurrent_attempt_db_100";
    let now_ms = chrono::Utc::now().timestamp_millis();

    // Insert completed attempt with session_json
    let session_payload = json!({
        "user_id": 100_099,
        "token": "sess_100.secret_token_12345"
    })
    .to_string();

    let attempt = OAuthAttemptRecord {
        attempt_id: attempt_id.to_string(),
        provider: "alipay".into(),
        state: "state_100".into(),
        status: "completed".into(),
        session_json: Some(session_payload.clone()),
        delivered_at_ms: None,
        expires_at_ms: now_ms + 600_000,
        created_at_ms: now_ms,
    };
    db.create_oauth_attempt(&attempt)
        .expect("Create attempt must succeed");

    const CONCURRENCY: usize = 100;
    let barrier = Arc::new(Barrier::new(CONCURRENCY + 1));
    let mut handles = Vec::with_capacity(CONCURRENCY);

    for task_idx in 0..CONCURRENCY {
        let db_clone = db.clone();
        let barrier_clone = barrier.clone();
        let delivered_at = now_ms + (task_idx as i64);

        let handle = tokio::spawn(async move {
            barrier_clone.wait().await;
            db_clone.claim_oauth_attempt_session(attempt_id, delivered_at)
        });
        handles.push(handle);
    }

    // Release all 100 DB claims at once
    barrier.wait().await;

    let mut successes = 0;
    let mut nones = 0;

    for handle in handles {
        let result = handle.await.expect("Task must not panic");
        match result {
            Ok(Some(claimed_str)) => {
                assert_eq!(claimed_str, session_payload);
                successes += 1;
            }
            Ok(None) => {
                nones += 1;
            }
            Err(e) => {
                panic!("DB error during concurrent claim: {}", e);
            }
        }
    }

    assert_eq!(
        successes, 1,
        "Direct DB race: exactly 1 thread must claim the session token, got {}",
        successes
    );
    assert_eq!(
        nones, 99,
        "Direct DB race: exactly 99 threads must receive None, got {}",
        nones
    );

    // Final verification of DB row
    let final_record = db
        .get_oauth_attempt(attempt_id)
        .expect("Query must succeed")
        .expect("Attempt must exist");
    assert!(
        final_record.session_json.is_none(),
        "session_json must be NULL"
    );
    assert!(
        final_record.delivered_at_ms.is_some(),
        "delivered_at_ms must be non-null"
    );
}
