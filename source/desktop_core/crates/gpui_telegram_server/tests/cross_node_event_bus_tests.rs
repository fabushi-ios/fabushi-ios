use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use gpui_telegram_core::types::UserId;
use gpui_telegram_proto::ws::WsServerFrame;
use gpui_telegram_server::auth::PersistentSessionStore;
use gpui_telegram_server::bus::{
    CrossNodeEventBus, EventPayload, InProcessEventBus, RedisEventBusStub, ServerEvent,
};
use gpui_telegram_server::db::{ServerDatabase, SqlitePool};
use gpui_telegram_server::server::AppState;
use gpui_telegram_server::ws::GatewayHub;
use gpui_telegram_server::{build_router_with_state, ServerConfig};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};
use tower::ServiceExt;

#[tokio::test]
async fn test_in_process_event_bus_fanout() {
    let bus = InProcessEventBus::new("node-alpha");
    let channel = "chat:101";

    let mut sub1 = bus.subscribe(channel).await.unwrap();
    let mut sub2 = bus.subscribe(channel).await.unwrap();

    let event = ServerEvent::new(
        channel,
        bus.node_id(),
        EventPayload::NewMessage {
            chat_id: 101,
            message_id: 1,
            sender_id: 42,
            text: "Fanout test message".to_string(),
            pts: 1,
        },
    );

    bus.publish(channel, &event).await.unwrap();

    let e1 = timeout(Duration::from_millis(500), sub1.recv())
        .await
        .unwrap()
        .unwrap();
    let e2 = timeout(Duration::from_millis(500), sub2.recv())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(e1.event_id, event.event_id);
    assert_eq!(e2.event_id, event.event_id);
    assert_eq!(e1.payload, event.payload);
    assert_eq!(e2.payload, event.payload);
}

#[tokio::test]
async fn test_channel_filtering_isolation() {
    let bus = InProcessEventBus::new("node-alpha");

    let mut sub_chat_a = bus.subscribe("chat:100").await.unwrap();
    let mut sub_chat_b = bus.subscribe("chat:200").await.unwrap();

    let event_a = ServerEvent::new(
        "chat:100",
        bus.node_id(),
        EventPayload::NewMessage {
            chat_id: 100,
            message_id: 5,
            sender_id: 1,
            text: "Only for chat 100".to_string(),
            pts: 5,
        },
    );

    bus.publish("chat:100", &event_a).await.unwrap();

    let received_a = timeout(Duration::from_millis(200), sub_chat_a.recv())
        .await
        .expect("Channel A should receive event")
        .expect("Receive error");
    assert_eq!(received_a.event_id, event_a.event_id);

    // Channel B must NOT receive events sent to Channel A
    let received_b = timeout(Duration::from_millis(100), sub_chat_b.recv()).await;
    assert!(received_b.is_err(), "Channel B should have timed out without receiving Channel A events");
}

#[tokio::test]
async fn test_simulated_multi_node_cluster() {
    let shared_broker = Arc::new(InProcessEventBus::new("redis-cluster-engine"));

    let node1 = RedisEventBusStub::with_shared_broker(
        "server-node-1",
        "redis://127.0.0.1:6379/0",
        shared_broker.clone(),
    );
    let node2 = RedisEventBusStub::with_shared_broker(
        "server-node-2",
        "redis://127.0.0.1:6379/0",
        shared_broker.clone(),
    );

    let channel = "chat:777";
    let mut node2_sub = node2.subscribe(channel).await.unwrap();

    let node1_event = ServerEvent::new(
        channel,
        node1.node_id(),
        EventPayload::NewMessage {
            chat_id: 777,
            message_id: 99,
            sender_id: 1001,
            text: "Cross-node distributed message".to_string(),
            pts: 10,
        },
    );

    // Node 1 publishes
    node1.publish(channel, &node1_event).await.unwrap();

    // Node 2 receives across simulated cluster network
    let received = timeout(Duration::from_millis(500), node2_sub.recv())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(received.event_id, node1_event.event_id);
    assert_eq!(received.origin_node_id, "server-node-1");
    assert_eq!(received.payload, node1_event.payload);
}

#[tokio::test]
async fn test_session_revocation_broadcast() {
    let pool = SqlitePool::open_in_memory().unwrap();

    // Run schema migrations for sessions table
    pool.with_writer(|conn| {
        conn.execute_batch(gpui_telegram_server::db::schema::CREATE_USERS_TABLE).unwrap();
        conn.execute_batch(gpui_telegram_server::db::schema::CREATE_SESSIONS_TABLE).unwrap();
        conn.execute(
            "INSERT INTO users (user_id, username, first_name, created_at_ms, updated_at_ms) VALUES (888, 'test888', 'User 888', 1000, 1000)",
            [],
        ).unwrap();
        Ok(())
    })
    .unwrap();

    let bus = Arc::new(InProcessEventBus::new("auth-node-1"));
    let session_store = PersistentSessionStore::new(pool);
    session_store.set_event_bus(bus.clone());

    let target_user_id = 888u64;
    let user_channel = format!("user:{target_user_id}");
    let mut user_sub = bus.subscribe(&user_channel).await.unwrap();

    // Create session using unified "{session_id}.{raw_secret}" format
    let (session, token) = session_store
        .issue_session(target_user_id, "device_desktop", "desktop", "1.0", 1, 3600)
        .unwrap();

    assert!(session_store.verify_token(&token).unwrap().is_some());

    // Revoke session
    let revoked = session_store
        .revoke_session(&session.session_id, "User logged out")
        .unwrap();
    assert!(revoked);

    // Token must now be rejected
    assert!(session_store.verify_token(&token).unwrap().is_none());

    // Verify broadcast event was received by subscriber
    let evt = timeout(Duration::from_millis(500), user_sub.recv())
        .await
        .unwrap()
        .unwrap();

    match evt.payload {
        EventPayload::SessionRevoked {
            user_id,
            session_id,
            reason,
        } => {
            assert_eq!(user_id, target_user_id);
            assert_eq!(session_id, session.session_id);
            assert_eq!(reason, "User logged out");
        }
        other => panic!("Expected SessionRevoked payload, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_high_volume_event_burst() {
    let bus = InProcessEventBus::with_capacity("node-burst", 2048);
    let channel = "chat:burst";
    let mut sub = bus.subscribe(channel).await.unwrap();

    let total = 200;
    for i in 0..total {
        let evt = ServerEvent::new(
            channel,
            bus.node_id(),
            EventPayload::NewMessage {
                chat_id: 999,
                message_id: i,
                sender_id: 1,
                text: format!("Burst msg {i}"),
                pts: i as u64,
            },
        );
        bus.publish(channel, &evt).await.unwrap();
    }

    for i in 0..total {
        let r = timeout(Duration::from_millis(200), sub.recv())
            .await
            .unwrap()
            .unwrap();
        if let EventPayload::NewMessage { message_id, .. } = r.payload {
            assert_eq!(message_id, i);
        } else {
            panic!("Unexpected payload");
        }
    }
}

#[tokio::test]
async fn test_in_process_event_bus_wildcard_and_channel_pruning() {
    let bus = InProcessEventBus::new("node-wild");

    let mut sub_wild = bus.subscribe("user:*").await.unwrap();
    let sub_exact = bus.subscribe("chat:42").await.unwrap();
    assert_eq!(bus.active_channel_count(), 2);

    let evt = ServerEvent::new(
        "user:200",
        bus.node_id(),
        EventPayload::SessionRevoked {
            user_id: 200,
            session_id: "sess_abc".into(),
            reason: "test".into(),
        },
    );
    bus.publish("user:200", &evt).await.unwrap();

    let received = timeout(Duration::from_millis(300), sub_wild.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.channel, "user:200");

    drop(sub_exact);
    assert_eq!(bus.active_channel_count(), 1);
    drop(sub_wild);
    assert_eq!(bus.active_channel_count(), 0);
}

#[tokio::test]
async fn test_gateway_hub_cross_node_ws_delivery_and_no_local_echo() {
    let broker = Arc::new(InProcessEventBus::new("shared-broker"));
    let bus_a: Arc<dyn CrossNodeEventBus> = Arc::new(RedisEventBusStub::with_shared_broker(
        "node-a",
        "redis://127.0.0.1:6379/0",
        broker.clone(),
    ));
    let bus_b: Arc<dyn CrossNodeEventBus> = Arc::new(RedisEventBusStub::with_shared_broker(
        "node-b",
        "redis://127.0.0.1:6379/0",
        broker,
    ));

    let hub_a = GatewayHub::new();
    hub_a.set_event_bus(bus_a);
    let hub_b = GatewayHub::new();
    hub_b.set_event_bus(bus_b);

    let (tx_a, mut rx_a) = mpsc::unbounded_channel();
    let (tx_b, mut rx_b) = mpsc::unbounded_channel();
    hub_a.register(UserId(200), tx_a);
    hub_b.register(UserId(200), tx_b);

    // Yield so background subscription tasks complete subscribe()
    tokio::time::sleep(Duration::from_millis(30)).await;

    hub_a.broadcast_to_user(UserId(200), WsServerFrame::Pong { nonce: 777 });

    let frame_a = timeout(Duration::from_millis(300), rx_a.recv())
        .await
        .expect("Node A should deliver locally")
        .unwrap();
    assert!(matches!(frame_a, WsServerFrame::Pong { nonce: 777 }));

    // Node A must NOT receive a duplicate echo from the cluster bus
    let dup_a = timeout(Duration::from_millis(60), rx_a.recv()).await;
    assert!(dup_a.is_err(), "Node A received duplicate local echo");

    let frame_b = timeout(Duration::from_millis(500), rx_b.recv())
        .await
        .expect("Node B should receive cross-node frame")
        .unwrap();
    assert!(matches!(frame_b, WsServerFrame::Pong { nonce: 777 }));
}

#[test]
fn test_gateway_hub_and_session_store_safe_outside_tokio_runtime() {
    let bus = Arc::new(InProcessEventBus::new("sync-node"));
    let hub = GatewayHub::new();
    hub.set_event_bus(bus.clone());

    hub.broadcast_to_user(UserId(10), WsServerFrame::Pong { nonce: 1 });
    hub.broadcast_to_chat(20, WsServerFrame::Pong { nonce: 2 });

    let pool = SqlitePool::open_in_memory().unwrap();
    pool.with_writer(|conn| {
        conn.execute_batch(gpui_telegram_server::db::schema::CREATE_USERS_TABLE)
            .unwrap();
        conn.execute_batch(gpui_telegram_server::db::schema::CREATE_SESSIONS_TABLE)
            .unwrap();
        conn.execute(
            "INSERT INTO users (user_id, username, first_name, created_at_ms, updated_at_ms) VALUES (55, 'sync55', 'Sync', 1000, 1000)",
            [],
        )
        .unwrap();
        Ok(())
    })
    .unwrap();

    let store = PersistentSessionStore::new(pool);
    store.set_event_bus(bus);
    let (sess, _tok) = store
        .issue_session(55, "dev", "desktop", "1.0", 1, 3600)
        .unwrap();
    assert!(store.revoke_session(&sess.session_id, "sync_test").unwrap());
}

#[tokio::test]
async fn test_two_node_http_session_revocation_and_non_blocking_verify() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("cluster_auth.db");
    let db_a = Arc::new(ServerDatabase::open(&db_path).unwrap());
    let db_b = Arc::new(ServerDatabase::open(&db_path).unwrap());

    let broker = Arc::new(InProcessEventBus::new("cluster-broker"));
    let bus_a: Arc<dyn CrossNodeEventBus> = Arc::new(RedisEventBusStub::with_shared_broker(
        "http-node-a",
        "redis://127.0.0.1:6379/0",
        broker.clone(),
    ));
    let bus_b: Arc<dyn CrossNodeEventBus> = Arc::new(RedisEventBusStub::with_shared_broker(
        "http-node-b",
        "redis://127.0.0.1:6379/0",
        broker,
    ));

    let state_a = Arc::new(AppState::with_db(ServerConfig::default(), db_a.clone()).with_event_bus(bus_a));
    let state_b = Arc::new(AppState::with_db(ServerConfig::default(), db_b).with_event_bus(bus_b));
    let app_a = build_router_with_state(state_a.clone());
    let app_b = build_router_with_state(state_b.clone());

    tokio::time::sleep(Duration::from_millis(30)).await;

    // 1. Register on Node A
    let reg_res = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "username": "cluster_user",
                        "password": "Password123!",
                        "first_name": "Cluster",
                        "last_name": "Tester"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reg_res.status(), StatusCode::CREATED);
    let reg_val: Value =
        serde_json::from_slice(&to_bytes(reg_res.into_body(), usize::MAX).await.unwrap()).unwrap();
    let token = reg_val["token"].as_str().unwrap().to_string();
    let session_id = reg_val["session_id"].as_str().unwrap().to_string();

    // 2. Token created on Node A authenticates cleanly on Node B
    let me_b_before = app_b
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/auth/me")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(me_b_before.status(), StatusCode::OK);

    // 3. PersistentSessionStore::verify_token must NOT block during a 250ms writer transaction
    let held = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let held_w = held.clone();
    let db_w = db_a.clone();
    let writer = std::thread::spawn(move || {
        db_w.pool()
            .with_transaction(|_tx| {
                held_w.store(true, std::sync::atomic::Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(250));
                Ok(())
            })
            .unwrap();
    });
    while !held.load(std::sync::atomic::Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let start = Instant::now();
    let verified = state_b.session_store.verify_token(&token).unwrap();
    let verify_elapsed = start.elapsed();
    assert!(verified.is_some());
    assert!(
        verify_elapsed < Duration::from_millis(100),
        "verify_token stalled on writer lock: {verify_elapsed:?}"
    );
    writer.join().unwrap();

    // 4. Register a WS receiver on Node B bound to session_id, then revoke session on Node A
    let user_id = UserId(reg_val["user_id"].as_u64().unwrap());
    let (ws_tx_b, mut ws_rx_b) = mpsc::unbounded_channel();
    state_b
        .gateway_hub
        .register_session(user_id, Some(session_id.clone()), ws_tx_b);

    let del_res = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/auth/sessions/{session_id}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(del_res.status(), StatusCode::OK);

    // 5. Node B disconnects WS with 401 frame and rejects subsequent HTTP requests with 401
    let ws_close = timeout(Duration::from_millis(500), ws_rx_b.recv())
        .await
        .expect("Node B should send 401 frame on SessionRevoked")
        .unwrap();
    assert!(matches!(ws_close, WsServerFrame::Error { code: 401, .. }));

    let me_b_after = app_b
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/auth/me")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(me_b_after.status(), StatusCode::UNAUTHORIZED);
}
