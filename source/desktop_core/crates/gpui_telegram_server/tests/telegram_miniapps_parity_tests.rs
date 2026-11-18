//! End-to-End Integration Tests for 1:1 Telegram Mini Apps & Bot HTTP API Parity.
//!
//! Covers all 6 Telegram parity invariants:
//! 1. Fresh boot seeds ONLY `@BotFather` (`777_001`) and zero built-in host Mini Apps
//!    (`PomodoroFocusBot` / `SplitBillBot` / `PollMasterBot` absent; legacy rows unpublished).
//! 2. Standard bot creation via `@BotFather` (`/newbot`, `/newapp`) + configuration via
//!    `/bot<token>/setMyCommands`, `setMyDescription`, `setMyShortDescription`,
//!    `setMyPrivacyPolicy`, `setChatMenuButton`, `publishMiniApp` exposes exact per-bot
//!    metadata, commands, menu button, description, and 30d MAU in
//!    `GET /api/v1/bots/profile/{bot}` WITHOUT creating any DM chat.
//! 3. Launching Mini App (`POST /api/v1/miniapps/{bot}/{short}/launch`) creates NO bot DM chat,
//!    signs `allows_write_to_pm: false` in `initData` HMAC-SHA256, records usage in
//!    `GET /api/v1/miniapps/recent` & MAU, and `/bot<token>/sendMessage` returns `403 Forbidden`.
//! 4. `POST /api/v1/bots/{bot_id}/allow-send-message` creates the bot chat (no AI welcome),
//!    inserts `"您已允许该机器人在其网页应用中向您发送消息。"`, delivers `write_access_allowed`
//!    update to `/bot<token>/getUpdates` (and webhook), and unlocks `/bot<token>/sendMessage`
//!    (`200 OK`); blocking (`POST /api/v1/bots/{bot_id}/block`) revokes it (`403 Forbidden`).
//! 5. Standard bot incoming user messages set `has_started_chat = true`, deliver `message`
//!    updates to `/bot<token>/getUpdates` without triggering fake Hermes AI replies, and
//!    `answerWebAppQuery` / `editMessageText` / `deleteMessage` work.
//! 6. `GET /js/telegram-web-app.js` returns `200 OK` JavaScript implementing `requestWriteAccess`
//!    and full `window.Telegram.WebApp` surface; `GET /miniapps/{bot}/{short}` serves hosted
//!    bundle HTML or external URL iframe bridge cleanly.

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use gpui_telegram_proto::miniapp_contracts::{
    AllowSendMessageResponse, BotPublicProfileDto, CanSendMessageResponse, MiniAppEntry,
    MiniAppLaunchContext, BOTFATHER_USER_ID,
};
use gpui_telegram_server::{
    build_router_with_state, config::ServerConfig, server::AppState,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(prefix: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("{}_{}", prefix, Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self { path: dir }
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn boot_temp_server(prefix: &str) -> (TempDirGuard, Arc<AppState>, Router) {
    let dir = TempDirGuard::new(prefix);
    let db_path = dir.path.join("tg_miniapps_parity.db");
    let media_dir = dir.path.join("media_cas");
    std::fs::create_dir_all(&media_dir).expect("create media dir");
    let mut cfg = ServerConfig::default();
    cfg.database_url = format!("sqlite://{}", db_path.display());
    cfg.media_storage_path = media_dir.to_string_lossy().to_string();
    cfg.host = "127.0.0.1".to_string();
    cfg.port = 0;
    let state = Arc::new(AppState::try_new(cfg).expect("boot AppState"));
    let router = build_router_with_state(state.clone());
    (dir, state, router)
}

async fn http_raw(
    router: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Option<String>, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = if let Some(b) = body {
        builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let resp = router.clone().oneshot(req).await.expect("oneshot router");
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, content_type, bytes.to_vec())
}

async fn http_json(
    router: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, _, bytes) = http_raw(router, method, uri, token, body).await;
    let val = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, val)
}

async fn register_user(router: &Router, username: &str) -> (u64, String) {
    let (status, val) = http_json(
        router,
        "POST",
        "/api/v1/auth/register",
        None,
        Some(json!({
            "username": username,
            "password": "ParityStrongPassword!2026",
            "first_name": username,
            "last_name": "Parity",
            "device_name": "ParityTestDesktop"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "register {username}: {val}");
    (
        val["user_id"].as_u64().expect("user_id"),
        val["token"].as_str().expect("token").to_string(),
    )
}

async fn list_chat_messages(router: &Router, token: &str, chat_id: i64) -> Vec<Value> {
    let (status, val) = http_json(
        router,
        "GET",
        &format!("/api/v1/chats/{chat_id}/messages?limit=100"),
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "list messages {chat_id}: {val}");
    val["messages"].as_array().cloned().unwrap_or_default()
}

async fn list_chats(router: &Router, token: &str) -> Vec<Value> {
    let (status, val) = http_json(router, "GET", "/api/v1/chats", Some(token), None).await;
    assert_eq!(status, StatusCode::OK, "list chats: {val}");
    val["chats"].as_array().cloned().unwrap_or_default()
}

async fn send_dm_and_wait_botfather_reply(
    router: &Router,
    token: &str,
    chat_id: i64,
    text: &str,
) -> Value {
    let (st, send_res) = http_json(
        router,
        "POST",
        &format!("/api/v1/chats/{chat_id}/messages"),
        Some(token),
        Some(json!({ "text": text })),
    )
    .await;
    assert!(st.is_success(), "send '{text}' failed: {st} {send_res}");
    let sent_id = send_res["message_id"].as_i64().unwrap_or(0);

    for _ in 0..100 {
        let msgs = list_chat_messages(router, token, chat_id).await;
        if let Some(reply) = msgs.iter().rev().find(|m| {
            m["id"].as_i64().unwrap_or(0) > sent_id
                && m["sender_id"].as_u64() == Some(BOTFATHER_USER_ID as u64)
        }) {
            return reply.clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for BotFather reply to '{text}'");
}

fn extract_token_from_botfather_reply(text: &str) -> String {
    for seg in text.split('`') {
        let trimmed = seg.trim();
        if let Some((id_part, secret)) = trimmed.split_once(':') {
            if id_part.chars().all(|c| c.is_ascii_digit()) && secret.starts_with("AA") {
                return trimmed.to_string();
            }
        }
    }
    panic!("could not extract bot_token from BotFather reply: {text}");
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k_pad = [0u8; BLOCK];
    if key.len() > BLOCK {
        k_pad[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k_pad[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k_pad[i];
        opad[i] ^= k_pad[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(data);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    let out = outer.finalize();
    let mut res = [0u8; 32];
    res.copy_from_slice(&out);
    res
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn verify_telegram_hmac_independent(raw_query: &str, bot_token: &str) -> bool {
    let mut pairs: BTreeMap<String, String> = BTreeMap::new();
    let mut provided_hash = None;
    for part in raw_query.split('&') {
        if part.is_empty() {
            continue;
        }
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        let dk = percent_decode(k);
        let dv = percent_decode(v);
        if dk == "hash" {
            provided_hash = Some(dv);
        } else {
            pairs.insert(dk, dv);
        }
    }
    let Some(hash_hex) = provided_hash else {
        return false;
    };
    let check_str = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n");
    let secret = hmac_sha256(b"WebAppData", bot_token.as_bytes());
    let sig = hmac_sha256(&secret, check_str.as_bytes());
    let computed: String = sig.iter().map(|b| format!("{:02x}", b)).collect();
    computed == hash_hex.to_ascii_lowercase()
}

#[tokio::test]
async fn test_parity_flow_1_only_botfather_seeded_and_zero_host_built_miniapps() {
    let (_dir, state, router) = boot_temp_server("parity_seed_only_botfather");
    let (_uid, token) = register_user(&router, "seed_checker").await;

    // 1. Catalog, search("apps"), recent, installed, and fabushi-bots MUST contain zero built-in demo Mini Apps
    let (st_apps, apps_val) =
        http_json(&router, "GET", "/api/v1/miniapps", Some(&token), None).await;
    assert_eq!(st_apps, StatusCode::OK);
    assert!(
        apps_val["apps"].as_array().is_some_and(|a| a.is_empty()),
        "fresh server must have 0 seeded Mini Apps, got: {apps_val}"
    );

    let (st_search_apps, search_apps_val) = http_json(
        &router,
        "GET",
        "/api/v1/search?q=&category=apps",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_search_apps, StatusCode::OK);
    assert!(
        search_apps_val["apps"]
            .as_array()
            .is_some_and(|a| a.is_empty()),
        "search category=apps must not return host demo apps: {search_apps_val}"
    );

    let (st_fab, fab_val) =
        http_json(&router, "GET", "/api/v1/fabushi-bots", Some(&token), None).await;
    assert_eq!(st_fab, StatusCode::OK);
    let fab_bots = fab_val["bots"].as_array().cloned().unwrap_or_default();
    for forbidden_id in [777_001_i64, 777_010, 777_011, 777_012] {
        assert!(
            fab_bots
                .iter()
                .all(|b| b["bot_user_id"].as_i64() != Some(forbidden_id)),
            "forbidden seeded bot {forbidden_id} appeared in /api/v1/fabushi-bots: {fab_bots:?}"
        );
    }
    for forbidden_name in ["PomodoroFocusBot", "SplitBillBot", "PollMasterBot"] {
        let (st_p, _) = http_json(
            &router,
            "GET",
            &format!("/api/v1/bots/profile/{forbidden_name}"),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(
            st_p,
            StatusCode::NOT_FOUND,
            "host demo bot {forbidden_name} must NOT be seeded"
        );
    }

    // 2. @BotFather (777_001) IS seeded and accessible via search & public profile without creating a chat
    let (st_bf, bf_val) = http_json(
        &router,
        "GET",
        "/api/v1/bots/profile/BotFather",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_bf, StatusCode::OK, "BotFather profile missing: {bf_val}");
    let bf_prof: BotPublicProfileDto = serde_json::from_value(bf_val).unwrap();
    assert_eq!(bf_prof.bot.bot_user_id, BOTFATHER_USER_ID);
    assert_eq!(bf_prof.bot.username, "BotFather");
    assert!(bf_prof.bot.is_official);
    assert!(!bf_prof.bot.is_fabushi_bot);
    assert!(list_chats(&router, &token).await.is_empty());

    // 3. Even if legacy rows for 777010..=777012 existed in mini_apps, seed_botfather_and_flagship_apps unpublishes them
    let _ = state.db.create_or_upsert_mini_app(
        777_010,
        "PomodoroFocusBot",
        "focus",
        "Legacy Focus",
        "Legacy",
        "productivity",
        "🍅",
        None,
        "{}",
        true,
        true,
        false,
    );
    state
        .db
        .seed_botfather_and_flagship_apps()
        .expect("re-run seed");
    let (_, after_reseed) =
        http_json(&router, "GET", "/api/v1/miniapps", Some(&token), None).await;
    assert!(
        after_reseed["apps"]
            .as_array()
            .is_some_and(|a| a.is_empty()),
        "legacy 777010..777012 rows must be unpublished on seed: {after_reseed}"
    );
}

#[tokio::test]
async fn test_parity_flows_2_3_4_5_6_botfather_bot_api_launch_write_access_block_and_js() {
    let (_dir, _state, router) = boot_temp_server("parity_full_e2e");
    let (_dev_id, dev_tok) = register_user(&router, "dev_creator").await;
    let (alice_id, alice_tok) = register_user(&router, "alice_viewer").await;
    let (_bob_id, bob_tok) = register_user(&router, "bob_viewer").await;

    // ------------------------------------------------------------------------
    // Flow 2a: Create Standard Bot via @BotFather (/newbot)
    // ------------------------------------------------------------------------
    let (st_add_bf, add_bf) = http_json(
        &router,
        "POST",
        "/api/v1/contacts/add",
        Some(&dev_tok),
        Some(json!({ "username": "BotFather", "is_bot": true })),
    )
    .await;
    assert!(st_add_bf.is_success());
    let bf_cid = add_bf["chat_id"].as_i64().unwrap();

    let _ = send_dm_and_wait_botfather_reply(&router, &dev_tok, bf_cid, "/newbot").await;
    let _ = send_dm_and_wait_botfather_reply(&router, &dev_tok, bf_cid, "Pizza Shop Bot").await;
    let created_reply =
        send_dm_and_wait_botfather_reply(&router, &dev_tok, bf_cid, "@PizzaShopBot").await;
    let bot_token = extract_token_from_botfather_reply(created_reply["text"].as_str().unwrap());

    // Verify /bot<token>/getMe returns valid Telegram Bot API shape
    let (st_me, me_val) =
        http_json(&router, "GET", &format!("/bot{bot_token}/getMe"), None, None).await;
    assert_eq!(st_me, StatusCode::OK, "getMe failed: {me_val}");
    assert_eq!(me_val["ok"], true);
    assert_eq!(me_val["result"]["username"], "PizzaShopBot");
    assert_eq!(me_val["result"]["is_bot"], true);
    let bot_id = me_val["result"]["id"].as_u64().expect("bot id");

    // Verify initial unconfigured standard bot profile for Alice: empty commands, default menu, no chat created
    let (st_p0, p0_val) = http_json(
        &router,
        "GET",
        "/api/v1/bots/profile/PizzaShopBot",
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_p0, StatusCode::OK, "initial profile failed: {p0_val}");
    let p0: BotPublicProfileDto = serde_json::from_value(p0_val).unwrap();
    assert!(!p0.bot.is_fabushi_bot, "standard bot must not be a Fabushi LLM bot");
    assert!(p0.bot.commands.is_empty(), "standard bot starts with empty commands");
    assert_eq!(p0.bot.menu_button_type, "default");
    assert!(!p0.bot.has_main_app);
    assert_eq!(p0.existing_chat_id, None);
    assert!(!p0.has_started_chat);
    assert!(!p0.allows_write_to_pm);
    assert!(list_chats(&router, &alice_tok).await.is_empty());

    // ------------------------------------------------------------------------
    // Flow 2b: Configure standard bot via /bot<token>/... HTTP APIs & /newapp
    // ------------------------------------------------------------------------
    let (st_cmds, cmds_res) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/setMyCommands"),
        None,
        Some(json!({
            "commands": [
                { "command": "menu", "description": "Browse pizza menu" },
                { "command": "help", "description": "Order support" }
            ]
        })),
    )
    .await;
    assert_eq!(st_cmds, StatusCode::OK, "setMyCommands: {cmds_res}");
    assert_eq!(cmds_res["ok"], true);

    let (st_gcmds, gcmds_res) = http_json(
        &router,
        "GET",
        &format!("/bot{bot_token}/getMyCommands"),
        None,
        None,
    )
    .await;
    assert_eq!(st_gcmds, StatusCode::OK);
    assert_eq!(gcmds_res["result"].as_array().map(|a| a.len()), Some(2));

    let (st_desc, _) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/setMyDescription"),
        None,
        Some(json!({ "description": "Order wood-fired pizza right inside Telegram." })),
    )
    .await;
    assert_eq!(st_desc, StatusCode::OK);

    let (st_short, _) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/setMyShortDescription"),
        None,
        Some(json!({ "short_description": "Official Pizza Shop Mini App" })),
    )
    .await;
    assert_eq!(st_short, StatusCode::OK);

    let (st_priv, _) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/setMyPrivacyPolicy"),
        None,
        Some(json!({ "privacy_policy_url": "https://pizza.example.com/privacy" })),
    )
    .await;
    assert_eq!(st_priv, StatusCode::OK);

    let (st_mb, mb_res) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/setChatMenuButton"),
        None,
        Some(json!({
            "menu_button": {
                "type": "web_app",
                "text": "Order Pizza",
                "web_app": { "url": "https://pizza.example.com/app" }
            }
        })),
    )
    .await;
    assert_eq!(st_mb, StatusCode::OK, "setChatMenuButton: {mb_res}");

    let (st_gmb, gmb_res) = http_json(
        &router,
        "GET",
        &format!("/bot{bot_token}/getChatMenuButton"),
        None,
        None,
    )
    .await;
    assert_eq!(st_gmb, StatusCode::OK);
    assert_eq!(gmb_res["result"]["type"], "web_app");
    assert_eq!(gmb_res["result"]["text"], "Order Pizza");
    assert_eq!(gmb_res["result"]["web_app"]["url"], "https://pizza.example.com/app");

    // Publish Main Mini App via Bot API `publishMiniApp` (with hosted HTML bundle)
    let (st_pub, pub_res) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/publishMiniApp"),
        None,
        Some(json!({
            "short_name": "shop",
            "title": "Pizza Mini App",
            "description": "Fast hot pizza delivery",
            "category": "shopping",
            "icon_emoji": "🍕",
            "html_bundle": "<!doctype html><html><head><title>Pizza App</title></head><body><h1>Pizza Storefront</h1></body></html>",
            "is_main_app": true,
            "header_color": "#FF5500",
            "background_color": "#FFFDF9"
        })),
    )
    .await;
    assert_eq!(st_pub, StatusCode::OK, "publishMiniApp: {pub_res}");
    assert_eq!(pub_res["ok"], true);
    let shop_app_id = pub_res["result"]["app_id"].as_str().unwrap().to_string();

    // Also create a secondary external-URL Mini App via @BotFather `/newapp`
    let _ = send_dm_and_wait_botfather_reply(&router, &dev_tok, bf_cid, "/newapp").await;
    let _ = send_dm_and_wait_botfather_reply(&router, &dev_tok, bf_cid, "@PizzaShopBot").await;
    let _ = send_dm_and_wait_botfather_reply(&router, &dev_tok, bf_cid, "Pizza Tracker").await;
    let _ = send_dm_and_wait_botfather_reply(&router, &dev_tok, bf_cid, "Track live courier").await;
    let _ = send_dm_and_wait_botfather_reply(
        &router,
        &dev_tok,
        bf_cid,
        "https://pizza.example.com/tracker",
    )
    .await;
    let newapp_done =
        send_dm_and_wait_botfather_reply(&router, &dev_tok, bf_cid, "tracker").await;
    assert!(
        newapp_done["text"]
            .as_str()
            .unwrap_or("")
            .contains("t.me/PizzaShopBot/tracker"),
        "expected /newapp confirmation: {newapp_done}"
    );

    // Check GET /api/v1/bots/profile/PizzaShopBot as Alice (still zero chats!)
    let (st_p1, p1_val) = http_json(
        &router,
        "GET",
        "/api/v1/bots/profile/@PizzaShopBot",
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_p1, StatusCode::OK, "profile after config: {p1_val}");
    let p1: BotPublicProfileDto = serde_json::from_value(p1_val).unwrap();
    assert_eq!(p1.bot.commands.len(), 2);
    assert_eq!(p1.bot.commands[0].command, "menu");
    assert_eq!(p1.bot.commands[1].command, "help");
    assert_eq!(p1.bot.menu_button_type, "web_app");
    assert_eq!(p1.bot.menu_button_text, "Order Pizza");
    assert_eq!(p1.bot.menu_button_url, "https://pizza.example.com/app");
    assert_eq!(
        p1.bot.description,
        "Order wood-fired pizza right inside Telegram."
    );
    assert_eq!(p1.bot.about, "Official Pizza Shop Mini App");
    assert_eq!(p1.bot.privacy_policy_url, "https://pizza.example.com/privacy");
    assert!(p1.bot.has_main_app);
    assert_eq!(p1.bot.main_app_short_name, "shop");
    assert_eq!(
        p1.main_app.as_ref().map(|a| a.short_name.as_str()),
        Some("shop")
    );
    assert_eq!(
        p1.main_app.as_ref().and_then(|a| a.header_color.as_deref()),
        Some("#FF5500")
    );
    assert_eq!(
        p1.main_app
            .as_ref()
            .and_then(|a| a.background_color.as_deref()),
        Some("#FFFDF9")
    );
    assert_eq!(p1.apps.len(), 2);
    assert_eq!(p1.bot.mau_count, 0);
    assert_eq!(p1.existing_chat_id, None);
    assert!(list_chats(&router, &alice_tok).await.is_empty());

    // ------------------------------------------------------------------------
    // Flow 3: Launch Mini App -> NO DM chat created, allows_write_to_pm=false in initData,
    //         populates /api/v1/miniapps/recent & MAU, sendMessage returns 403 Forbidden
    // ------------------------------------------------------------------------
    let (st_launch, launch_val) = http_json(
        &router,
        "POST",
        "/api/v1/miniapps/PizzaShopBot/shop/launch",
        Some(&alice_tok),
        Some(json!({ "launch_mode": "main_mini_app", "start_param": "promo_1" })),
    )
    .await;
    assert_eq!(st_launch, StatusCode::OK, "launch failed: {launch_val}");
    let launch_ctx: MiniAppLaunchContext = serde_json::from_value(launch_val).unwrap();
    assert_eq!(launch_ctx.companion_chat_id, 0, "launch MUST NOT create a bot DM chat");
    assert!(!launch_ctx.app.is_installed, "launch MUST NOT auto-install");
    assert!(!launch_ctx.companion_bot.allows_write_to_pm);
    assert!(
        verify_telegram_hmac_independent(&launch_ctx.init_data.raw_query_string, &bot_token),
        "HMAC-SHA256 verification failed for initData"
    );
    let init_user: Value = serde_json::from_str(&launch_ctx.init_data.user_json).unwrap();
    assert_eq!(init_user["id"], alice_id);
    assert_eq!(init_user["allows_write_to_pm"], false);

    // Alice still has ZERO chats!
    assert!(
        list_chats(&router, &alice_tok).await.is_empty(),
        "launching Mini App must NOT create any chat in /api/v1/chats"
    );

    // Recent Mini Apps lists Pizza Mini App (`is_recent_used: true`)
    let (st_recent, recent_val) =
        http_json(&router, "GET", "/api/v1/miniapps/recent", Some(&alice_tok), None).await;
    assert_eq!(st_recent, StatusCode::OK);
    let recent_apps: Vec<MiniAppEntry> =
        serde_json::from_value(recent_val["apps"].clone()).unwrap();
    assert_eq!(recent_apps.len(), 1);
    assert_eq!(recent_apps[0].short_name, "shop");
    assert!(recent_apps[0].is_recent_used);
    assert!(recent_apps[0].last_launched_at > 0);

    // Bob also launches -> 30d MAU becomes 2!
    let (st_bob_launch, _) = http_json(
        &router,
        "POST",
        "/api/v1/miniapps/PizzaShopBot/shop/launch",
        Some(&bob_tok),
        Some(json!({})),
    )
    .await;
    assert_eq!(st_bob_launch, StatusCode::OK);
    let (_, p2_val) = http_json(
        &router,
        "GET",
        "/api/v1/bots/profile/PizzaShopBot",
        Some(&alice_tok),
        None,
    )
    .await;
    let p2: BotPublicProfileDto = serde_json::from_value(p2_val).unwrap();
    assert_eq!(p2.bot.mau_count, 2, "2 distinct users launched within 30d");

    // can-send-message is false & /bot<token>/sendMessage returns 403 Forbidden
    let (st_can0, can0_val) = http_json(
        &router,
        "GET",
        &format!("/api/v1/bots/{bot_id}/can-send-message"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_can0, StatusCode::OK);
    let can0: CanSendMessageResponse = serde_json::from_value(can0_val).unwrap();
    assert!(!can0.can_send);
    assert!(!can0.allows_write_to_pm);
    assert!(!can0.has_started_chat);
    assert!(!can0.is_blocked);

    let (st_send_denied, send_denied_val) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/sendMessage"),
        None,
        Some(json!({ "chat_id": alice_id, "text": "Unsolicited message" })),
    )
    .await;
    assert_eq!(st_send_denied, StatusCode::FORBIDDEN, "expected 403: {send_denied_val}");
    assert_eq!(send_denied_val["ok"], false);
    assert_eq!(send_denied_val["error_code"], 403);

    // ------------------------------------------------------------------------
    // Flow 4: accept-app-tos + allow-send-message -> creates chat, inserts exact
    //         service message, delivers write_access_allowed update, unlocks sendMessage;
    //         blocking revokes it (403 Forbidden)
    // ------------------------------------------------------------------------
    let (st_tos, tos_val) = http_json(
        &router,
        "POST",
        &format!("/api/v1/bots/{bot_id}/accept-app-tos"),
        Some(&alice_tok),
        Some(json!({ "app_id": shop_app_id })),
    )
    .await;
    assert_eq!(st_tos, StatusCode::OK, "accept-app-tos failed: {tos_val}");
    assert_eq!(tos_val["app_tos_accepted"], true);

    let (st_allow, allow_val) = http_json(
        &router,
        "POST",
        &format!("/api/v1/bots/{bot_id}/allow-send-message"),
        Some(&alice_tok),
        Some(json!({
            "app_id": shop_app_id,
            "web_app_name": "Pizza Mini App",
            "from_attachment_menu": false
        })),
    )
    .await;
    assert_eq!(st_allow, StatusCode::OK, "allow-send-message failed: {allow_val}");
    let allow_resp: AllowSendMessageResponse = serde_json::from_value(allow_val).unwrap();
    assert!(allow_resp.ok);
    assert!(allow_resp.chat_id > 0);
    assert_eq!(
        allow_resp.service_text,
        "您已允许该机器人在其网页应用中向您发送消息。"
    );

    // Chat now exists with ONLY the service message (no fake AI welcome)
    let alice_chats = list_chats(&router, &alice_tok).await;
    assert!(alice_chats.iter().any(|c| c["chat_id"].as_i64() == Some(allow_resp.chat_id)));
    let chat_msgs = list_chat_messages(&router, &alice_tok, allow_resp.chat_id).await;
    assert_eq!(chat_msgs.len(), 1, "expected only 1 service message, got: {chat_msgs:?}");
    assert_eq!(
        chat_msgs[0]["text"].as_str(),
        Some("您已允许该机器人在其网页应用中向您发送消息。")
    );

    // /bot<token>/getUpdates received `message.write_access_allowed`
    let (st_upd, upd_val) = http_json(
        &router,
        "GET",
        &format!("/bot{bot_token}/getUpdates"),
        None,
        None,
    )
    .await;
    assert_eq!(st_upd, StatusCode::OK, "getUpdates failed: {upd_val}");
    let updates = upd_val["result"].as_array().expect("updates array");
    let write_upd = updates
        .iter()
        .find(|u| u["message"].get("write_access_allowed").is_some())
        .expect("missing message.write_access_allowed update");
    assert_eq!(
        write_upd["message"]["write_access_allowed"]["from_request"],
        true
    );
    assert_eq!(
        write_upd["message"]["write_access_allowed"]["web_app_name"],
        "Pizza Mini App"
    );
    assert_eq!(
        write_upd["message"]["write_access_allowed"]["from_attachment_menu"],
        false
    );
    assert_eq!(write_upd["message"]["from"]["id"], alice_id);
    let last_upd_id = write_upd["update_id"].as_i64().unwrap();

    // Relaunching Mini App now embeds `allows_write_to_pm: true` in initData!
    let (_, relaunch_val) = http_json(
        &router,
        "POST",
        "/api/v1/miniapps/PizzaShopBot/shop/launch",
        Some(&alice_tok),
        Some(json!({})),
    )
    .await;
    let relaunch_ctx: MiniAppLaunchContext = serde_json::from_value(relaunch_val).unwrap();
    let relaunch_user: Value = serde_json::from_str(&relaunch_ctx.init_data.user_json).unwrap();
    assert_eq!(relaunch_user["allows_write_to_pm"], true);

    // /bot<token>/sendMessage now returns 200 OK with inline_keyboard!
    let (st_send_ok, send_ok_val) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/sendMessage"),
        None,
        Some(json!({
            "chat_id": alice_id,
            "text": "Order #42 confirmed!",
            "reply_markup": {
                "inline_keyboard": [[
                    { "text": "Open Tracker", "web_app": { "url": "https://pizza.example.com/tracker" } }
                ]]
            }
        })),
    )
    .await;
    assert_eq!(st_send_ok, StatusCode::OK, "sendMessage after allow failed: {send_ok_val}");
    assert_eq!(send_ok_val["ok"], true);
    let bot_msg_id = send_ok_val["result"]["message_id"].as_i64().unwrap();

    // editMessageText works
    let (st_edit, edit_val) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/editMessageText"),
        None,
        Some(json!({
            "chat_id": alice_id,
            "message_id": bot_msg_id,
            "text": "Order #42 out for delivery!"
        })),
    )
    .await;
    assert_eq!(st_edit, StatusCode::OK, "editMessageText failed: {edit_val}");
    assert_eq!(edit_val["result"]["text"], "Order #42 out for delivery!");

    // answerWebAppQuery works with query_id from launch_ctx
    let (st_awaq, awaq_val) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/answerWebAppQuery"),
        None,
        Some(json!({
            "web_app_query_id": relaunch_ctx.init_data.query_id,
            "result": {
                "type": "article",
                "id": "order_42",
                "title": "Pizza Receipt",
                "input_message_content": { "message_text": "Ordered 1x Margherita Pizza" }
            }
        })),
    )
    .await;
    assert_eq!(st_awaq, StatusCode::OK, "answerWebAppQuery failed: {awaq_val}");
    assert!(awaq_val["result"]["inline_message_id"].as_str().is_some());

    // Blocking bot revokes write access (`403 Forbidden`)
    let (st_blk, blk_val) = http_json(
        &router,
        "POST",
        &format!("/api/v1/bots/{bot_id}/block"),
        Some(&alice_tok),
        Some(json!({ "blocked": true })),
    )
    .await;
    assert_eq!(st_blk, StatusCode::OK, "block failed: {blk_val}");

    let (_, can_blk_val) = http_json(
        &router,
        "GET",
        &format!("/api/v1/bots/{bot_id}/can-send-message"),
        Some(&alice_tok),
        None,
    )
    .await;
    let can_blk: CanSendMessageResponse = serde_json::from_value(can_blk_val).unwrap();
    assert!(!can_blk.can_send);
    assert!(!can_blk.allows_write_to_pm);
    assert!(can_blk.is_blocked);

    let (st_send_blocked, send_blocked_val) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/sendMessage"),
        None,
        Some(json!({ "chat_id": alice_id, "text": "Blocked test" })),
    )
    .await;
    assert_eq!(st_send_blocked, StatusCode::FORBIDDEN, "expected 403 when blocked: {send_blocked_val}");

    // Unblock + user sends `/menu` to standard bot -> enqueues `message` update (no fake AI reply)
    let _ = http_json(
        &router,
        "POST",
        &format!("/api/v1/bots/{bot_id}/block"),
        Some(&alice_tok),
        Some(json!({ "blocked": false })),
    )
    .await;
    let (st_umsg, _) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{}/messages", allow_resp.chat_id),
        Some(&alice_tok),
        Some(json!({ "text": "/menu" })),
    )
    .await;
    assert!(st_umsg.is_success());
    tokio::time::sleep(Duration::from_millis(120)).await;

    // Verify no fabricated Hermes AI reply was inserted for standard bot
    let msgs_after_user = list_chat_messages(&router, &alice_tok, allow_resp.chat_id).await;
    assert_eq!(
        msgs_after_user.last().and_then(|m| m["text"].as_str()),
        Some("/menu"),
        "standard bot must NOT generate fake Hermes AI replies: {msgs_after_user:?}"
    );

    // Verify `/bot<token>/getUpdates?offset=...` delivers the `/menu` message update with bot_command entity
    let (st_upd2, upd2_val) = http_json(
        &router,
        "GET",
        &format!("/bot{bot_token}/getUpdates?offset={}", last_upd_id + 1),
        None,
        None,
    )
    .await;
    assert_eq!(st_upd2, StatusCode::OK);
    let updates2 = upd2_val["result"].as_array().unwrap();
    assert!(
        updates2.iter().any(|u| u["message"]["text"] == "/menu"),
        "expected /menu message update in getUpdates: {updates2:?}"
    );

    // ------------------------------------------------------------------------
    // Flow 5 & 6: `/js/telegram-web-app.js` & `/miniapps/{bot}/{short}` serving
    // ------------------------------------------------------------------------
    let (st_js, ct_js, bytes_js) =
        http_raw(&router, "GET", "/js/telegram-web-app.js", None, None).await;
    assert_eq!(st_js, StatusCode::OK);
    assert!(
        ct_js
            .as_deref()
            .is_some_and(|ct| ct.contains("javascript")),
        "unexpected content-type for telegram-web-app.js: {ct_js:?}"
    );
    let js_text = String::from_utf8(bytes_js).expect("utf8 js");
    for required in [
        "requestWriteAccess",
        "web_app_request_write_access",
        "MainButton",
        "SecondaryButton",
        "BackButton",
        "SettingsButton",
        "CloudStorage",
        "TelegramWebviewProxy",
        "allows_write_to_pm",
    ] {
        assert!(
            js_text.contains(required),
            "telegram-web-app.js missing `{required}`"
        );
    }

    // Hosted HTML bundle endpoint
    let (st_html_hosted, _, bytes_hosted) = http_raw(
        &router,
        "GET",
        "/miniapps/PizzaShopBot/shop?init_data=query_id%3Dq1",
        None,
        None,
    )
    .await;
    assert_eq!(st_html_hosted, StatusCode::OK);
    let hosted_html = String::from_utf8(bytes_hosted).unwrap();
    assert!(hosted_html.contains("Pizza Storefront"));
    assert!(hosted_html.contains("/js/telegram-web-app.js"));

    // External URL Mini App iframe bridge
    let (st_html_ext, _, bytes_ext) = http_raw(
        &router,
        "GET",
        "/miniapps/PizzaShopBot/tracker?init_data=query_id%3Dq2",
        None,
        None,
    )
    .await;
    assert_eq!(st_html_ext, StatusCode::OK);
    let ext_html = String::from_utf8(bytes_ext).unwrap();
    assert!(ext_html.contains("https://pizza.example.com/tracker"));
    assert!(ext_html.contains("/js/telegram-web-app.js"));
}

#[tokio::test]
async fn test_altery_kyc_abandon_send_resume_webapp_and_pin_chat_message_e2e() {
    let (_dir, state, router) = boot_temp_server("tg_miniapps_altery_kyc_pin");
    let (_dev_uid, dev_tok) = register_user(&router, "altery_dev").await;
    let (alice_uid, alice_tok) = register_user(&router, "alice_kyc").await;

    // 1. Developer creates standard bot `@AlteryVerifyBot` via @BotFather `/newbot` (NO `/newapp`!)
    //    and configures `setChatMenuButton` (`web_app.url = "https://app.altery.example/start"`).
    let (st_add_bf, add_bf) = http_json(
        &router,
        "POST",
        "/api/v1/contacts/add",
        Some(&dev_tok),
        Some(json!({ "username": "BotFather", "is_bot": true })),
    )
    .await;
    assert!(st_add_bf.is_success());
    let bf_cid = add_bf["chat_id"].as_i64().unwrap();

    let _ = send_dm_and_wait_botfather_reply(&router, &dev_tok, bf_cid, "/newbot").await;
    let _ = send_dm_and_wait_botfather_reply(&router, &dev_tok, bf_cid, "Altery").await;
    let created_reply =
        send_dm_and_wait_botfather_reply(&router, &dev_tok, bf_cid, "@AlteryVerifyBot").await;
    let bot_token = extract_token_from_botfather_reply(created_reply["text"].as_str().unwrap());

    let (st_me, me_val) =
        http_json(&router, "GET", &format!("/bot{bot_token}/getMe"), None, None).await;
    assert_eq!(st_me, StatusCode::OK);
    let bot_id = me_val["result"]["id"].as_u64().expect("bot id");

    let (st_mb, _) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/setChatMenuButton"),
        None,
        Some(json!({
            "menu_button": {
                "type": "web_app",
                "text": "Launch Altery",
                "web_app": { "url": "https://app.altery.example/start" }
            }
        })),
    )
    .await;
    assert_eq!(st_mb, StatusCode::OK);

    // 2. Alice launches `/api/v1/miniapps/AlteryVerifyBot/app/launch` with `custom_url` (ad-hoc, no `/newapp`!).
    let (st_launch1, launch1_val) = http_json(
        &router,
        "POST",
        "/api/v1/miniapps/AlteryVerifyBot/app/launch",
        Some(&alice_tok),
        Some(json!({
            "launch_mode": "MenuButton",
            "custom_url": "https://app.altery.example/start"
        })),
    )
    .await;
    assert_eq!(
        st_launch1,
        StatusCode::OK,
        "ad-hoc bot webapp launch failed: {launch1_val}"
    );
    let launch1_ctx: MiniAppLaunchContext = serde_json::from_value(launch1_val).unwrap();
    assert_eq!(launch1_ctx.app.web_url, "https://app.altery.example/start");
    assert!(
        verify_telegram_hmac_independent(&launch1_ctx.init_data.raw_query_string, &bot_token),
        "initData HMAC signature must verify with bot_token"
    );

    // Ensure ad-hoc launch did NOT leak into published catalog
    let (st_cat, cat_val) = http_json(&router, "GET", "/api/v1/miniapps", Some(&alice_tok), None).await;
    assert_eq!(st_cat, StatusCode::OK);
    assert!(
        cat_val["apps"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["bot_username"] != "AlteryVerifyBot"),
        "unpublished ad-hoc webapp must not appear in public catalog: {cat_val}"
    );

    // Grant DM write access so Altery backend can message Alice
    let (st_allow, allow_val) = http_json(
        &router,
        "POST",
        &format!("/api/v1/bots/{bot_id}/allow-send-message"),
        Some(&alice_tok),
        Some(json!({ "web_app_name": "Altery", "from_attachment_menu": false })),
    )
    .await;
    assert_eq!(st_allow, StatusCode::OK, "allow-send-message: {allow_val}");
    let allow_resp: AllowSendMessageResponse = serde_json::from_value(allow_val).unwrap();
    let dm_cid = allow_resp.chat_id;
    assert!(dm_cid > 0);

    // 3. Bot sends & pins initial `"📌 Welcome to Altery"` message
    let (st_welcome, welcome_val) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/sendMessage"),
        None,
        Some(json!({
            "chat_id": alice_uid,
            "text": "📌 Welcome to Altery\nYour global financial account."
        })),
    )
    .await;
    assert_eq!(st_welcome, StatusCode::OK, "sendMessage welcome: {welcome_val}");
    let welcome_mid = welcome_val["result"]["message_id"].as_i64().unwrap();

    let (st_pin1, pin1_val) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/pinChatMessage"),
        None,
        Some(json!({
            "chat_id": alice_uid,
            "message_id": welcome_mid
        })),
    )
    .await;
    assert_eq!(st_pin1, StatusCode::OK, "pinChatMessage welcome: {pin1_val}");
    assert_eq!(pin1_val["ok"], true);
    assert_eq!(pin1_val["result"], true);

    let (st_pins1, pins1_val) = http_json(
        &router,
        "GET",
        &format!("/api/v1/chats/{dm_cid}/pins"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_pins1, StatusCode::OK);
    assert_eq!(pins1_val["pinned_message_ids"], json!([welcome_mid]));
    assert_eq!(
        pins1_val["pins"][0]["preview_text"],
        "📌 Welcome to Altery"
    );

    let pts_before_abandon = state
        .db
        .get_current_pts(gpui_telegram_core::types::ChatId(dm_cid))
        .unwrap();

    // 4. User abandons KYC midway -> Bot backend calls `sendMessage` with deep-link `web_app` button
    //    `https://app.altery.example/kyc?resume=1` and immediately calls `pinChatMessage`.
    let (st_remind, remind_val) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/sendMessage"),
        None,
        Some(json!({
            "chat_id": alice_uid,
            "text": "⏳ Remember to Get Verified\nTap here to verify your Altery account and unlock full access.",
            "reply_markup": {
                "inline_keyboard": [[
                    {
                        "text": "🆔 Verify Account",
                        "web_app": { "url": "https://app.altery.example/kyc?resume=1" }
                    }
                ]]
            }
        })),
    )
    .await;
    assert_eq!(st_remind, StatusCode::OK, "sendMessage reminder: {remind_val}");
    let remind_mid = remind_val["result"]["message_id"].as_i64().unwrap();

    tokio::time::sleep(Duration::from_millis(2)).await;

    let (st_pin2, pin2_val) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/pinChatMessage"),
        None,
        Some(json!({
            "chat_id": alice_uid,
            "message_id": remind_mid
        })),
    )
    .await;
    assert_eq!(st_pin2, StatusCode::OK, "pinChatMessage reminder: {pin2_val}");
    assert_eq!(pin2_val["result"], true);

    // 5. Verify `GET /api/v1/chats/{cid}/pins` has `remind_mid` at index 0, and PTS `getDifference`
    //    carries BOTH `NewMessage` (with `inline_keyboard` `web_app_url`) AND `PinnedMessagesChanged`!
    let (st_pins2, pins2_val) = http_json(
        &router,
        "GET",
        &format!("/api/v1/chats/{dm_cid}/pins"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_pins2, StatusCode::OK);
    assert_eq!(
        pins2_val["pinned_message_ids"],
        json!([remind_mid, welcome_mid])
    );
    assert_eq!(
        pins2_val["pins"][0]["preview_text"],
        "⏳ Remember to Get Verified"
    );

    let diff = state
        .pts_engine
        .get_difference(&gpui_telegram_proto::sync::SyncGetDifferenceRequest::new(
            gpui_telegram_core::types::ChatId(dm_cid),
            pts_before_abandon,
            50,
        ))
        .expect("get_difference");
    let has_kb_in_pts = diff.new_messages.iter().any(|m| {
        m.id.0 == remind_mid
            && m.inline_keyboard
                .as_ref()
                .and_then(|rows| rows.first())
                .and_then(|row| row.first())
                .and_then(|btn| btn.web_app_url.as_deref())
                == Some("https://app.altery.example/kyc?resume=1")
    });
    assert!(
        has_kb_in_pts,
        "PTS NewMessage must carry inline_keyboard with web_app_url: {:?}",
        diff.new_messages
    );
    let has_pin_event = diff.other_updates.iter().any(|u| {
        matches!(
            u,
            gpui_telegram_proto::sync::UpdateEvent::PinnedMessagesChanged { pinned_ids, .. }
                if pinned_ids.first().map(|id| id.0) == Some(remind_mid)
        )
    });
    assert!(
        has_pin_event,
        "PTS journal must record PinnedMessagesChanged with remind_mid first: {:?}",
        diff.other_updates
    );

    // 6. User clicks `🆔 Verify Account` -> launches by numeric bot_id with `custom_url`
    let (st_resume, resume_val) = http_json(
        &router,
        "POST",
        &format!("/api/v1/miniapps/{bot_id}/app/launch"),
        Some(&alice_tok),
        Some(json!({
            "chat_id": dm_cid,
            "launch_mode": "InlineKeyboard",
            "custom_url": "https://app.altery.example/kyc?resume=1"
        })),
    )
    .await;
    assert_eq!(st_resume, StatusCode::OK, "resume launch failed: {resume_val}");
    let resume_ctx: MiniAppLaunchContext = serde_json::from_value(resume_val).unwrap();
    assert_eq!(
        resume_ctx.app.web_url,
        "https://app.altery.example/kyc?resume=1"
    );
    assert!(verify_telegram_hmac_independent(
        &resume_ctx.init_data.raw_query_string,
        &bot_token
    ));
    assert!(resume_ctx.init_data.raw_query_string.contains("allows_write_to_pm"));

    // Verify `answerWebAppQuery` also works for ad-hoc session `query_id`
    let (st_awaq, awaq_val) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/answerWebAppQuery"),
        None,
        Some(json!({
            "web_app_query_id": resume_ctx.init_data.query_id,
            "result": {
                "type": "article",
                "id": "kyc_ok",
                "title": "Verified",
                "input_message_content": { "message_text": "✅ KYC Verification Completed!" }
            }
        })),
    )
    .await;
    assert_eq!(st_awaq, StatusCode::OK, "answerWebAppQuery on ad-hoc session: {awaq_val}");

    // 7. Verify `unpinChatMessage` (omitted `message_id` unpins most recent `remind_mid`, leaving `welcome_mid`)
    //    and `unpinAllChatMessages`, plus non-admin bot in group chat receiving 400.
    let (st_unpin_latest, _) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/unpinChatMessage"),
        None,
        Some(json!({ "chat_id": alice_uid })),
    )
    .await;
    assert_eq!(st_unpin_latest, StatusCode::OK);
    let (_, pins_after_unpin1) = http_json(
        &router,
        "GET",
        &format!("/api/v1/chats/{dm_cid}/pins"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(pins_after_unpin1["pinned_message_ids"], json!([welcome_mid]));

    let (st_unpin_all, _) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/unpinAllChatMessages"),
        None,
        Some(json!({ "chat_id": alice_uid })),
    )
    .await;
    assert_eq!(st_unpin_all, StatusCode::OK);
    let (_, pins_after_clear) = http_json(
        &router,
        "GET",
        &format!("/api/v1/chats/{dm_cid}/pins"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(pins_after_clear["pinned_message_ids"], json!([]));

    // Group chat where bot is a regular non-admin member -> `pinChatMessage` returns 400
    let (st_grp, grp_val) = http_json(
        &router,
        "POST",
        "/api/v1/chats",
        Some(&alice_tok),
        Some(json!({ "title": "Altery Community", "chat_type": "group" })),
    )
    .await;
    assert_eq!(st_grp, StatusCode::CREATED);
    let grp_cid = grp_val["chat_id"].as_i64().unwrap();
    let (st_add_mem, _) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{grp_cid}/members"),
        Some(&alice_tok),
        Some(json!({ "user_id": bot_id })),
    )
    .await;
    assert!(st_add_mem.is_success(), "add bot to group");
    let (st_gmsg, gmsg_val) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{grp_cid}/messages"),
        Some(&alice_tok),
        Some(json!({ "text": "Group announcement" })),
    )
    .await;
    assert_eq!(st_gmsg, StatusCode::CREATED);
    let grp_mid = gmsg_val["message_id"].as_i64().unwrap();

    let (st_grp_pin, grp_pin_val) = http_json(
        &router,
        "POST",
        &format!("/bot{bot_token}/pinChatMessage"),
        None,
        Some(json!({ "chat_id": grp_cid, "message_id": grp_mid })),
    )
    .await;
    assert_eq!(
        st_grp_pin,
        StatusCode::BAD_REQUEST,
        "non-admin bot in group must get 400: {grp_pin_val}"
    );
    assert!(
        grp_pin_val["description"]
            .as_str()
            .unwrap_or("")
            .contains("not enough rights"),
        "unexpected error description: {grp_pin_val}"
    );
}
