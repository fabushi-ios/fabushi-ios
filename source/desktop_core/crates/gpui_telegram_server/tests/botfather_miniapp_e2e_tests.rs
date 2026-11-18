use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use gpui_telegram_proto::miniapp_contracts::{
    BotRegistryEntry, MiniAppActionRequest, MiniAppActionResponse,
    MiniAppEntry, MiniAppLaunchContext, NaturalLanguageCreateResponse,
    PeerBotActionResult, ValidateInitDataRequest, ValidateInitDataResponse,
    BOTFATHER_USER_ID, CLOUD_STORAGE_MAX_KEY_BYTES, CLOUD_STORAGE_MAX_VALUE_BYTES,
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

    fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn build_temp_server_config(dir: &std::path::Path) -> ServerConfig {
    let db_path = dir.join("e2e_botfather_miniapps.db");
    let media_dir = dir.join("media_cas");
    std::fs::create_dir_all(&media_dir).expect("create media dir");
    let mut cfg = ServerConfig::default();
    cfg.database_url = format!("sqlite://{}", db_path.display());
    cfg.media_storage_path = media_dir.to_string_lossy().to_string();
    cfg.host = "127.0.0.1".to_string();
    cfg.port = 0;
    cfg
}

fn boot_state_and_router(cfg: ServerConfig) -> (Arc<AppState>, Router) {
    let state = Arc::new(AppState::try_new(cfg).expect("boot AppState with real SQLite + HermesEngine"));
    let router = build_router_with_state(state.clone());
    (state, router)
}

async fn http_bytes(
    router: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Vec<u8>) {
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
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, bytes.to_vec())
}

async fn http_json(
    router: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, bytes) = http_bytes(router, method, uri, token, body).await;
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
            "password": "E2eStrongPassword!2026",
            "first_name": username,
            "last_name": "E2E",
            "device_name": "E2ETestDesktop"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "register {username}: {val}");
    (
        val["user_id"].as_u64().expect("user_id"),
        val["token"].as_str().expect("token").to_string(),
    )
}

async fn login_user(router: &Router, username: &str) -> (u64, String) {
    let (status, val) = http_json(
        router,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({
            "username": username,
            "password": "E2eStrongPassword!2026",
            "device_name": "E2EReconnect"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login {username}: {val}");
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
    assert_eq!(status, StatusCode::OK, "list messages for {chat_id}: {val}");
    val["messages"].as_array().cloned().unwrap_or_default()
}

async fn send_dm_and_wait_bot_reply(
    router: &Router,
    token: &str,
    chat_id: i64,
    text: &str,
) -> (Value, Vec<Value>) {
    let (st, send_res) = http_json(
        router,
        "POST",
        &format!("/api/v1/chats/{chat_id}/messages"),
        Some(token),
        Some(json!({ "text": text })),
    )
    .await;
    assert!(
        st.is_success(),
        "send message '{text}' to chat {chat_id} failed: {st} {send_res}"
    );
    let sent_id = send_res["message_id"].as_i64().unwrap_or(0);

    for _ in 0..100 {
        let msgs = list_chat_messages(router, token, chat_id).await;
        if let Some(reply) = msgs.iter().rev().find(|m| {
            m["id"].as_i64().unwrap_or(0) > sent_id
                && m["sender_id"].as_u64().is_some_and(|sid| sid >= 500_000)
        }) {
            return (reply.clone(), msgs);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let msgs = list_chat_messages(router, token, chat_id).await;
    panic!(
        "Timed out waiting for bot reply in chat {chat_id} after '{text}' (sent_id={sent_id}); messages={msgs:?}"
    );
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k_pad = [0u8; BLOCK];
    if key.len() > BLOCK {
        let hashed = Sha256::digest(key);
        k_pad[..32].copy_from_slice(&hashed);
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

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn verify_telegram_hmac_independent(raw_query: &str, bot_token: &str) -> bool {
    let mut pairs: BTreeMap<String, String> = BTreeMap::new();
    let mut provided_hash = None;
    for part in raw_query.split('&') {
        if part.is_empty() {
            continue;
        }
        let (k, v) = match part.split_once('=') {
            Some((k, v)) => (k, v),
            None => (part, ""),
        };
        let dec_k = percent_decode(k);
        let dec_v = percent_decode(v);
        if dec_k == "hash" {
            provided_hash = Some(dec_v);
        } else {
            pairs.insert(dec_k, dec_v);
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
    let computed = hex_lower(&hmac_sha256(&secret, check_str.as_bytes()));
    computed == hash_hex.to_ascii_lowercase()
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

fn msg_has_button_with_callback(msg: &Value, expected: &str) -> bool {
    msg["inline_keyboard"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row.as_array())
        .flatten()
        .any(|btn| btn["callback_data"].as_str() == Some(expected))
}

fn msg_has_webapp_button(msg: &Value, expected_substr: &str) -> bool {
    msg["inline_keyboard"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row.as_array())
        .flatten()
        .any(|btn| {
            btn["web_app_url"]
                .as_str()
                .is_some_and(|u| u.contains(expected_substr))
        })
}

#[tokio::test]
async fn test_e2e_botfather_search_onboarding_and_human_id_isolation() {
    let dir = TempDirGuard::new("bf_e2e_onboarding");
    let cfg = build_temp_server_config(dir.path());
    let (state, router) = boot_state_and_router(cfg.clone());

    // 1. Register multiple human users -> must stay < 500_000 and never collide with 777_001 (@BotFather)
    let (alice_id, alice_tok) = register_user(&router, "alice_e2e").await;
    let (bob_id, bob_tok) = register_user(&router, "bob_e2e").await;
    let (carol_id, _) = register_user(&router, "carol_e2e").await;
    for uid in [alice_id, bob_id, carol_id] {
        assert!(
            (100_001..500_000).contains(&uid),
            "human user_id {uid} escaped < 500_000 safe range or collided with 777_001"
        );
        assert_ne!(uid, BOTFATHER_USER_ID as u64);
    }

    // 2. Global search "BotFather" & Chinese "机器人之父"
    let (st_en, search_en) = http_json(
        &router,
        "GET",
        "/api/v1/search?q=BotFather",
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_en, StatusCode::OK, "search BotFather: {search_en}");
    assert!(
        search_en["bots"]
            .as_array()
            .is_some_and(|arr| arr.iter().any(|b| b["bot_user_id"].as_i64() == Some(BOTFATHER_USER_ID as i64))),
        "bots missing @BotFather: {search_en}"
    );
    assert!(
        search_en["results"]
            .as_array()
            .is_some_and(|arr| arr.iter().any(|r| r["username"].as_str() == Some("BotFather"))),
        "results missing @BotFather: {search_en}"
    );

    let (st_zh, search_zh) = http_json(
        &router,
        "GET",
        "/api/v1/search?q=%E6%9C%BA%E5%99%A8%E4%BA%BA%E4%B9%8B%E7%88%B6",
        Some(&bob_tok),
        None,
    )
    .await;
    assert_eq!(st_zh, StatusCode::OK, "search 机器人之父: {search_zh}");
    assert!(
        search_zh["results"]
            .as_array()
            .is_some_and(|arr| arr.iter().any(|r| r["bot_user_id"].as_i64() == Some(BOTFATHER_USER_ID as i64))),
        "Chinese search missing @BotFather: {search_zh}"
    );

    // Resolve peer @BotFather
    let (st_res, resolved) = http_json(
        &router,
        "GET",
        "/api/v1/peers/resolve?username=BotFather",
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_res, StatusCode::OK, "resolve BotFather: {resolved}");
    assert_eq!(resolved["user"]["user_id"].as_u64(), Some(BOTFATHER_USER_ID as u64));

    // 3. Add @BotFather via username and Chinese alias
    let (st_add_a, add_a) = http_json(
        &router,
        "POST",
        "/api/v1/contacts/add",
        Some(&alice_tok),
        Some(json!({ "username": "BotFather", "is_bot": true })),
    )
    .await;
    assert!(st_add_a.is_success(), "add BotFather: {add_a}");
    let bf_chat_a = add_a["chat_id"].as_i64().expect("chat_id");
    assert!(
        (600_000_000..=699_999_999).contains(&bf_chat_a),
        "BotFather DM chat_id {bf_chat_a} out of companion DM range"
    );

    let (st_add_b, add_b) = http_json(
        &router,
        "POST",
        "/api/v1/contacts/add",
        Some(&bob_tok),
        Some(json!({ "first_name": "机器人之父", "username": "机器人之父" })),
    )
    .await;
    assert!(st_add_b.is_success(), "add 机器人之父: {add_b}");
    let bf_chat_b = add_b["chat_id"].as_i64().expect("bob bf chat_id");

    // 4. Verify welcome message + inline buttons
    let msgs_a = list_chat_messages(&router, &alice_tok, bf_chat_a).await;
    assert!(!msgs_a.is_empty(), "expected welcome message from BotFather");
    let welcome = &msgs_a[0];
    assert_eq!(welcome["sender_id"].as_u64(), Some(BOTFATHER_USER_ID as u64));
    let welcome_txt = welcome["text"].as_str().unwrap_or("");
    assert!(
        welcome_txt.contains("/newbot") && welcome_txt.contains("/newapp"),
        "welcome missing /newbot & /newapp: {welcome_txt}"
    );
    for cb in ["bf:/newbot", "bf:/newapp", "bf:/mybots", "bf:/myapps", "bf:nl_guide"] {
        assert!(
            msg_has_button_with_callback(welcome, cb),
            "welcome missing inline button {cb}: {welcome}"
        );
    }

    let msgs_b = list_chat_messages(&router, &bob_tok, bf_chat_b).await;
    assert_eq!(msgs_b[0]["sender_id"].as_u64(), Some(BOTFATHER_USER_ID as u64));

    // 5. Reopen SQLite DB cleanly to prove durable onboarding + ID sequence
    drop(router);
    drop(state);
    let (_state2, router2) = boot_state_and_router(cfg);
    let (dave_id, _) = register_user(&router2, "dave_after_restart").await;
    assert!(
        (100_001..500_000).contains(&dave_id) && dave_id > carol_id,
        "post-restart human id={dave_id} carol={carol_id}"
    );
    let (_uid2, alice_tok2) = login_user(&router2, "alice_e2e").await;
    let persisted_msgs = list_chat_messages(&router2, &alice_tok2, bf_chat_a).await;
    assert!(!persisted_msgs.is_empty());
}

#[tokio::test]
async fn test_e2e_botfather_slash_fsm_newbot_mybots_newapp_and_callbacks() {
    let dir = TempDirGuard::new("bf_e2e_slash_fsm");
    let cfg = build_temp_server_config(dir.path());
    let (_state, router) = boot_state_and_router(cfg);

    let (_alice_id, alice_tok) = register_user(&router, "alice_fsm").await;
    let (_, add_bf) = http_json(
        &router,
        "POST",
        "/api/v1/contacts/add",
        Some(&alice_tok),
        Some(json!({ "username": "BotFather", "is_bot": true })),
    )
    .await;
    let bf_cid = add_bf["chat_id"].as_i64().unwrap();
    let initial_msgs = list_chat_messages(&router, &alice_tok, bf_cid).await;
    let welcome_msg_id = initial_msgs[0]["id"].as_i64().unwrap();

    // 1. Trigger /newbot via inline keyboard callback
    let (cb_st, cb_val) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{bf_cid}/callback"),
        Some(&alice_tok),
        Some(json!({ "message_id": welcome_msg_id, "callback_data": "bf:/newbot" })),
    )
    .await;
    assert_eq!(cb_st, StatusCode::OK, "callback bf:/newbot: {cb_val}");
    let cb_parsed: PeerBotActionResult = serde_json::from_value(cb_val).unwrap();
    assert!(cb_parsed.ok);

    // 2. Send bot display name -> invalid username rejection (`foo`) -> valid `@CoffeeOrderBot`
    let (r_name, _) =
        send_dm_and_wait_bot_reply(&router, &alice_tok, bf_cid, "Coffee Order Assistant").await;
    assert!(
        r_name["text"].as_str().unwrap_or("").contains("username"),
        "expected username prompt, got: {r_name}"
    );

    let (r_bad, _) = send_dm_and_wait_bot_reply(&router, &alice_tok, bf_cid, "foo").await;
    let r_bad_txt = r_bad["text"].as_str().unwrap_or("");
    assert!(
        r_bad_txt.contains('❌') || r_bad_txt.to_lowercase().contains("bot"),
        "expected invalid username rejection for 'foo', got: {r_bad_txt}"
    );

    let (r_good, _) =
        send_dm_and_wait_bot_reply(&router, &alice_tok, bf_cid, "@CoffeeOrderBot").await;
    let r_good_txt = r_good["text"].as_str().unwrap_or("");
    assert!(
        r_good_txt.contains("CoffeeOrderBot") && r_good_txt.contains(':'),
        "expected bot creation + token reply, got: {r_good_txt}"
    );
    assert!(msg_has_button_with_callback(&r_good, "bf:open_bot:CoffeeOrderBot"));
    assert!(msg_has_button_with_callback(&r_good, "bf:newapp_for:CoffeeOrderBot"));

    // Verify /bots/my includes CoffeeOrderBot
    let (my_st, my_bots_val) =
        http_json(&router, "GET", "/api/v1/bots/my", Some(&alice_tok), None).await;
    assert_eq!(my_st, StatusCode::OK);
    let my_bots: Vec<BotRegistryEntry> =
        serde_json::from_value(my_bots_val["bots"].clone()).expect("bots vec");
    assert!(
        my_bots
            .iter()
            .any(|b| b.username == "CoffeeOrderBot"),
        "CoffeeOrderBot missing from /bots/my: {my_bots:?}"
    );

    // 3. Send /mybots and verify interactive list + open_bot/newapp buttons
    let (r_mybots, _) = send_dm_and_wait_bot_reply(&router, &alice_tok, bf_cid, "/mybots").await;
    assert!(
        r_mybots["text"].as_str().unwrap_or("").contains("CoffeeOrderBot"),
        "/mybots missing CoffeeOrderBot: {r_mybots}"
    );
    assert!(
        msg_has_button_with_callback(&r_mybots, "bf:manage_bot:CoffeeOrderBot"),
        "/mybots missing manage_bot button: {r_mybots}"
    );

    // 4. Send /newapp -> bot -> title -> desc -> URL -> short_name -> live Mini App
    let (r_newapp, _) = send_dm_and_wait_bot_reply(&router, &alice_tok, bf_cid, "/newapp").await;
    assert!(msg_has_button_with_callback(&r_newapp, "bf:newapp_for:CoffeeOrderBot"));

    let (r_bot, _) =
        send_dm_and_wait_bot_reply(&router, &alice_tok, bf_cid, "@CoffeeOrderBot").await;
    assert!(r_bot["text"].as_str().unwrap_or("").contains("Title") || r_bot["text"].as_str().unwrap_or("").contains("标题"));

    let _ = send_dm_and_wait_bot_reply(&router, &alice_tok, bf_cid, "精品咖啡点单").await;
    let _ = send_dm_and_wait_bot_reply(&router, &alice_tok, bf_cid, "每日手冲、美式与拿铁点单并同步订单给机器人").await;
    let (r_url_prompt, _) = send_dm_and_wait_bot_reply(&router, &alice_tok, bf_cid, "auto").await;
    assert!(
        r_url_prompt["text"].as_str().unwrap_or("").contains("short_name"),
        "expected short_name prompt: {r_url_prompt}"
    );

    let (r_app_done, _) = send_dm_and_wait_bot_reply(&router, &alice_tok, bf_cid, "order").await;
    assert!(
        r_app_done["text"].as_str().unwrap_or("").contains("t.me/CoffeeOrderBot/order"),
        "expected live Mini App link: {r_app_done}"
    );
    assert!(
        msg_has_webapp_button(&r_app_done, "gpui-miniapp://CoffeeOrderBot/order"),
        "expected WebApp launch button: {r_app_done}"
    );

    // Verify Mini App is published in catalog & HTML5 WebApp endpoint renders
    let (apps_st, apps_val) =
        http_json(&router, "GET", "/api/v1/miniapps?q=order", Some(&alice_tok), None).await;
    assert_eq!(apps_st, StatusCode::OK);
    let apps: Vec<MiniAppEntry> = serde_json::from_value(apps_val["apps"].clone()).unwrap();
    assert!(
        apps.iter()
            .any(|a| a.bot_username == "CoffeeOrderBot" && a.short_name == "order"),
        "published Mini App not found in /api/v1/miniapps: {apps:?}"
    );

    let (html_st, html_bytes) =
        http_bytes(&router, "GET", "/miniapps/CoffeeOrderBot/order", None, None).await;
    assert_eq!(html_st, StatusCode::OK);
    let html = String::from_utf8_lossy(&html_bytes);
    assert!(html.contains("window.Telegram.WebApp"));
    assert!(html.contains("精品咖啡点单"));
}

#[tokio::test]
async fn test_e2e_botfather_nl_creation_and_every_bot_is_ai_bot() {
    let dir = TempDirGuard::new("bf_e2e_nl_ai_bots");
    let cfg = build_temp_server_config(dir.path());
    let (_state, router) = boot_state_and_router(cfg);

    let (_alice_id, alice_tok) = register_user(&router, "alice_nl").await;
    let (_bob_id, bob_tok) = register_user(&router, "bob_nl").await;

    // 1. Alice sends NL creation prompt directly inside @BotFather DM
    let (_, add_bf) = http_json(
        &router,
        "POST",
        "/api/v1/contacts/add",
        Some(&alice_tok),
        Some(json!({ "username": "BotFather", "is_bot": true })),
    )
    .await;
    let bf_cid = add_bf["chat_id"].as_i64().unwrap();

    let nl_prompt = "帮我创建一个任务打卡机器人和打卡小程序，支持每日打卡和积分统计";
    let (nl_reply, _) = send_dm_and_wait_bot_reply(&router, &alice_tok, bf_cid, nl_prompt).await;
    let nl_text = nl_reply["text"].as_str().unwrap_or("");
    assert!(
        nl_text.contains("t.me/") && nl_text.contains('@'),
        "expected NL Bot + Mini App creation reply, got: {nl_text}"
    );
    assert!(
        msg_has_webapp_button(&nl_reply, "gpui-miniapp://"),
        "expected WebApp launch button in NL creation reply: {nl_reply}"
    );

    // Fetch created bot & mini app for Alice
    let (_, my_bots_val) =
        http_json(&router, "GET", "/api/v1/bots/my", Some(&alice_tok), None).await;
    let my_bots: Vec<BotRegistryEntry> =
        serde_json::from_value(my_bots_val["bots"].clone()).unwrap();
    assert_eq!(my_bots.len(), 1, "expected 1 NL-created bot: {my_bots:?}");
    let nl_bot = &my_bots[0];

    let (_, apps_val) =
        http_json(&router, "GET", "/api/v1/miniapps", Some(&alice_tok), None).await;
    let all_apps: Vec<MiniAppEntry> = serde_json::from_value(apps_val["apps"].clone()).unwrap();
    let nl_app = all_apps
        .iter()
        .find(|a| a.bot_username == nl_bot.username)
        .expect("NL-created Mini App must be published");
    assert!(nl_app.created_by_nl, "expected created_by_nl=true");
    assert!(!nl_app.ui_schema.widgets.is_empty(), "expected non-empty NL widgets");

    // 2. Also exercise HTTP POST /api/v1/botfather/nl-create -> creates a Coffee Order Bot + App
    let (nl_http_st, nl_http_val) = http_json(
        &router,
        "POST",
        "/api/v1/botfather/nl-create",
        Some(&alice_tok),
        Some(json!({ "prompt": "帮我创建一个手冲咖啡点单机器人和点单小程序，支持加冰加奶下单" })),
    )
    .await;
    assert_eq!(nl_http_st, StatusCode::CREATED, "nl-create HTTP failed: {nl_http_val}");
    let nl_created: NaturalLanguageCreateResponse = serde_json::from_value(nl_http_val).unwrap();
    assert!(!nl_created.bot_token.is_empty());

    // 3. Another user (Bob) searches for the NL bot, adds it, and chats naturally + commands
    let (s_st, s_val) = http_json(
        &router,
        "GET",
        &format!("/api/v1/search?q={}", nl_bot.username),
        Some(&bob_tok),
        None,
    )
    .await;
    assert_eq!(s_st, StatusCode::OK);
    let bots_found: Vec<BotRegistryEntry> = serde_json::from_value(s_val["bots"].clone()).unwrap();
    assert!(
        bots_found.iter().any(|b| b.username == nl_bot.username),
        "Bob could not discover {} via global search: {s_val}",
        nl_bot.username
    );

    let (add_bot_st, add_bot_val) = http_json(
        &router,
        "POST",
        "/api/v1/contacts/add",
        Some(&bob_tok),
        Some(json!({ "username": nl_bot.username, "is_bot": true })),
    )
    .await;
    assert!(add_bot_st.is_success(), "Bob add companion bot: {add_bot_val}");
    let bob_bot_cid = add_bot_val["chat_id"].as_i64().unwrap();

    // Verify welcome message exists with WebApp launch button
    let initial_bot_msgs = list_chat_messages(&router, &bob_tok, bob_bot_cid).await;
    assert!(!initial_bot_msgs.is_empty());
    assert!(
        msg_has_webapp_button(
            &initial_bot_msgs[0],
            &format!("gpui-miniapp://{}/{}", nl_bot.username, nl_app.short_name)
        ),
        "companion welcome missing WebApp button: {:?}",
        initial_bot_msgs[0]
    );

    // Send natural language chat message -> persona AI reply + Mini App launch button
    let (ai_reply, _) = send_dm_and_wait_bot_reply(
        &router,
        &bob_tok,
        bob_bot_cid,
        "今天我想完成每日打卡并看下我的积分统计，怎么操作？",
    )
    .await;
    let ai_txt = ai_reply["text"].as_str().unwrap_or("");
    assert!(!ai_txt.trim().is_empty(), "empty AI bot reply");
    assert!(
        msg_has_webapp_button(
            &ai_reply,
            &format!("gpui-miniapp://{}/{}", nl_bot.username, nl_app.short_name)
        ),
        "AI bot reply missing Mini App launch button: {ai_reply}"
    );

    // Send /help and /app commands -> non-empty command replies
    let (help_reply, _) = send_dm_and_wait_bot_reply(&router, &bob_tok, bob_bot_cid, "/help").await;
    let help_txt = help_reply["text"].as_str().unwrap_or("");
    assert!(
        help_txt.contains("/app") || help_txt.contains(&nl_bot.username),
        "unexpected /help reply: {help_txt}"
    );

    let (app_reply, _) = send_dm_and_wait_bot_reply(&router, &bob_tok, bob_bot_cid, "/app").await;
    assert!(
        msg_has_webapp_button(&app_reply, &nl_app.short_name),
        "/app reply missing Mini App WebApp button: {app_reply}"
    );
}

#[tokio::test]
async fn test_e2e_discover_install_auto_companion_runtime_hmac_storage_actions_and_cascades() {
    let dir = TempDirGuard::new("bf_e2e_full_runtime");
    let cfg = build_temp_server_config(dir.path());
    let (state, router) = boot_state_and_router(cfg.clone());

    let (_alice_id, alice_tok) = register_user(&router, "alice_owner").await;
    let (bob_id, bob_tok) = register_user(&router, "bob_player").await;

    // 1. Alice creates a Coffee Order Bot + Mini App via REST + a secondary bot for /deletebot test
    let (nl_st, nl_val) = http_json(
        &router,
        "POST",
        "/api/v1/botfather/nl-create",
        Some(&alice_tok),
        Some(json!({ "prompt": "创建一个团队咖啡点单机器人和咖啡点单小程序，支持美式拿铁和总金额统计" })),
    )
    .await;
    assert_eq!(nl_st, StatusCode::CREATED, "nl-create coffee: {nl_val}");
    let created: NaturalLanguageCreateResponse = serde_json::from_value(nl_val).unwrap();
    let bot_un = created.bot.username.clone();
    let short = created.app.as_ref().expect("app").short_name.clone();
    let bot_token = created.bot_token.clone();

    let (tmp_bot_st, tmp_bot_val) = http_json(
        &router,
        "POST",
        "/api/v1/bots",
        Some(&alice_tok),
        Some(json!({
            "username": "TempDeleteTestBot",
            "display_name": "Temp Delete Bot",
            "description": "Disposable bot for cascade test"
        })),
    )
    .await;
    assert_eq!(tmp_bot_st, StatusCode::CREATED, "create disposable bot: {tmp_bot_val}");

    // 2. Bob searches "apps" -> built-in flagship Mini Apps are not seeded; Alice's newly created Mini App is present
    let (apps_s_st, apps_s_val) = http_json(
        &router,
        "GET",
        "/api/v1/search?q=&category=apps",
        Some(&bob_tok),
        None,
    )
    .await;
    assert_eq!(apps_s_st, StatusCode::OK);
    let apps_catalog: Vec<MiniAppEntry> =
        serde_json::from_value(apps_s_val["apps"].clone()).unwrap();
    for flagship_short in ["focus", "split", "poll"] {
        assert!(
            apps_catalog.iter().all(|a| a.short_name != flagship_short),
            "unexpected built-in flagship Mini App {flagship_short} in apps search"
        );
    }
    assert!(
        apps_catalog
            .iter()
            .any(|a| a.bot_username == bot_un && a.short_name == short),
        "Alice's published Mini App {bot_un}/{short} missing from Bob's apps catalog"
    );

    // Verify Bob initially has 0 installed Mini Apps
    let (inst0_st, inst0_val) = http_json(
        &router,
        "GET",
        "/api/v1/miniapps/installed",
        Some(&bob_tok),
        None,
    )
    .await;
    assert_eq!(inst0_st, StatusCode::OK);
    assert!(inst0_val["apps"].as_array().is_some_and(|a| a.is_empty()));

    // 3. Bob installs Mini App -> verify automatic companion bot DM + welcome + contacts + attach menu
    let (inst_st, inst_val) = http_json(
        &router,
        "POST",
        &format!("/api/v1/miniapps/{bot_un}/{short}/install"),
        Some(&bob_tok),
        Some(json!({ "allow_write_access": true })),
    )
    .await;
    assert_eq!(inst_st, StatusCode::OK, "install Mini App: {inst_val}");
    let inst_res: PeerBotActionResult = serde_json::from_value(inst_val).unwrap();
    assert!(inst_res.ok);
    let companion_cid = inst_res.companion_chat_id.expect("companion_chat_id");
    assert!(companion_cid >= 600_000_000);
    assert!(inst_res.app.as_ref().is_some_and(|a| a.is_installed));

    // Verify companion bot chat appears in Bob's chats list
    let (chats_st, chats_val) =
        http_json(&router, "GET", "/api/v1/chats", Some(&bob_tok), None).await;
    assert_eq!(chats_st, StatusCode::OK);
    let has_companion_chat = chats_val["chats"].as_array().is_some_and(|arr| {
        arr.iter()
            .any(|c| c["chat_id"].as_i64() == Some(companion_cid) || c["id"].as_i64() == Some(companion_cid))
    });
    assert!(has_companion_chat, "companion_chat_id {companion_cid} missing from /api/v1/chats");

    // Verify companion bot welcome message + inline WebApp button
    let companion_msgs = list_chat_messages(&router, &bob_tok, companion_cid).await;
    assert!(!companion_msgs.is_empty(), "missing companion welcome message");
    assert!(
        msg_has_webapp_button(&companion_msgs[0], &format!("gpui-miniapp://{bot_un}/{short}")),
        "companion welcome missing WebApp button: {:?}",
        companion_msgs[0]
    );

    // Verify companion bot appears in contacts and attach-menu
    let (_, contacts_val) =
        http_json(&router, "GET", "/api/v1/contacts", Some(&bob_tok), None).await;
    assert!(
        contacts_val["contacts"]
            .as_array()
            .is_some_and(|c| c.iter().any(|u| u["username"].as_str() == Some(bot_un.as_str()))),
        "companion bot missing from Bob's contacts: {contacts_val}"
    );
    let (_, attach_val) =
        http_json(&router, "GET", "/api/v1/bots/attach-menu", Some(&bob_tok), None).await;
    assert!(
        attach_val["bots"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x["username"].as_str() == Some(bot_un.as_str()))),
        "installed Mini App bot missing from attach-menu: {attach_val}"
    );

    // 4. Launch Mini App -> Verify cryptographic Telegram HMAC-SHA256 initData + reject tampered
    let (launch_st, launch_val) = http_json(
        &router,
        "POST",
        &format!("/api/v1/miniapps/{bot_un}/{short}/launch"),
        Some(&bob_tok),
        Some(json!({
            "chat_id": companion_cid,
            "launch_mode": "CompactSheet",
            "start_param": "promo_e2e"
        })),
    )
    .await;
    assert_eq!(launch_st, StatusCode::OK, "launch Mini App: {launch_val}");
    let launch_ctx: MiniAppLaunchContext = serde_json::from_value(launch_val).unwrap();
    assert!(launch_ctx.init_data.hmac_verified);
    assert_eq!(launch_ctx.init_data.start_param.as_deref(), Some("promo_e2e"));
    assert!(
        verify_telegram_hmac_independent(&launch_ctx.init_data.raw_query_string, &bot_token),
        "independent RFC-2104 HMAC-SHA256 verification failed for raw_query_string={}",
        launch_ctx.init_data.raw_query_string
    );

    let (val_ok_st, val_ok_json) = http_json(
        &router,
        "POST",
        &format!("/api/v1/miniapps/{bot_un}/{short}/validate-init-data"),
        Some(&bob_tok),
        Some(serde_json::to_value(ValidateInitDataRequest {
            init_data: launch_ctx.init_data.raw_query_string.clone(),
        }).unwrap()),
    )
    .await;
    assert_eq!(val_ok_st, StatusCode::OK);
    let val_ok: ValidateInitDataResponse = serde_json::from_value(val_ok_json).unwrap();
    assert!(val_ok.valid);
    assert_eq!(val_ok.user_id, Some(bob_id as i64));

    let tampered_init = format!("{}&tamper=1", launch_ctx.init_data.raw_query_string);
    let (val_bad_st, val_bad_json) = http_json(
        &router,
        "POST",
        &format!("/api/v1/miniapps/{bot_un}/{short}/validate-init-data"),
        Some(&bob_tok),
        Some(json!({ "init_data": tampered_init })),
    )
    .await;
    assert_eq!(val_bad_st, StatusCode::OK);
    let val_bad: ValidateInitDataResponse = serde_json::from_value(val_bad_json).unwrap();
    assert!(!val_bad.valid, "tampered initData MUST be rejected");

    // 5. CloudStorage: PUT / GET / DELETE / Validation quotas / Persistence across DB reopen
    let (put1_st, _) = http_json(
        &router,
        "PUT",
        &format!("/api/v1/miniapps/{bot_un}/{short}/storage"),
        Some(&bob_tok),
        Some(json!({ "key": "favorite_drink", "value": "Iced Americano Large" })),
    )
    .await;
    assert_eq!(put1_st, StatusCode::OK);

    let (put_batch_st, _) = http_json(
        &router,
        "PUT",
        &format!("/api/v1/miniapps/{bot_un}/{short}/storage"),
        Some(&bob_tok),
        Some(json!({ "entries": { "theme": "dark", "temp_key": "remove_me" } })),
    )
    .await;
    assert_eq!(put_batch_st, StatusCode::OK);

    // Key/value limit validation
    let oversized_key = "k".repeat(CLOUD_STORAGE_MAX_KEY_BYTES + 1);
    let (bad_key_st, _) = http_json(
        &router,
        "PUT",
        &format!("/api/v1/miniapps/{bot_un}/{short}/storage"),
        Some(&bob_tok),
        Some(json!({ "key": oversized_key, "value": "v" })),
    )
    .await;
    assert_eq!(bad_key_st, StatusCode::BAD_REQUEST, "oversized CloudStorage key must be rejected");

    let oversized_val = "v".repeat(CLOUD_STORAGE_MAX_VALUE_BYTES + 1);
    let (bad_val_st, _) = http_json(
        &router,
        "PUT",
        &format!("/api/v1/miniapps/{bot_un}/{short}/storage"),
        Some(&bob_tok),
        Some(json!({ "key": "valid_k", "value": oversized_val })),
    )
    .await;
    assert_eq!(bad_val_st, StatusCode::BAD_REQUEST, "oversized CloudStorage value must be rejected");

    // Delete single key and confirm remaining keys
    let (del_st, _) = http_json(
        &router,
        "DELETE",
        &format!("/api/v1/miniapps/{bot_un}/{short}/storage?key=temp_key"),
        Some(&bob_tok),
        None,
    )
    .await;
    assert_eq!(del_st, StatusCode::OK);

    let (get_cs_st, get_cs_val) = http_json(
        &router,
        "GET",
        &format!("/api/v1/miniapps/{bot_un}/{short}/storage"),
        Some(&bob_tok),
        None,
    )
    .await;
    assert_eq!(get_cs_st, StatusCode::OK);
    assert_eq!(get_cs_val["entries"]["favorite_drink"].as_str(), Some("Iced Americano Large"));
    assert_eq!(get_cs_val["entries"]["theme"].as_str(), Some("dark"));
    assert!(get_cs_val["entries"].get("temp_key").is_none());

    // 6. Widget Event + MainButton + sendData + Companion Bot intelligence + AI Co-Pilot
    let widget_req = MiniAppActionRequest::widget_event(
        "coffee_item".to_string(),
        json!("拿铁 ¥28"),
        Some(companion_cid),
    );
    let (w_st, w_val) = http_json(
        &router,
        "POST",
        &format!("/api/v1/miniapps/{bot_un}/{short}/action"),
        Some(&bob_tok),
        Some(serde_json::to_value(widget_req).unwrap()),
    )
    .await;
    assert_eq!(w_st, StatusCode::OK);
    let w_resp: MiniAppActionResponse = serde_json::from_value(w_val).unwrap();
    assert!(w_resp.ok);

    let main_req = MiniAppActionRequest::main_button_click(Some(companion_cid));
    let (mb_st, mb_val) = http_json(
        &router,
        "POST",
        &format!("/api/v1/miniapps/{bot_un}/{short}/action"),
        Some(&bob_tok),
        Some(serde_json::to_value(main_req).unwrap()),
    )
    .await;
    assert_eq!(mb_st, StatusCode::OK);
    let mb_resp: MiniAppActionResponse = serde_json::from_value(mb_val).unwrap();
    assert!(mb_resp.ok);
    assert!(
        mb_resp.state_json["orders"].as_array().is_some_and(|o| !o.is_empty()),
        "MainButton failed to record order in state_json: {}",
        mb_resp.state_json
    );

    // AI Co-Pilot in-app query
    let copilot_req =
        MiniAppActionRequest::ai_copilot("帮我总结当前已点的咖啡数量和总价", Some(companion_cid));
    let (cp_st, cp_val) = http_json(
        &router,
        "POST",
        &format!("/api/v1/miniapps/{bot_un}/{short}/action"),
        Some(&bob_tok),
        Some(serde_json::to_value(copilot_req).unwrap()),
    )
    .await;
    assert_eq!(cp_st, StatusCode::OK);
    let cp_resp: MiniAppActionResponse = serde_json::from_value(cp_val).unwrap();
    assert!(cp_resp.ok);
    assert!(
        cp_resp.ai_reply.as_deref().is_some_and(|r| !r.trim().is_empty()),
        "AI Co-Pilot returned empty reply"
    );

    // MainButton sendData -> persists state, posts to companion chat, sets close_app=true
    let pre_count = list_chat_messages(&router, &bob_tok, companion_cid).await.len();
    let sd_req = MiniAppActionRequest::send_data(
        "已下单 拿铁 ¥28 x1（小程序同步）".to_string(),
        Some(companion_cid),
    );
    let (sd_st, sd_val) = http_json(
        &router,
        "POST",
        &format!("/api/v1/miniapps/{bot_un}/{short}/action"),
        Some(&bob_tok),
        Some(serde_json::to_value(sd_req).unwrap()),
    )
    .await;
    assert_eq!(sd_st, StatusCode::OK);
    let sd_resp: MiniAppActionResponse = serde_json::from_value(sd_val).unwrap();
    assert!(sd_resp.ok && sd_resp.close_app);
    let post_msgs = list_chat_messages(&router, &bob_tok, companion_cid).await;
    assert!(
        post_msgs.len() > pre_count
            && post_msgs.last().unwrap()["text"]
                .as_str()
                .unwrap_or("")
                .contains("拿铁"),
        "companion chat did not receive sendData message: {post_msgs:?}"
    );

    // Companion bot follow-up replies with awareness of latest state
    let (follow_up, _) = send_dm_and_wait_bot_reply(
        &router,
        &bob_tok,
        companion_cid,
        "我刚才下单总计花了多少钱？",
    )
    .await;
    assert!(!follow_up["text"].as_str().unwrap_or("").trim().is_empty());

    // 7. Reopen SQLite DB -> verify CloudStorage & MiniApp session state persist across relaunch!
    drop(router);
    drop(state);
    let (_state2, router2) = boot_state_and_router(cfg);
    let (_, relaunch_val) = http_json(
        &router2,
        "POST",
        &format!("/api/v1/miniapps/{bot_un}/{short}/launch"),
        Some(&bob_tok),
        Some(json!({ "launch_mode": "DirectLink" })),
    )
    .await;
    let relaunch_ctx: MiniAppLaunchContext = serde_json::from_value(relaunch_val).unwrap();
    assert!(
        relaunch_ctx
            .cloud_storage
            .iter()
            .any(|i| i.key == "favorite_drink" && i.value == "Iced Americano Large"),
        "CloudStorage failed persistence across DB reopen + relaunch: {:?}",
        relaunch_ctx.cloud_storage
    );
    assert!(
        relaunch_ctx.state_json["orders"].as_array().is_some_and(|o| !o.is_empty()),
        "Mini App session state lost across reopen"
    );

    // 8. Uninstall cascade for Bob + /deleteapp & /deletebot cascades for Alice
    let (un_st, _) = http_json(
        &router2,
        "DELETE",
        &format!("/api/v1/miniapps/{bot_un}/{short}/install"),
        Some(&bob_tok),
        None,
    )
    .await;
    assert_eq!(un_st, StatusCode::OK);
    let (_, inst_after) =
        http_json(&router2, "GET", "/api/v1/miniapps/installed", Some(&bob_tok), None).await;
    assert!(
        inst_after["apps"]
            .as_array()
            .is_some_and(|arr| !arr.iter().any(|a| a["short_name"].as_str() == Some(short.as_str()))),
        "uninstalled Mini App still returned in /installed"
    );

    // Alice deletes the Mini App via @BotFather /deleteapp
    let (_, add_bf_a) = http_json(
        &router2,
        "POST",
        "/api/v1/contacts/add",
        Some(&alice_tok),
        Some(json!({ "username": "BotFather", "is_bot": true })),
    )
    .await;
    let bf_cid_a = add_bf_a["chat_id"].as_i64().unwrap();
    let (del_prompt, _) = send_dm_and_wait_bot_reply(&router2, &alice_tok, bf_cid_a, "/deleteapp").await;
    assert!(msg_has_button_with_callback(
        &del_prompt,
        &format!("bf:deleteapp:{bot_un}:{short}")
    ));
    let (del_app_done, _) = send_dm_and_wait_bot_reply(
        &router2,
        &alice_tok,
        bf_cid_a,
        &format!("{bot_un}/{short}"),
    )
    .await;
    assert!(
        del_app_done["text"].as_str().unwrap_or("").contains("已删除")
            || del_app_done["text"].as_str().unwrap_or("").to_lowercase().contains("deleted"),
        "expected /deleteapp confirmation: {del_app_done}"
    );
    let (launch_deleted_st, _) = http_json(
        &router2,
        "POST",
        &format!("/api/v1/miniapps/{bot_un}/{short}/launch"),
        Some(&bob_tok),
        Some(json!({ "launch_mode": "DirectLink" })),
    )
    .await;
    assert_eq!(launch_deleted_st, StatusCode::NOT_FOUND);

    // Alice deletes TempDeleteTestBot via /deletebot -> removed from bots_my & search
    let (del_bot_prompt, _) =
        send_dm_and_wait_bot_reply(&router2, &alice_tok, bf_cid_a, "/deletebot").await;
    assert!(msg_has_button_with_callback(&del_bot_prompt, "bf:pick:deletebot:TempDeleteTestBot"));
    let (del_bot_done, _) =
        send_dm_and_wait_bot_reply(&router2, &alice_tok, bf_cid_a, "@TempDeleteTestBot").await;
    assert!(
        del_bot_done["text"].as_str().unwrap_or("").contains("已删除")
            || del_bot_done["text"].as_str().unwrap_or("").to_lowercase().contains("deleted"),
        "expected /deletebot confirmation: {del_bot_done}"
    );
    let (_, bots_after) =
        http_json(&router2, "GET", "/api/v1/bots/my", Some(&alice_tok), None).await;
    assert!(
        !bots_after["bots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["username"].as_str() == Some("TempDeleteTestBot"))
    );
}
