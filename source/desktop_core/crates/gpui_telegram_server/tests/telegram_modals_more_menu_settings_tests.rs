use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use gpui_telegram_server::{build_router_with_state, server::AppState, ServerConfig};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
use tower::ServiceExt;
use uuid::Uuid;

struct TempDirGuard(PathBuf);
impl TempDirGuard {
    fn new(prefix: &str) -> Self {
        let p = std::env::temp_dir().join(format!("{prefix}_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn durable_app(dir: &TempDirGuard) -> (Arc<AppState>, Router) {
    let cfg = ServerConfig {
        database_url: dir.0.join("modals_settings_parity.db").to_string_lossy().into_owned(),
        media_storage_path: dir.0.join("cas").to_string_lossy().into_owned(),
        ..Default::default()
    };
    let state = Arc::new(AppState::try_new(cfg).expect("durable AppState must open"));
    let router = build_router_with_state(state.clone());
    (state, router)
}

async fn api(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let st = res.status();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (st, json)
}

async fn reg(app: &Router, uname: &str, pw: &str) -> (u64, String) {
    let (st, v) = api(
        app,
        Method::POST,
        "/api/v1/auth/register",
        None,
        Some(json!({
            "username": uname,
            "password": pw,
            "first_name": uname,
            "device_name": "ParityMac"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    (
        v["user_id"].as_u64().unwrap(),
        v["token"].as_str().unwrap().to_string(),
    )
}

#[tokio::test]
async fn test_profile_phone_2fa_status_and_durable_user_privacy_settings() {
    let dir = TempDirGuard::new("tg_settings_truth");
    let (st0, app) = durable_app(&dir);
    let (_alice_id, alice_tok) = reg(&app, "alice_real_cfg", "StrongPass123!").await;
    let (_bob_id, _bob_tok) = reg(&app, "bob_taken", "StrongPass123!").await;

    // 1. Profile edit including phone + bio via PATCH /api/v1/users/me
    let (st, updated) = api(
        &app,
        Method::PATCH,
        "/api/v1/users/me",
        Some(&alice_tok),
        Some(json!({
            "first_name": "Alice",
            "last_name": "Lin",
            "username": "alice_lin",
            "phone": "+8613800138000",
            "bio": "Real Telegram parity settings"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(updated["first_name"], "Alice");
    assert_eq!(updated["last_name"], "Lin");
    assert_eq!(updated["username"], "alice_lin");
    assert_eq!(updated["phone"], "+8613800138000");
    assert_eq!(updated["bio"], "Real Telegram parity settings");

    // Boundary/error cases: empty first_name or duplicate username must fail cleanly
    let (st_empty, _) = api(
        &app,
        Method::PATCH,
        "/api/v1/users/me",
        Some(&alice_tok),
        Some(json!({ "first_name": "   " })),
    )
    .await;
    assert_eq!(st_empty, StatusCode::BAD_REQUEST);

    let (st_dup, _) = api(
        &app,
        Method::PATCH,
        "/api/v1/users/me",
        Some(&alice_tok),
        Some(json!({ "username": "bob_taken" })),
    )
    .await;
    assert_eq!(st_dup, StatusCode::CONFLICT);

    // Profile remains intact after failed mutations
    let (st_me, me) = api(&app, Method::GET, "/api/v1/users/me", Some(&alice_tok), None).await;
    assert_eq!(st_me, StatusCode::OK);
    assert_eq!(me["username"], "alice_lin");
    assert_eq!(me["phone"], "+8613800138000");

    // 2. 2FA status, set, wrong-password disable rejection, and valid disable
    let (st_2fa0, s0) = api(&app, Method::GET, "/api/v1/auth/2fa/status", Some(&alice_tok), None).await;
    assert_eq!(st_2fa0, StatusCode::OK);
    assert_eq!(s0["has_password"], false);

    let (st_set, s1) = api(
        &app,
        Method::POST,
        "/api/v1/auth/2fa/set",
        Some(&alice_tok),
        Some(json!({ "new_password": "CloudSecret#7", "hint": "lucky7" })),
    )
    .await;
    assert_eq!(st_set, StatusCode::OK);
    assert_eq!(s1["has_password"], true);

    let (st_2fa1, s2) = api(&app, Method::GET, "/api/v1/auth/2fa/status", Some(&alice_tok), None).await;
    assert_eq!(st_2fa1, StatusCode::OK);
    assert_eq!(s2["has_password"], true);
    assert_eq!(s2["hint"], "lucky7");

    let (st_bad_dis, _) = api(
        &app,
        Method::POST,
        "/api/v1/auth/2fa/disable",
        Some(&alice_tok),
        Some(json!({ "current_password": "wrong_password" })),
    )
    .await;
    assert_eq!(st_bad_dis, StatusCode::UNAUTHORIZED);

    let (st_ok_dis, s3) = api(
        &app,
        Method::POST,
        "/api/v1/auth/2fa/disable",
        Some(&alice_tok),
        Some(json!({ "current_password": "CloudSecret#7" })),
    )
    .await;
    assert_eq!(st_ok_dis, StatusCode::OK);
    assert_eq!(s3["has_password"], false);

    // 3. Durable UserPrivacySettingsDto roundtrip + SQLite reopen persistence
    let (st_def, def_cfg) = api(&app, Method::GET, "/api/v1/users/me/settings", Some(&alice_tok), None).await;
    assert_eq!(st_def, StatusCode::OK);
    assert_eq!(def_cfg["ttl_months"], 6);
    assert_eq!(def_cfg["notifications_private"], true);

    let custom_settings = json!({
        "phone_privacy": "Nobody",
        "last_seen_privacy": "My Contacts",
        "photo_privacy": "Everybody",
        "calls_privacy": "My Contacts",
        "forwards_privacy": "Nobody",
        "groups_privacy": "My Contacts",
        "ttl_months": 12,
        "global_ttl_seconds": 86400,
        "sensitive_content_enabled": true,
        "passcode_enabled": true,
        "notify_private_chats": true,
        "notify_groups": false,
        "notify_channels": false,
        "notify_sound": false,
        "p2p_mode": "Never",
        "accept_calls": false
    });
    let (st_put, saved_cfg) = api(
        &app,
        Method::PUT,
        "/api/v1/users/me/settings",
        Some(&alice_tok),
        Some(custom_settings),
    )
    .await;
    assert_eq!(st_put, StatusCode::OK);
    assert_eq!(saved_cfg["phone_privacy"], "Nobody");
    assert_eq!(saved_cfg["ttl_months"], 12);
    assert_eq!(saved_cfg["global_ttl_seconds"], 86400);
    assert_eq!(saved_cfg["sensitive_content_enabled"], true);
    assert_eq!(saved_cfg["p2p_mode"], "Never");

    // Drop first instance cleanly and reopen SQLite database from disk to verify durability
    drop(app);
    drop(st0);
    let (_, reopened_app) = durable_app(&dir);
    let (st_re, re_cfg) = api(
        &reopened_app,
        Method::GET,
        "/api/v1/users/me/settings",
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_re, StatusCode::OK);
    assert_eq!(re_cfg["phone_privacy"], "Nobody");
    assert_eq!(re_cfg["forwards_privacy"], "Nobody");
    assert_eq!(re_cfg["global_ttl_seconds"], 86400);
    assert_eq!(re_cfg["notifications_groups"], false);
    assert_eq!(re_cfg["accept_calls"], false);
}

#[tokio::test]
async fn test_more_menu_peer_block_ttl_clear_history_tantivy_isolation_and_leave() {
    let dir = TempDirGuard::new("tg_more_menu_ops");
    let (state, app) = durable_app(&dir);
    let (alice_id, alice_tok) = reg(&app, "alice_menu", "StrongPass123!").await;
    let (bob_id, bob_tok) = reg(&app, "bob_menu", "StrongPass123!").await;
    let (carol_id, _carol_tok) = reg(&app, "carol_menu", "StrongPass123!").await;

    // Create DM between Alice & Bob (chat A) and DM between Alice & Carol (chat B)
    let (st_dm1, dm1) = api(
        &app,
        Method::POST,
        "/api/v1/chats",
        Some(&alice_tok),
        Some(json!({ "chat_type": 0, "user_id": bob_id })),
    )
    .await;
    assert_eq!(st_dm1, StatusCode::CREATED);
    let chat_ab = dm1["chat_id"].as_i64().unwrap();

    let (st_dm2, dm2) = api(
        &app,
        Method::POST,
        "/api/v1/chats",
        Some(&alice_tok),
        Some(json!({ "chat_type": 0, "user_id": carol_id })),
    )
    .await;
    assert_eq!(st_dm2, StatusCode::CREATED);
    let chat_ac = dm2["chat_id"].as_i64().unwrap();

    // Send message #1 in BOTH chats with distinct searchable keyword + shared keyword
    let (st_m1, _) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{chat_ab}/messages"),
        Some(&alice_tok),
        Some(json!({ "text": "unicornbob message one" })),
    )
    .await;
    assert_eq!(st_m1, StatusCode::CREATED);

    let (st_m2, _) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{chat_ac}/messages"),
        Some(&alice_tok),
        Some(json!({ "text": "unicorncarol surviving message one" })),
    )
    .await;
    assert_eq!(st_m2, StatusCode::CREATED);
    state.search_indexer.commit().await.unwrap();

    // 1. Auto-Delete TTL setting + clamping + chat list reflection
    let (st_ttl, ttl_res) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{chat_ab}/ttl"),
        Some(&alice_tok),
        Some(json!({ "ttl_seconds": 86400 })),
    )
    .await;
    assert_eq!(st_ttl, StatusCode::OK);
    assert_eq!(ttl_res["ttl_seconds"], 86400);

    // Overflow boundary clamp to 1 year (31_536_000)
    let (st_ttl_max, ttl_max) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{chat_ab}/ttl"),
        Some(&alice_tok),
        Some(json!({ "ttl_seconds": 999_999_999u64 })),
    )
    .await;
    assert_eq!(st_ttl_max, StatusCode::OK);
    assert_eq!(ttl_max["ttl_seconds"], 31_536_000);

    // 2. Peer Block edge cases & enforcement
    // Self-block rejected with 400
    let (st_self_blk, _) = api(
        &app,
        Method::POST,
        &format!("/api/v1/users/{alice_id}/block"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_self_blk, StatusCode::BAD_REQUEST);

    // Non-existent user block rejected with 404
    let (st_missing_blk, _) = api(
        &app,
        Method::POST,
        "/api/v1/users/99999999/block",
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_missing_blk, StatusCode::NOT_FOUND);

    // Valid block of Bob
    let (st_blk, blk_dto) = api(
        &app,
        Method::POST,
        &format!("/api/v1/users/{bob_id}/block"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_blk, StatusCode::OK);
    assert_eq!(blk_dto["user_id"], bob_id);

    // Blocked list includes Bob, and GET /api/v1/chats/{chat_ab} reports is_blocked == true
    let (st_blist, blist) = api(&app, Method::GET, "/api/v1/users/blocked", Some(&alice_tok), None).await;
    assert_eq!(st_blist, StatusCode::OK);
    assert_eq!(blist["total"], 1);
    assert_eq!(blist["blocked"][0]["user_id"], bob_id);

    let (st_cdet, cdet) = api(
        &app,
        Method::GET,
        &format!("/api/v1/chats/{chat_ab}"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_cdet, StatusCode::OK);
    assert_eq!(cdet["is_blocked"], true);
    assert_eq!(cdet["message_ttl_seconds"], 31_536_000);

    // Blocked Bob CANNOT send messages to Alice in DM (403 user_is_blocked)
    let (st_bob_blocked_send, err_blk) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{chat_ab}/messages"),
        Some(&bob_tok),
        Some(json!({ "text": "spam while blocked" })),
    )
    .await;
    assert_eq!(st_bob_blocked_send, StatusCode::FORBIDDEN);
    assert_eq!(err_blk["error"], "user_is_blocked");

    // Unblock Bob -> messaging immediately works again
    let (st_unblk, unblk_res) = api(
        &app,
        Method::DELETE,
        &format!("/api/v1/users/{bob_id}/block"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_unblk, StatusCode::OK);
    assert_eq!(unblk_res["unblocked"], true);

    let (st_bob_ok, _) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{chat_ab}/messages"),
        Some(&bob_tok),
        Some(json!({ "text": "hello after unblock" })),
    )
    .await;
    assert_eq!(st_bob_ok, StatusCode::CREATED);

    // 3. Clear Chat History on chat_ab deletes SQLite + Tantivy for chat_ab ONLY (chat_ac msg #1 survives!)
    let (st_clr, clr_res) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{chat_ab}/clear"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_clr, StatusCode::OK);
    assert_eq!(clr_res["cleared"], true);
    assert_eq!(clr_res["deleted_count"], 2);

    let (_, hist_ab) = api(
        &app,
        Method::GET,
        &format!("/api/v1/chats/{chat_ab}/messages"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(hist_ab["messages"].as_array().unwrap().len(), 0);

    let hits_bob = state.search_indexer.search("unicornbob", None).unwrap();
    assert!(hits_bob.is_empty(), "Cleared chat_ab messages must be purged from Tantivy");
    let hits_carol = state.search_indexer.search("unicorncarol", None).unwrap();
    assert_eq!(
        hits_carol.len(),
        1,
        "Clearing chat_ab must NOT delete message_id=1 from chat_ac in Tantivy"
    );

    // 4. Group Leave vs Delete Chat
    let (st_grp, grp) = api(
        &app,
        Method::POST,
        "/api/v1/chats",
        Some(&alice_tok),
        Some(json!({ "title": "Rust Modal Group", "chat_type": 1, "member_ids": [bob_id] })),
    )
    .await;
    assert_eq!(st_grp, StatusCode::CREATED);
    let grp_id = grp["chat_id"].as_i64().unwrap();

    // Non-admin Bob cannot set group TTL
    let (st_bob_ttl, _) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{grp_id}/ttl"),
        Some(&bob_tok),
        Some(json!({ "ttl_seconds": 86400 })),
    )
    .await;
    assert_eq!(st_bob_ttl, StatusCode::FORBIDDEN);

    // Bob leaves group -> Bob loses membership (404/403), Alice still owns group (200)
    let (st_leave, lv) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{grp_id}/leave"),
        Some(&bob_tok),
        None,
    )
    .await;
    assert_eq!(st_leave, StatusCode::OK);
    assert_eq!(lv["deleted"], true);

    let (st_bob_after, _) = api(
        &app,
        Method::GET,
        &format!("/api/v1/chats/{grp_id}"),
        Some(&bob_tok),
        None,
    )
    .await;
    assert!(matches!(st_bob_after, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND));

    let (st_alice_after, _) = api(
        &app,
        Method::GET,
        &format!("/api/v1/chats/{grp_id}"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_alice_after, StatusCode::OK);

    // Owner Alice deletes group -> 404/403 afterwards
    let (st_owner_del, _) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{grp_id}/leave"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_owner_del, StatusCode::OK);

    let (st_gone, _) = api(
        &app,
        Method::GET,
        &format!("/api/v1/chats/{grp_id}"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_gone, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_auto_delete_ttl_expiry_purges_sqlite_and_tantivy_without_cross_chat_collision() {
    let dir = TempDirGuard::new("tg_ttl_expiry_purge");
    let (state, app) = durable_app(&dir);
    let (_alice_id, alice_tok) = reg(&app, "alice_ttl_exp", "StrongPass123!").await;
    let (bob_id, _bob_tok) = reg(&app, "bob_ttl_exp", "StrongPass123!").await;
    let (carol_id, _carol_tok) = reg(&app, "carol_ttl_exp", "StrongPass123!").await;

    // Chat 1 (TTL = 1s) with Bob; Chat 2 (TTL = 0 / Off) with Carol
    let (_, dm_bob) = api(
        &app,
        Method::POST,
        "/api/v1/chats",
        Some(&alice_tok),
        Some(json!({ "chat_type": 0, "user_id": bob_id })),
    )
    .await;
    let chat_ttl = dm_bob["chat_id"].as_i64().unwrap();

    let (_, dm_carol) = api(
        &app,
        Method::POST,
        "/api/v1/chats",
        Some(&alice_tok),
        Some(json!({ "chat_type": 0, "user_id": carol_id })),
    )
    .await;
    let chat_keep = dm_carol["chat_id"].as_i64().unwrap();

    let (st_ttl, _) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{chat_ttl}/ttl"),
        Some(&alice_tok),
        Some(json!({ "ttl_seconds": 1 })),
    )
    .await;
    assert_eq!(st_ttl, StatusCode::OK);

    // Send msg #1 in chat_ttl and msg #1 in chat_keep (both get per-chat message_id == 1!)
    let (st1, _) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{chat_ttl}/messages"),
        Some(&alice_tok),
        Some(json!({ "text": "ephemeralphoenix secret token" })),
    )
    .await;
    assert_eq!(st1, StatusCode::CREATED);

    let (st2, _) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{chat_keep}/messages"),
        Some(&alice_tok),
        Some(json!({ "text": "permanentphoenix archive token" })),
    )
    .await;
    assert_eq!(st2, StatusCode::CREATED);
    state.search_indexer.commit().await.unwrap();

    // Verify Tantivy indexed BOTH message_id=1 documents without cross-chat upsert overwrite!
    assert_eq!(
        state.search_indexer.search("ephemeralphoenix", None).unwrap().len(),
        1
    );
    assert_eq!(
        state.search_indexer.search("permanentphoenix", None).unwrap().len(),
        1
    );

    // Wait 1.15s so chat_ttl's 1-second TTL expires
    tokio::time::sleep(std::time::Duration::from_millis(1150)).await;

    // Send a fresh message in chat_ttl so chat_ttl has 1 expired + 1 fresh message
    let (st3, _) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{chat_ttl}/messages"),
        Some(&alice_tok),
        Some(json!({ "text": "freshphoenix still alive" })),
    )
    .await;
    assert_eq!(st3, StatusCode::CREATED);

    // Fetching history enforces TTL expiry: expired msg #1 is purged, fresh msg #2 remains!
    let (st_hist, hist) = api(
        &app,
        Method::GET,
        &format!("/api/v1/chats/{chat_ttl}/messages"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_hist, StatusCode::OK);
    let msgs = hist["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["text"], "freshphoenix still alive");

    state.search_indexer.commit().await.unwrap();
    assert!(
        state.search_indexer.search("ephemeralphoenix", None).unwrap().is_empty(),
        "Expired TTL message must be purged from Tantivy search index"
    );
    assert_eq!(
        state.search_indexer.search("permanentphoenix", None).unwrap().len(),
        1,
        "Non-TTL chat message with message_id=1 must remain intact in Tantivy"
    );
}

#[tokio::test]
async fn test_premium_persistence_and_stars_wallet_topup_roundtrip() {
    let dir = TempDirGuard::new("tg_premium_stars_truth");
    let (st0, app) = durable_app(&dir);
    let (_uid, tok) = reg(&app, "alice_stars_prem", "StrongPass123!").await;

    // Initially non-premium and 0 Stars
    let (_, me0) = api(&app, Method::GET, "/api/v1/users/me", Some(&tok), None).await;
    assert_eq!(me0["is_premium"], false);

    let (st_bal0, bal0) = api(&app, Method::GET, "/api/v1/wallet/balance", Some(&tok), None).await;
    assert_eq!(st_bal0, StatusCode::OK);
    assert_eq!(bal0["stars_balance"], 0);

    // Activate Premium via PUT /api/v1/users/me/settings + Top-up 500 Stars via POST /api/v1/wallet/topup
    let (st_put, cfg) = api(
        &app,
        Method::PUT,
        "/api/v1/users/me/settings",
        Some(&tok),
        Some(json!({ "is_premium": true, "ttl_months": 12 })),
    )
    .await;
    assert_eq!(st_put, StatusCode::OK);
    assert_eq!(cfg["is_premium"], true);

    let (st_top, bal1) = api(
        &app,
        Method::POST,
        "/api/v1/wallet/topup",
        Some(&tok),
        Some(json!({ "amount_stars": 500 })),
    )
    .await;
    assert_eq!(st_top, StatusCode::OK);
    assert_eq!(bal1["stars_balance"], 500);

    // Reopen SQLite from disk and verify both GET /api/v1/users/me.is_premium and wallet transactions persist
    drop(app);
    drop(st0);
    let (_, reopened) = durable_app(&dir);

    let (st_me1, me1) = api(&reopened, Method::GET, "/api/v1/users/me", Some(&tok), None).await;
    assert_eq!(st_me1, StatusCode::OK);
    assert_eq!(me1["is_premium"], true);

    let (st_txs, txs) = api(
        &reopened,
        Method::GET,
        "/api/v1/wallet/transactions",
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st_txs, StatusCode::OK);
    let tx_arr = txs["transactions"].as_array().unwrap();
    assert_eq!(tx_arr.len(), 1);
    assert_eq!(tx_arr[0]["amount_stars"], 500);
}

#[tokio::test]
async fn test_global_ttl_inherited_by_new_dm_group_channel_and_contact_chats() {
    let dir = TempDirGuard::new("tg_global_ttl_inherit");
    let (_state, app) = durable_app(&dir);
    let (_alice_id, alice_tok) = reg(&app, "alice_gttl", "StrongPass123!").await;
    let (bob_id, _bob_tok) = reg(&app, "bob_gttl", "StrongPass123!").await;
    let (dave_id, _dave_tok) = reg(&app, "dave_gttl", "StrongPass123!").await;
    let (_erin_id, _erin_tok) = reg(&app, "erin_gttl", "StrongPass123!").await;

    // 1. Before enabling global TTL (default 0), a new DM with Bob has message_ttl_seconds == 0
    let (st_dm0, dm_bob) = api(
        &app,
        Method::POST,
        "/api/v1/chats",
        Some(&alice_tok),
        Some(json!({ "chat_type": 0, "user_id": bob_id })),
    )
    .await;
    assert_eq!(st_dm0, StatusCode::CREATED);
    let chat_bob = dm_bob["chat_id"].as_i64().unwrap();
    assert_eq!(dm_bob["message_ttl_seconds"].as_u64().unwrap_or(0), 0);

    // 2. Alice sets global_ttl_seconds = 604800 (1 week) via PUT /api/v1/users/me/settings
    let (st_put, cfg) = api(
        &app,
        Method::PUT,
        "/api/v1/users/me/settings",
        Some(&alice_tok),
        Some(json!({ "global_ttl_seconds": 604_800 })),
    )
    .await;
    assert_eq!(st_put, StatusCode::OK);
    assert_eq!(cfg["global_ttl_seconds"], 604_800);

    // 3. Reusing the existing DM with Bob returns the same chat_id and preserves its existing TTL (0)
    let (st_reuse, dm_bob_reused) = api(
        &app,
        Method::POST,
        "/api/v1/chats",
        Some(&alice_tok),
        Some(json!({ "chat_type": 0, "user_id": bob_id })),
    )
    .await;
    assert!(matches!(st_reuse, StatusCode::OK | StatusCode::CREATED));
    assert_eq!(dm_bob_reused["chat_id"], chat_bob);
    assert_eq!(dm_bob_reused["message_ttl_seconds"], 0);

    // 4. Newly created DM with Dave, Group, and Channel all inherit global_ttl_seconds == 604800
    let (st_dave, dm_dave) = api(
        &app,
        Method::POST,
        "/api/v1/chats",
        Some(&alice_tok),
        Some(json!({ "chat_type": 0, "user_id": dave_id })),
    )
    .await;
    assert_eq!(st_dave, StatusCode::CREATED);
    assert_eq!(dm_dave["message_ttl_seconds"], 604_800);
    let chat_dave = dm_dave["chat_id"].as_i64().unwrap();

    let (st_get_dave, det_dave) = api(
        &app,
        Method::GET,
        &format!("/api/v1/chats/{chat_dave}"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_get_dave, StatusCode::OK);
    assert_eq!(det_dave["message_ttl_seconds"], 604_800);

    let (st_grp, grp) = api(
        &app,
        Method::POST,
        "/api/v1/chats",
        Some(&alice_tok),
        Some(json!({ "title": "Ephemeral Team", "chat_type": 1, "member_ids": [dave_id] })),
    )
    .await;
    assert_eq!(st_grp, StatusCode::CREATED);
    assert_eq!(grp["message_ttl_seconds"], 604_800);

    let (st_chan, chan) = api(
        &app,
        Method::POST,
        "/api/v1/chats",
        Some(&alice_tok),
        Some(json!({ "title": "Ephemeral News", "chat_type": 2 })),
    )
    .await;
    assert_eq!(st_chan, StatusCode::CREATED);
    assert_eq!(chan["message_ttl_seconds"], 604_800);

    // 5. Saved Messages (chat_type: 5) must NOT inherit global auto-delete TTL
    let (st_saved, saved) = api(
        &app,
        Method::POST,
        "/api/v1/chats",
        Some(&alice_tok),
        Some(json!({ "title": "Saved Messages", "chat_type": 5 })),
    )
    .await;
    assert!(matches!(st_saved, StatusCode::CREATED | StatusCode::OK));
    assert_eq!(saved["message_ttl_seconds"].as_u64().unwrap_or(0), 0);

    // 6. Adding a human contact and an AI bot via POST /api/v1/contacts also inherits global TTL
    let (st_ct, ct_erin) = api(
        &app,
        Method::POST,
        "/api/v1/contacts",
        Some(&alice_tok),
        Some(json!({
            "first_name": "Erin",
            "username": "erin_gttl",
            "is_bot": false
        })),
    )
    .await;
    assert!(matches!(st_ct, StatusCode::CREATED | StatusCode::OK));
    assert_eq!(ct_erin["message_ttl_seconds"], 604_800);

    let (st_bot, ct_bot) = api(
        &app,
        Method::POST,
        "/api/v1/contacts",
        Some(&alice_tok),
        Some(json!({
            "first_name": "TTL Helper Bot",
            "username": "ttl_helper_bot",
            "is_bot": true,
            "bot_system_prompt": "Be concise."
        })),
    )
    .await;
    assert!(matches!(st_bot, StatusCode::CREATED | StatusCode::OK));
    assert_eq!(ct_bot["message_ttl_seconds"], 604_800);
}
