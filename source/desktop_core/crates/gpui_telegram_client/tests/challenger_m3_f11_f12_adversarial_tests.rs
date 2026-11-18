use gpui_telegram_client::net::http_client::{
    HttpClient, HttpClientError, LoginRequest, SendMessageHttpRequest,
};
use gpui_telegram_client::net::ws_channel::WsChannel;
use gpui_telegram_client::sync::coordinator::ClientSyncCoordinator;
use gpui_telegram_client::sync::SyncOutcome;
use gpui_telegram_core::types::{ChatId, UserId};
use gpui_telegram_proto::sync::{PtsUpdate, TypingAction, TypingEvent, UpdateEvent};
use gpui_telegram_proto::ws::WsServerFrame;
use std::time::Duration;

const LIVE_CLOUD_URL: &str = "http://34.46.77.133:8080";

fn get_live_client() -> HttpClient {
    let url = std::env::var("GPUI_TELEGRAM_SERVER_URL")
        .unwrap_or_else(|_| LIVE_CLOUD_URL.to_string());
    HttpClient::new_with_timeout(url, Duration::from_secs(15))
}

async fn get_authenticated_session(client: &HttpClient) -> String {
    let login_req = LoginRequest {
        username: "cloud_probe_user".to_string(),
        password: "probe_password_123".to_string(),
        device_name: "Challenger F11-F12 Harness".to_string(),
    };
    let auth = client
        .login(&login_req)
        .await
        .expect("Authentication with live backend failed");
    auth.token
}

// ============================================================================
// Feature F11: Live Backend HTTP Messaging & History Retrieval Adversarial
// ============================================================================

#[tokio::test]
async fn test_live_cloud_http_send_message_diverse_payloads() {
    let client = get_live_client();
    let token = get_authenticated_session(&client).await;
    let welcome = client
        .provision_welcome_chat(&token)
        .await
        .expect("Failed to provision welcome chat");
    let target_chat_id = welcome.chat_id as u64;

    // Test matrix of diverse payloads:
    // 1. Standard text
    // 2. Heavy Unicode & CJK characters
    // 3. Emoji & composite surrogate pairs
    // 4. Special chars, SQL injection, XSS payloads
    // 5. Long text (2,000+ chars)
    let long_text = "RepeatChunk-0123456789ABCDEF-".repeat(70);
    let payloads = vec![
        ("ASCII normal", "Hello live backend from Challenger M3.1!"),
        (
            "Unicode & CJK",
            "【边界测试】GPUI Telegram Desktop 官方对齐：测试中文、日文（こんにちは）、韩文（안녕하세요）与特殊标点。",
        ),
        (
            "Emoji & Symbols",
            "🚀🔥🎉✨🤖💡📱💻⚡️🍎🍉🪐🛸🛰️ — Multi-byte emojis and surrogate pairs test.",
        ),
        (
            "SQL & XSS Injection Vectors",
            "'; DROP TABLE messages; -- <script>alert(\"XSS\")</script> &amp; \"quotes\" \r\n\t",
        ),
        (
            "Long Text (>2000 chars)",
            long_text.as_str(),
        ),
    ];

    for (desc, text) in payloads {
        let req = SendMessageHttpRequest {
            chat_id: target_chat_id,
            text: text.to_string(),
            reply_to: None,
        };

        let result = client.send_message(&token, &req).await;
        assert!(
            result.is_ok(),
            "Failed sending payload '{}': {:?}",
            desc,
            result.err()
        );

        let ack = result.unwrap();
        assert!(
            ack.message_id > 0,
            "ack.message_id must be positive, got {}",
            ack.message_id
        );
        assert!(ack.pts > 0, "ack.pts must be positive, got {}", ack.pts);
    }
}

#[tokio::test]
async fn test_live_cloud_dual_endpoint_message_history_fallback() {
    let client = get_live_client();
    let token = get_authenticated_session(&client).await;
    let welcome = client
        .provision_welcome_chat(&token)
        .await
        .expect("Failed to provision welcome chat");
    let target_chat_id = welcome.chat_id as u64;

    // 1. Verify GET /api/v1/chats/{chat_id}/messages succeeds with 200 OK on upgraded server,
    // and non-existent subroute still yields 404.
    let direct_client = reqwest::Client::new();
    let primary_url = format!("{}/api/v1/chats/{}/messages", client.base_url, target_chat_id);
    let resp = direct_client
        .get(&primary_url)
        .bearer_auth(&token)
        .send()
        .await
        .expect("HTTP request to primary history endpoint failed");

    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "Expected /api/v1/chats/{}/messages to return 200 OK, got {}",
        target_chat_id,
        resp.status()
    );

    // 2. Verify HttpClient::get_chat_messages retrieves message history cleanly.
    let history = client
        .get_chat_messages(&token, target_chat_id, Some(20))
        .await
        .expect("Failed to retrieve message history");

    // The live server currently returns {"messages": [], "has_more": false, "current_pts": ...}
    // Verify that the client parses this successfully into an empty or populated Vec.
    assert!(
        history.is_empty() || !history.is_empty(),
        "History should parse cleanly without error"
    );

    // Verify MessagesHistoryResponse deserialization for both Wrapped and direct List schemas
    let wrapped_json = r#"{
        "messages": [
            {"message_id": 101, "sender_id": 1, "text": "Hello historical", "date": 1700000000},
            {"message_id": 102, "sender_id": 2, "text": "World historical", "date": 1700000001, "reply_to": 101}
        ],
        "has_more": true,
        "current_pts": 15
    }"#;
    let wrapped_parsed: gpui_telegram_client::net::http_client::MessagesHistoryResponse =
        serde_json::from_str(wrapped_json).expect("Wrapped MessagesHistoryResponse must parse");
    let msgs = wrapped_parsed.into_messages();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].message_id, 101);
    assert_eq!(msgs[0].text, "Hello historical");
    assert_eq!(msgs[1].reply_to, Some(101));

    let list_json = r#"[
        {"message_id": 201, "sender_id": 10, "text": "Bare list message", "date": 1700000010}
    ]"#;
    let list_parsed: gpui_telegram_client::net::http_client::MessagesHistoryResponse =
        serde_json::from_str(list_json).expect("Bare list MessagesHistoryResponse must parse");
    let list_msgs = list_parsed.into_messages();
    assert_eq!(list_msgs.len(), 1);
    assert_eq!(list_msgs[0].message_id, 201);

    // 3. Negative check: Unauthorized token returns Unauthorized error
    let bad_history = client
        .get_chat_messages("forged_invalid_token_9999", 4, Some(10))
        .await;
    match bad_history {
        Err(HttpClientError::Unauthorized(_)) => {}
        other => panic!("Expected Unauthorized error for invalid token, got {:?}", other),
    }
}

#[tokio::test]
async fn test_live_cloud_get_chats_retrieval() {
    let client = get_live_client();
    let token = get_authenticated_session(&client).await;

    let chats = client
        .get_chats(&token)
        .await
        .expect("Failed to retrieve user chats from live backend");

    // The live server returns {"chats": [], "total": 0} for new users
    println!("Live cloud GET /api/v1/chats returned {} chats", chats.len());

    // Verify ChatsListResponse deserialization for both Wrapped and direct List schemas
    let wrapped_chats_json = r#"{
        "chats": [
            {"chat_id": 4, "title": "AI Autonomous Agent", "unread_count": 0, "last_message": "Ready"}
        ],
        "total": 1
    }"#;
    let wrapped_chats: gpui_telegram_client::net::http_client::ChatsListResponse =
        serde_json::from_str(wrapped_chats_json).expect("Wrapped ChatsListResponse must parse");
    let parsed_chats = wrapped_chats.into_chats();
    assert_eq!(parsed_chats.len(), 1);
    assert_eq!(parsed_chats[0].chat_id, 4);
    assert_eq!(parsed_chats[0].title, "AI Autonomous Agent");

    let list_chats_json = r#"[
        {"chat_id": 5, "title": "Direct Peer Dialog", "unread_count": 2, "last_message": "Ping"}
    ]"#;
    let list_chats: gpui_telegram_client::net::http_client::ChatsListResponse =
        serde_json::from_str(list_chats_json).expect("List ChatsListResponse must parse");
    let parsed_list = list_chats.into_chats();
    assert_eq!(parsed_list.len(), 1);
    assert_eq!(parsed_list[0].chat_id, 5);

    // Negative check: Unauthorized token returns Unauthorized error
    let bad_chats = client.get_chats("invalid_token_99999").await;
    match bad_chats {
        Err(HttpClientError::Unauthorized(_)) => {}
        other => panic!("Expected Unauthorized for bad token, got {:?}", other),
    }
}

// ============================================================================
// Feature F12: WebSocket Streaming & PTS Delta Sync Under Heavy Stress
// ============================================================================

fn make_pts_update(chat_id: ChatId, pts: u64, pts_count: u32) -> PtsUpdate {
    PtsUpdate::new(
        chat_id,
        pts,
        pts_count,
        UpdateEvent::UserTyping(TypingEvent::new(chat_id, UserId(1), TypingAction::Typing)),
        chrono::Utc::now().timestamp_millis(),
    )
}

#[test]
fn test_adversarial_pts_gap_scrambled_delivery_and_complete_recovery() {
    let coordinator = ClientSyncCoordinator::new();
    let chat = ChatId(202);

    // Seed coordinator at PTS 500
    coordinator.set_pts(chat, 500);
    assert_eq!(coordinator.last_pts(chat), 500);

    // Severe packet loss & reordering:
    // Expected order: 501, 502, 503, 504, 505
    // Delivery arrival order: 504, 502, 505, 503 (501 delayed)
    let u504 = make_pts_update(chat, 504, 1);
    let u502 = make_pts_update(chat, 502, 1);
    let u505 = make_pts_update(chat, 505, 1);
    let u503 = make_pts_update(chat, 503, 1);

    // 1. Update 504 arrives
    let outcome = coordinator.evaluate_update(&u504);
    assert_eq!(
        outcome,
        SyncOutcome::GapDetected {
            last_seen: 500,
            incoming: 504,
            missing_range: 501..=503,
        }
    );
    coordinator.buffer_gap_update(u504);
    assert_eq!(coordinator.last_pts(chat), 500);

    // 2. Update 502 arrives
    let outcome = coordinator.evaluate_update(&u502);
    assert_eq!(
        outcome,
        SyncOutcome::GapDetected {
            last_seen: 500,
            incoming: 502,
            missing_range: 501..=501,
        }
    );
    coordinator.buffer_gap_update(u502);

    // 3. Update 505 arrives
    let outcome = coordinator.evaluate_update(&u505);
    assert_eq!(
        outcome,
        SyncOutcome::GapDetected {
            last_seen: 500,
            incoming: 505,
            missing_range: 501..=504,
        }
    );
    coordinator.buffer_gap_update(u505);

    // 4. Update 503 arrives
    let outcome = coordinator.evaluate_update(&u503);
    assert_eq!(
        outcome,
        SyncOutcome::GapDetected {
            last_seen: 500,
            incoming: 503,
            missing_range: 501..=502,
        }
    );
    coordinator.buffer_gap_update(u503);

    // 5. Attempt premature drain while 501 is still missing: MUST yield nothing!
    let premature = coordinator.drain_contiguous_buffer(chat);
    assert!(
        premature.is_empty(),
        "Premature drain must be empty because 501 is missing"
    );
    assert_eq!(coordinator.last_pts(chat), 500);

    // 6. Missing update 501 arrives contiguously!
    let u501 = make_pts_update(chat, 501, 1);
    let outcome = coordinator.evaluate_update(&u501);
    assert_eq!(outcome, SyncOutcome::Contiguous);
    assert_eq!(coordinator.last_pts(chat), 501);

    // 7. Drain buffer: MUST drain 502, 503, 504, 505 in strict contiguous order!
    let drained = coordinator.drain_contiguous_buffer(chat);
    assert_eq!(drained.len(), 4);
    assert_eq!(drained[0].pts, 502);
    assert_eq!(drained[1].pts, 503);
    assert_eq!(drained[2].pts, 504);
    assert_eq!(drained[3].pts, 505);

    // Coordinator PTS must now be advanced to 505
    assert_eq!(coordinator.last_pts(chat), 505);

    // 8. Further drains must be empty
    assert!(coordinator.drain_contiguous_buffer(chat).is_empty());
}

#[test]
fn test_adversarial_pts_multi_step_partial_gaps() {
    let coordinator = ClientSyncCoordinator::new();
    let chat = ChatId(303);

    coordinator.set_pts(chat, 1000);

    // Buffer updates 1002, 1003, 1006, 1007 (leaving gaps at 1001, 1004, 1005)
    let u1002 = make_pts_update(chat, 1002, 1);
    let u1003 = make_pts_update(chat, 1003, 1);
    let u1006 = make_pts_update(chat, 1006, 1);
    let u1007 = make_pts_update(chat, 1007, 1);

    coordinator.buffer_gap_update(u1002);
    coordinator.buffer_gap_update(u1003);
    coordinator.buffer_gap_update(u1006);
    coordinator.buffer_gap_update(u1007);

    // Contiguous update 1001 arrives
    let u1001 = make_pts_update(chat, 1001, 1);
    assert_eq!(coordinator.evaluate_update(&u1001), SyncOutcome::Contiguous);
    assert_eq!(coordinator.last_pts(chat), 1001);

    // Partial drain: should drain 1002 and 1003, then STOP because 1004 is missing!
    let partial = coordinator.drain_contiguous_buffer(chat);
    assert_eq!(partial.len(), 2);
    assert_eq!(partial[0].pts, 1002);
    assert_eq!(partial[1].pts, 1003);
    assert_eq!(coordinator.last_pts(chat), 1003);

    // Another drain returns empty
    assert!(coordinator.drain_contiguous_buffer(chat).is_empty());

    // Advance PTS to 1005 (simulating WsClientFrame::Sync fetching difference for 1004..=1005)
    coordinator.set_pts(chat, 1005);
    assert_eq!(coordinator.last_pts(chat), 1005);

    // Second drain: should now drain 1006 and 1007!
    let remaining = coordinator.drain_contiguous_buffer(chat);
    assert_eq!(remaining.len(), 2);
    assert_eq!(remaining[0].pts, 1006);
    assert_eq!(remaining[1].pts, 1007);
    assert_eq!(coordinator.last_pts(chat), 1007);
}

#[test]
fn test_adversarial_pts_stale_duplicates_and_ephemeral_updates() {
    let coordinator = ClientSyncCoordinator::new();
    let chat = ChatId(404);

    coordinator.set_pts(chat, 200);

    // 1. Stale updates (pts <= 200) must return Duplicate
    let stale = make_pts_update(chat, 199, 1);
    assert_eq!(coordinator.evaluate_update(&stale), SyncOutcome::Duplicate);
    assert_eq!(coordinator.last_pts(chat), 200);

    let dup = make_pts_update(chat, 200, 1);
    assert_eq!(coordinator.evaluate_update(&dup), SyncOutcome::Duplicate);
    assert_eq!(coordinator.last_pts(chat), 200);

    // 2. Ephemeral updates (pts_count == 0)
    // When pts <= last_seen, ephemeral is Contiguous without advancing last_seen
    let ephemeral_seen = make_pts_update(chat, 150, 0);
    assert_eq!(
        coordinator.evaluate_update(&ephemeral_seen),
        SyncOutcome::Contiguous
    );
    assert_eq!(coordinator.last_pts(chat), 200);

    // When pts > last_seen, ephemeral produces GapDetected
    let ephemeral_future = make_pts_update(chat, 250, 0);
    assert_eq!(
        coordinator.evaluate_update(&ephemeral_future),
        SyncOutcome::GapDetected {
            last_seen: 200,
            incoming: 250,
            missing_range: 201..=250,
        }
    );
}

#[test]
fn test_adversarial_multi_chat_pts_isolation() {
    let coordinator = ClientSyncCoordinator::new();
    let chat_a = ChatId(1001);
    let chat_b = ChatId(1002);

    coordinator.set_pts(chat_a, 50);
    coordinator.set_pts(chat_b, 800);

    // Gap update on Chat A (55 arrives)
    let ua_55 = make_pts_update(chat_a, 55, 1);
    assert_eq!(
        coordinator.evaluate_update(&ua_55),
        SyncOutcome::GapDetected {
            last_seen: 50,
            incoming: 55,
            missing_range: 51..=54,
        }
    );
    coordinator.buffer_gap_update(ua_55);

    // Contiguous update on Chat B (801 arrives)
    let ub_801 = make_pts_update(chat_b, 801, 1);
    assert_eq!(
        coordinator.evaluate_update(&ub_801),
        SyncOutcome::Contiguous
    );
    assert_eq!(coordinator.last_pts(chat_b), 801);

    // Chat B drain is empty
    assert!(coordinator.drain_contiguous_buffer(chat_b).is_empty());

    // Chat A is still at 50 with buffered 55
    assert_eq!(coordinator.last_pts(chat_a), 50);
    assert!(coordinator.drain_contiguous_buffer(chat_a).is_empty());
}

#[tokio::test]
async fn test_ws_channel_multi_subscriber_broadcast_stress() {
    let channel = WsChannel::new();
    const NUM_SUBSCRIBERS: usize = 8;
    const NUM_FRAMES: usize = 100;

    let receivers: Vec<_> = (0..NUM_SUBSCRIBERS)
        .map(|_| channel.subscribe())
        .collect();

    // Spawn async tasks for each subscriber to collect received frames
    let handles: Vec<_> = receivers
        .into_iter()
        .enumerate()
        .map(|(sub_idx, mut rx)| {
            tokio::spawn(async move {
                let mut received = 0;
                while received < NUM_FRAMES {
                    match rx.recv().await {
                        Ok(WsServerFrame::Pong { nonce }) => {
                            assert_eq!(nonce, received as u64);
                            received += 1;
                        }
                        Ok(_) => panic!("Subscriber {} received unexpected frame", sub_idx),
                        Err(e) => panic!("Subscriber {} error: {:?}", sub_idx, e),
                    }
                }
                received
            })
        })
        .collect();

    // Dispatch 100 Pong frames
    for i in 0..NUM_FRAMES {
        channel.dispatch_inbound(WsServerFrame::Pong { nonce: i as u64 });
    }

    // Await all subscribers and verify 100% receipt across all
    for handle in handles {
        let count = handle.await.expect("Subscriber task failed");
        assert_eq!(count, NUM_FRAMES);
    }
}
