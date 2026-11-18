use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    Router,
};
use gpui_telegram_server::{
    build_router_with_state, config::ServerConfig, server::AppState,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

fn build_temp_server_config(dir: &std::path::Path) -> ServerConfig {
    let db_path = dir.join("fabushi_miniapp_e2e.db");
    let media_dir = dir.join("media_cas");
    std::fs::create_dir_all(&media_dir).expect("create media dir");
    let mut cfg = ServerConfig::default();
    cfg.database_url = format!("sqlite://{}", db_path.display());
    cfg.media_storage_path = media_dir.to_string_lossy().to_string();
    cfg.host = "127.0.0.1".to_string();
    cfg.port = 0;
    cfg
}

fn boot_test_server() -> (TempDir, Arc<AppState>, Router) {
    let dir = tempfile::Builder::new()
        .prefix("fabushi_e2e_")
        .tempdir_in(std::env::temp_dir())
        .expect("create tempdir");
    let cfg = build_temp_server_config(dir.path());
    let state = Arc::new(AppState::try_new(cfg).expect("boot AppState"));
    let router = build_router_with_state(state.clone());
    (dir, state, router)
}

async fn http_raw(
    router: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, String, Vec<u8>) {
    let mut builder = Request::builder()
        .method(Method::from_bytes(method.as_bytes()).expect("valid method"))
        .uri(uri);
    for (k, v) in headers {
        builder = builder.header(*k, *v);
    }
    let req_body = match body {
        Some(val) => {
            builder = builder.header("content-type", "application/json");
            Body::from(val.to_string())
        }
        None => Body::empty(),
    };
    let req = builder.body(req_body).expect("build request");
    let resp = router.clone().oneshot(req).await.expect("oneshot request");
    let status = resp.status();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read body")
        .to_vec();
    (status, content_type, bytes)
}

async fn http_json(
    router: &Router,
    method: &str,
    uri: &str,
    bearer: Option<&str>,
    init_data: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut hdrs: Vec<(&str, String)> = Vec::new();
    if let Some(tok) = bearer {
        hdrs.push(("authorization", format!("Bearer {tok}")));
    }
    if let Some(id) = init_data {
        hdrs.push(("x-telegram-init-data", id.to_string()));
    }
    let hdr_refs: Vec<(&str, &str)> = hdrs.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (status, _ct, bytes) = http_raw(router, method, uri, &hdr_refs, body).await;
    let val = serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|_| {
        json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() })
    });
    (status, val)
}

async fn register_user(router: &Router, username: &str) -> (u64, String) {
    let (st, val) = http_json(
        router,
        "POST",
        "/api/v1/auth/register",
        None,
        None,
        Some(json!({
            "username": username,
            "password": "StrongPassword#2026",
            "first_name": username,
            "device_name": "fabushi_e2e_test",
            "auto_welcome": true
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "register {username}: {val}");
    let uid = val["user_id"].as_u64().expect("user_id");
    let tok = val["token"].as_str().expect("token").to_string();
    (uid, tok)
}

async fn publish_official_fabushi(router: &Router) -> (String, i64, String) {
    let (_pub_uid, pub_tok) = register_user(router, "fabushi_official_publisher").await;

    let (st_bot, bot_val) = http_json(
        router,
        "POST",
        "/api/v1/bots",
        Some(&pub_tok),
        None,
        Some(json!({
            "display_name": "全球法布施",
            "username": "fabushi_bot",
            "description": "全球法布施与净心禅室",
            "about": "全球法布施官方伴生机器人",
            "allow_attach_menu": true,
            "attach_peer_types": ["users", "bots", "groups"],
            "menu_button_text": "全球法布施",
            "menu_button_url": "/miniapps/fabushi_bot/fabushi"
        })),
    )
    .await;
    assert_eq!(st_bot, StatusCode::CREATED, "create @fabushi_bot: {bot_val}");
    let bot_token = bot_val["bot_token"].as_str().expect("bot_token").to_string();
    let bot_user_id = bot_val["bot"]["bot_user_id"].as_i64().expect("bot_user_id");

    let (st_pub, pub_val) = http_json(
        router,
        "POST",
        &format!("/bot{bot_token}/publishMiniApp"),
        None,
        None,
        Some(json!({
            "short_name": "fabushi",
            "title": "全球法布施",
            "description": "全球法布施与净心禅室 — 纯Rust WASM全球弘法平台",
            "category": "lifestyle",
            "icon_emoji": "🪷",
            "web_url": "/miniapps/fabushi_bot/fabushi",
            "is_main_app": true,
            "header_color": "#0B0E14",
            "background_color": "#0B0E14",
            "ui_schema": {
                "title": "全球法布施",
                "subtitle": "@fabushi_bot · 全球法布施与净心禅室",
                "widgets": [
                    { "id": "mode", "kind": "Select", "label": "模式", "value": "全球法布施", "options": ["全球法布施", "禅室共修"] },
                    { "id": "sutra", "kind": "Select", "label": "经典", "value": "心经", "options": ["心经", "金刚经"] },
                    { "id": "target_country", "kind": "TextInput", "label": "节点", "value": "ALL" },
                    { "id": "chant_count", "kind": "Counter", "label": "遍数", "count": 108, "min": 1, "max": 10000 }
                ]
            },
            "html_bundle": fabushi_miniapp::hosted_bundle_html()
        })),
    )
    .await;
    assert_eq!(st_pub, StatusCode::OK, "publishMiniApp: {pub_val}");
    assert_eq!(pub_val["ok"].as_bool(), Some(true));

    let (st_mb, mb_val) = http_json(
        router,
        "POST",
        &format!("/bot{bot_token}/setChatMenuButton"),
        None,
        None,
        Some(json!({
            "menu_button": {
                "type": "web_app",
                "text": "全球法布施",
                "web_app": { "url": "/miniapps/fabushi_bot/fabushi" }
            }
        })),
    )
    .await;
    assert_eq!(st_mb, StatusCode::OK, "setChatMenuButton: {mb_val}");
    assert_eq!(mb_val["ok"].as_bool(), Some(true));

    (pub_tok, bot_user_id, bot_token)
}

#[tokio::test]
async fn test_e2e_clean_boot_has_zero_seeded_miniapps_and_tables_initialized() {
    let (_dir, _state, router) = boot_test_server();
    let (_uid, tok) = register_user(&router, "clean_boot_checker").await;

    let (st_apps, apps_val) =
        http_json(&router, "GET", "/api/v1/miniapps", Some(&tok), None, None).await;
    assert_eq!(st_apps, StatusCode::OK, "GET /api/v1/miniapps: {apps_val}");
    assert_eq!(
        apps_val["total"].as_u64(),
        Some(0),
        "Fresh server boot must have 0 seeded Mini Apps to preserve parity invariant: {apps_val}"
    );

    // Bearer fallback on /bootstrap initializes user profile + exposes 8 seeded practitioners
    let (st_boot, boot_val) = http_json(
        &router,
        "GET",
        "/api/v1/fabushi-miniapp/bootstrap",
        Some(&tok),
        None,
        None,
    )
    .await;
    assert_eq!(st_boot, StatusCode::OK, "bootstrap with Bearer: {boot_val}");
    assert_eq!(boot_val["type"].as_str(), Some("bootstrap"));
    assert!(boot_val["leaderboard"].as_array().is_some_and(|a| a.len() >= 8));
}

#[tokio::test]
async fn test_e2e_official_publish_flow_search_html_wasm_assets_and_attach_menu() {
    let (_dir, state, router) = boot_test_server();
    let (pub_tok, bot_user_id, _bot_tok) = publish_official_fabushi(&router).await;

    // Search via /api/v1/miniapps?q=全球法布施
    let (st_m, val_m) = http_json(
        &router,
        "GET",
        "/api/v1/miniapps?q=%E5%85%A8%E7%90%83%E6%B3%95%E5%B8%83%E6%96%BD",
        Some(&pub_tok),
        None,
        None,
    )
    .await;
    assert_eq!(st_m, StatusCode::OK, "/api/v1/miniapps: {val_m}");
    let app_entry = val_m["apps"]
        .as_array()
        .and_then(|arr| arr.iter().find(|a| a["short_name"].as_str() == Some("fabushi")))
        .expect("fabushi present in /api/v1/miniapps?q=全球法布施");
    assert_eq!(app_entry["title"].as_str(), Some("全球法布施"));
    assert_eq!(app_entry["is_main_app"].as_bool(), Some(true));
    assert_eq!(app_entry["header_color"].as_str(), Some("#0B0E14"));
    assert_eq!(app_entry["background_color"].as_str(), Some("#0B0E14"));
    let db_app = state
        .db
        .get_mini_app("fabushi_bot", "fabushi")
        .unwrap()
        .expect("db miniapp record");
    assert!(
        db_app.is_attach_enabled,
        "handle_publish_mini_app must inherit bot.allow_attach_menu"
    );

    // Search via /api/v1/search?q=fabushi&category=apps
    let (st_s, val_s) = http_json(
        &router,
        "GET",
        "/api/v1/search?q=fabushi&category=apps",
        Some(&pub_tok),
        None,
        None,
    )
    .await;
    assert_eq!(st_s, StatusCode::OK, "/api/v1/search: {val_s}");
    assert!(val_s["apps"]
        .as_array()
        .is_some_and(|arr| arr.iter().any(|a| a["short_name"].as_str() == Some("fabushi"))));
    assert!(val_s["bots"]
        .as_array()
        .is_some_and(|arr| arr.iter().any(|b| b["username"].as_str() == Some("fabushi_bot"))));

    // Hosted HTML bundle endpoint
    let (st_html, ct_html, body_html) = http_raw(
        &router,
        "GET",
        "/miniapps/fabushi_bot/fabushi",
        &[],
        None,
    )
    .await;
    assert_eq!(st_html, StatusCode::OK);
    assert!(ct_html.contains("text/html"));
    let html_str = String::from_utf8_lossy(&body_html);
    assert!(html_str.contains("/js/telegram-web-app.js"));
    assert!(html_str.contains("fabushi_wasm_init"));

    // Real WASM binary endpoint
    let (st_wasm, ct_wasm, body_wasm) = http_raw(
        &router,
        "GET",
        "/api/v1/fabushi-miniapp/app.wasm",
        &[],
        None,
    )
    .await;
    assert_eq!(st_wasm, StatusCode::OK);
    assert_eq!(ct_wasm, "application/wasm");
    assert!(
        body_wasm.starts_with(b"\0asm\x01\0\0\0") && body_wasm.len() > 100_000,
        "app.wasm must start with WebAssembly magic header and embed real UI binary, got {} bytes",
        body_wasm.len()
    );

    // Assets endpoint
    for (path, expected_mime) in [
        ("/api/v1/fabushi-miniapp/assets/earth_texture.jpg", "image/jpeg"),
        (
            "/api/v1/fabushi-miniapp/assets/home_world_lightfield.webp",
            "image/webp",
        ),
        (
            "/api/v1/fabushi-miniapp/assets/zen_room_backdrop_2d.webp",
            "image/webp",
        ),
    ] {
        let (st_a, ct_a, bytes_a) = http_raw(&router, "GET", path, &[], None).await;
        assert_eq!(st_a, StatusCode::OK, "asset {path}");
        assert_eq!(ct_a, expected_mime);
        assert!(!bytes_a.is_empty());
    }

    // Install as normal user and confirm attach-menu listing
    let (_u_id, u_tok) = register_user(&router, "attach_menu_user").await;
    let (st_inst, inst_val) = http_json(
        &router,
        "POST",
        "/api/v1/miniapps/fabushi_bot/fabushi/install",
        Some(&u_tok),
        None,
        Some(json!({ "allow_write_access": false })),
    )
    .await;
    assert_eq!(st_inst, StatusCode::OK, "install fabushi: {inst_val}");

    let (st_am, am_val) = http_json(
        &router,
        "GET",
        "/api/v1/bots/attach-menu",
        Some(&u_tok),
        None,
        None,
    )
    .await;
    assert_eq!(st_am, StatusCode::OK, "attach-menu: {am_val}");
    assert!(am_val["bots"]
        .as_array()
        .is_some_and(|arr| arr.iter().any(|b| b["bot_user_id"].as_i64() == Some(bot_user_id))));
}

#[tokio::test]
async fn test_e2e_init_data_hmac_auth_full_dharma_flow_cloud_storage_and_companion_dm() {
    let (_dir, _state, router) = boot_test_server();
    let (_pub_tok, bot_user_id, _bot_tok) = publish_official_fabushi(&router).await;
    let (_uid, tok) = register_user(&router, "practitioner_1").await;

    // Install with allow_write_access = false initially
    let (st_inst, inst_val) = http_json(
        &router,
        "POST",
        "/api/v1/miniapps/fabushi_bot/fabushi/install",
        Some(&tok),
        None,
        Some(json!({ "allow_write_access": false })),
    )
    .await;
    assert_eq!(st_inst, StatusCode::OK);
    let companion_cid = inst_val["companion_chat_id"].as_i64().expect("companion_chat_id");

    // Launch to receive HMAC-signed initData
    let (st_launch, launch_val) = http_json(
        &router,
        "POST",
        "/api/v1/miniapps/fabushi_bot/fabushi/launch",
        Some(&tok),
        None,
        Some(json!({ "launch_mode": "FullSheet" })),
    )
    .await;
    assert_eq!(st_launch, StatusCode::OK, "launch: {launch_val}");
    let init_data = launch_val["init_data"]["raw_query_string"]
        .as_str()
        .expect("signed initData raw_query_string")
        .to_string();

    // Tampered X-Telegram-Init-Data must return 401 even if Bearer is also passed
    let tampered = format!("{init_data}tampered");
    let (st_bad, _) = http_json(
        &router,
        "GET",
        "/api/v1/fabushi-miniapp/bootstrap",
        Some(&tok),
        Some(&tampered),
        None,
    )
    .await;
    assert_eq!(
        st_bad,
        StatusCode::UNAUTHORIZED,
        "tampered initData must be rejected with 401"
    );

    // Valid X-Telegram-Init-Data without Bearer authenticates bootstrap
    let (st_boot, boot_val) = http_json(
        &router,
        "GET",
        "/api/v1/fabushi-miniapp/bootstrap",
        None,
        Some(&init_data),
        None,
    )
    .await;
    assert_eq!(st_boot, StatusCode::OK, "initData bootstrap: {boot_val}");
    assert_eq!(boot_val["profile"]["username"].as_str(), Some("practitioner_1"));

    // Record baseline companion chat message count before transfer
    let (st_m0, m0_val) = http_json(
        &router,
        "GET",
        &format!("/api/v1/chats/{companion_cid}/messages?limit=50"),
        Some(&tok),
        None,
        None,
    )
    .await;
    assert_eq!(st_m0, StatusCode::OK);
    let msg_count_before = m0_val["messages"].as_array().map(|a| a.len()).unwrap_or(0);

    // Transfer BEFORE allows_write_to_pm: updates SQLite + CloudStorage, NO companion DM yet
    let (st_tr1, tr1_val) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-miniapp/transfer",
        None,
        Some(&init_data),
        Some(json!({
            "content_title": "心经",
            "content_kind": "sutra",
            "target_countries": ["CN", "US"],
            "bytes_sent": 4096,
            "beam_count": 2
        })),
    )
    .await;
    assert_eq!(st_tr1, StatusCode::OK, "transfer 1: {tr1_val}");
    assert_eq!(tr1_val["total_transfer_bytes"].as_u64(), Some(4096));
    assert_eq!(tr1_val["total_transfer_count"].as_u64(), Some(1));

    let (st_cs1, cs1_val) = http_json(
        &router,
        "GET",
        "/api/v1/miniapps/fabushi_bot/fabushi/storage",
        Some(&tok),
        None,
        None,
    )
    .await;
    assert_eq!(st_cs1, StatusCode::OK);
    let summary_1 = cs1_val["items"]
        .as_array()
        .and_then(|arr| arr.iter().find(|i| i["key"].as_str() == Some("fabushi_summary")))
        .and_then(|i| i["value"].as_str())
        .expect("fabushi_summary synced in CloudStorage");
    assert!(summary_1.contains("\"totalTransferBytes\":4096"));

    let (_, m1_val) = http_json(
        &router,
        "GET",
        &format!("/api/v1/chats/{companion_cid}/messages?limit=50"),
        Some(&tok),
        None,
        None,
    )
    .await;
    assert_eq!(
        m1_val["messages"].as_array().map(|a| a.len()).unwrap_or(0),
        msg_count_before,
        "must NOT DM user before allows_write_to_pm is granted"
    );

    // Grant allows_write_to_pm
    let (st_allow, _) = http_json(
        &router,
        "POST",
        &format!("/api/v1/bots/{bot_user_id}/allow-send-message"),
        Some(&tok),
        None,
        Some(json!({ "allow": true })),
    )
    .await;
    assert_eq!(st_allow, StatusCode::OK);

    // Transfer + Practice Session AFTER granting allows_write_to_pm -> both send DM receipts!
    let (st_tr2, _) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-miniapp/transfer",
        None,
        Some(&init_data),
        Some(json!({
            "content_title": "金刚经",
            "content_kind": "sutra",
            "target_countries": ["ALL"],
            "bytes_sent": 8192,
            "beam_count": 12
        })),
    )
    .await;
    assert_eq!(st_tr2, StatusCode::OK);

    let (st_ps, ps_val) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-miniapp/practice/session",
        None,
        Some(&init_data),
        Some(json!({
            "sutra": "金刚经",
            "duration_minutes": 15,
            "chant_count": 108,
            "notes": "回向十方法界众生"
        })),
    )
    .await;
    assert_eq!(st_ps, StatusCode::OK, "practice/session: {ps_val}");
    assert_eq!(ps_val["profile"]["total_practice_sessions"].as_u64(), Some(1));
    assert_eq!(ps_val["profile"]["total_practice_chants"].as_u64(), Some(108));

    let (_, m2_val) = http_json(
        &router,
        "GET",
        &format!("/api/v1/chats/{companion_cid}/messages?limit=50"),
        Some(&tok),
        None,
        None,
    )
    .await;
    let msgs2 = m2_val["messages"].as_array().expect("messages array");
    assert!(
        msgs2.len() >= msg_count_before + 2,
        "expected 2 DM receipts in companion chat, got {}: {m2_val}",
        msgs2.len()
    );
    let combined_text: String = msgs2
        .iter()
        .filter_map(|m| m["text"].as_str().or_else(|| m["content"]["text"].as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(combined_text.contains("全球法布施功德回执"));
    assert!(combined_text.contains("禅室共修功德回执"));

    // Manual practice record CRUD
    let (st_rec_add, rec_add_val) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-miniapp/practice/record",
        None,
        Some(&init_data),
        Some(json!({
            "sutra_name": "大悲咒",
            "chant_count": 21,
            "duration_minutes": 10,
            "record_date": "2026-11-16",
            "local_time": "08:30",
            "notes": "晨课"
        })),
    )
    .await;
    assert_eq!(st_rec_add, StatusCode::OK);
    let created_id = rec_add_val["practice_records"]
        .as_array()
        .and_then(|arr| arr.iter().find(|r| r["sutra_name"].as_str() == Some("大悲咒")))
        .and_then(|r| r["id"].as_i64())
        .expect("created manual record id");

    let (st_rec_put, rec_put_val) = http_json(
        &router,
        "PUT",
        &format!("/api/v1/fabushi-miniapp/practice/record/{created_id}"),
        None,
        Some(&init_data),
        Some(json!({
            "sutra_name": "大悲咒",
            "chant_count": 49,
            "duration_minutes": 20,
            "record_date": "2026-11-16",
            "local_time": "08:30",
            "notes": "加诵"
        })),
    )
    .await;
    assert_eq!(st_rec_put, StatusCode::OK, "PUT record: {rec_put_val}");
    assert!(rec_put_val["practice_records"]
        .as_array()
        .is_some_and(|arr| arr.iter().any(|r| r["id"].as_i64() == Some(created_id) && r["chant_count"].as_u64() == Some(49))));

    let (st_rec_del, rec_del_val) = http_json(
        &router,
        "DELETE",
        &format!("/api/v1/fabushi-miniapp/practice/record/{created_id}"),
        None,
        Some(&init_data),
        None,
    )
    .await;
    assert_eq!(st_rec_del, StatusCode::OK, "DELETE record: {rec_del_val}");
    assert!(!rec_del_val["practice_records"]
        .as_array()
        .is_some_and(|arr| arr.iter().any(|r| r["id"].as_i64() == Some(created_id))));

    // Profile update & 一门深入 lock immutability
    let (st_prof1, prof1_val) = http_json(
        &router,
        "PUT",
        "/api/v1/fabushi-miniapp/profile",
        None,
        Some(&init_data),
        Some(json!({
            "display_name": "净行居士",
            "bio": "每日念佛诵经",
            "locked_practice": {
                "title": "金刚经",
                "category": "禅室诵读",
                "file_path": "sutras/jingangjing",
                "selected_at_ms": 1700000000000_i64,
                "is_locked": true
            }
        })),
    )
    .await;
    assert_eq!(st_prof1, StatusCode::OK);
    assert_eq!(prof1_val["profile"]["display_name"].as_str(), Some("净行居士"));
    assert_eq!(prof1_val["locked_practice"]["title"].as_str(), Some("金刚经"));

    // Second lock attempt must preserve original locked_practice (一门深入)
    let (st_prof2, prof2_val) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-miniapp/profile",
        None,
        Some(&init_data),
        Some(json!({
            "locked_practice": {
                "title": "心经",
                "category": "禅室诵读",
                "file_path": "sutras/xinjing",
                "selected_at_ms": 1700000001000_i64,
                "is_locked": true
            }
        })),
    )
    .await;
    assert_eq!(st_prof2, StatusCode::OK);
    assert_eq!(prof2_val["locked_practice"]["title"].as_str(), Some("金刚经"));

    // Membership activation syncs both fabushi_profiles and /api/alipay/check-membership
    let (st_vip, vip_val) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-miniapp/membership",
        None,
        Some(&init_data),
        Some(json!({ "plan": "yearly" })),
    )
    .await;
    assert_eq!(st_vip, StatusCode::OK);
    assert_eq!(vip_val["profile"]["is_vip"].as_bool(), Some(true));

    let (st_cm, cm_val) = http_json(
        &router,
        "GET",
        "/api/alipay/check-membership",
        Some(&tok),
        None,
        None,
    )
    .await;
    assert_eq!(st_cm, StatusCode::OK);
    assert_eq!(cm_val["is_active"].as_bool(), Some(true));

    // Follow toggle, leaderboard, heartbeat, and ai-chat fallback
    let (st_flw1, flw1) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-miniapp/follow",
        None,
        Some(&init_data),
        Some(json!({ "target_username": "lotus_master_bj" })),
    )
    .await;
    assert_eq!(st_flw1, StatusCode::OK);
    assert_eq!(flw1["is_following"].as_bool(), Some(true));

    let (st_lb, lb_val) = http_json(
        &router,
        "GET",
        "/api/v1/fabushi-miniapp/leaderboard?kind=practice",
        None,
        Some(&init_data),
        None,
    )
    .await;
    assert_eq!(st_lb, StatusCode::OK);
    assert_eq!(lb_val["kind"].as_str(), Some("practice"));
    assert!(!lb_val["entries"].as_array().unwrap().is_empty());

    let (st_hb, hb_val) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-miniapp/heartbeat",
        None,
        Some(&init_data),
        Some(json!({ "activity_type": "zen_room" })),
    )
    .await;
    assert_eq!(st_hb, StatusCode::OK);
    assert!(hb_val["online_count"].as_u64().unwrap_or(0) >= 1);

    let (st_ai, ai_val) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-miniapp/ai-chat",
        None,
        Some(&init_data),
        Some(json!({ "message": "请问如何修持金刚经？" })),
    )
    .await;
    assert_eq!(st_ai, StatusCode::OK);
    let ai_reply = ai_val["reply"].as_str().expect("ai reply");
    assert!(ai_reply.contains("金刚经"));
}

#[tokio::test]
async fn test_e2e_headless_gpui_action_fallback() {
    let (_dir, _state, router) = boot_test_server();
    let _ = publish_official_fabushi(&router).await;
    let (_uid, tok) = register_user(&router, "gpui_headless_user").await;

    let (st_inst, _) = http_json(
        &router,
        "POST",
        "/api/v1/miniapps/fabushi_bot/fabushi/install",
        Some(&tok),
        None,
        Some(json!({ "allow_write_access": true })),
    )
    .await;
    assert_eq!(st_inst, StatusCode::OK);

    // Trigger widget_event + main_button_click on /miniapps/fabushi_bot/fabushi/action
    let (st_w, w_val) = http_json(
        &router,
        "POST",
        "/api/v1/miniapps/fabushi_bot/fabushi/action",
        Some(&tok),
        None,
        Some(json!({
            "action_type": "widget_event",
            "widget_id": "target_country",
            "value": "CN"
        })),
    )
    .await;
    assert_eq!(st_w, StatusCode::OK, "widget_event: {w_val}");

    let (st_act, act_val) = http_json(
        &router,
        "POST",
        "/api/v1/miniapps/fabushi_bot/fabushi/action",
        Some(&tok),
        None,
        Some(json!({ "action_type": "main_button_click" })),
    )
    .await;
    assert_eq!(st_act, StatusCode::OK, "main_button_click: {act_val}");
    assert_eq!(act_val["ok"].as_bool(), Some(true));
    assert!(
        act_val["state_json"]["total_transfer_count"]
            .as_u64()
            .unwrap_or(0)
            >= 1,
        "headless action must record transfer in SQLite: {act_val}"
    );
    assert!(act_val["companion_bot_message"]
        .as_str()
        .is_some_and(|s| s.contains("心经")));
}

#[tokio::test]
async fn test_e2e_publish_fabushi_miniapp_cli_fresh_and_idempotent_rerun() {
    let (_dir, _state, router) = boot_test_server();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    let server_url = format!("http://127.0.0.1:{}", addr.port());

    let server_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let bin_path = env!("CARGO_BIN_EXE_publish_fabushi_miniapp");

    for run_idx in [1, 2] {
        let output = tokio::process::Command::new(bin_path)
            .arg("--server-url")
            .arg(&server_url)
            .output()
            .await
            .expect("spawn publish_fabushi_miniapp");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "publish_fabushi_miniapp run #{run_idx} failed (status={}):\nstdout: {stdout}\nstderr: {stderr}",
            output.status
        );
        assert!(stdout.contains("Verified 200 OK"), "run #{run_idx} stdout: {stdout}");
    }

    server_handle.abort();
}
