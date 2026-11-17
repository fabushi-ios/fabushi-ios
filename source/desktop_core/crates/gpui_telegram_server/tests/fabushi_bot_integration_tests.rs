use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use gpui_telegram_proto::miniapp_contracts::{
    CreateFabushiBotRequest, FabushiBotCreateResponse, FabushiBotSummaryDto,
    PatchFabushiBotRequest, BOTFATHER_USER_ID,
};
use gpui_telegram_server::{
    build_router_with_state, config::ServerConfig, server::AppState,
};
use serde_json::{json, Value};
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

fn boot_temp_server(prefix: &str) -> (TempDirGuard, Arc<AppState>, Router) {
    let dir = TempDirGuard::new(prefix);
    let db_path = dir.path().join("fabushi_e2e.db");
    let media_dir = dir.path().join("media_cas");
    std::fs::create_dir_all(&media_dir).expect("create media dir");
    let mut cfg = ServerConfig::default();
    cfg.database_url = format!("sqlite://{}", db_path.display());
    cfg.media_storage_path = media_dir.to_string_lossy().to_string();
    cfg.host = "127.0.0.1".to_string();
    cfg.port = 0;

    let state = Arc::new(AppState::try_new(cfg).expect("boot AppState with FabushiBotHost"));
    let router = build_router_with_state(state.clone());
    (dir, state, router)
}

async fn http_json(
    router: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
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
            "password": "FabushiStrongPassword!2026",
            "first_name": username,
            "last_name": "Tester",
            "device_name": "FabushiDesktopTest"
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
    assert_eq!(status, StatusCode::OK, "list messages for {chat_id}: {val}");
    val["messages"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn unified_provisioning_across_all_entrypoints_and_official_bots() {
    let (dir, state, router) = boot_temp_server("fabushi_e2e_provisioning");
    let (_uid, token) = register_user(&router, "fabushi_owner_1").await;

    // 1) Official Hermes bot hydrated on boot (777_000); @BotFather and unseeded flagship Mini App bots (777_010..=777_012) excluded.
    let (st, list_val) = http_json(&router, "GET", "/api/v1/fabushi-bots", Some(&token), None).await;
    assert_eq!(st, StatusCode::OK, "list fabushi bots: {list_val}");
    let initial_bots: Vec<FabushiBotSummaryDto> =
        serde_json::from_value(list_val["bots"].clone()).expect("deserialize bots list");
    assert!(
        initial_bots.iter().all(|b| b.bot_user_id != BOTFATHER_USER_ID as i64),
        "BotFather must NEVER be exposed as a Fabushi LLM bot"
    );
    assert!(
        state.fabushi_bots.get_by_user_id(BOTFATHER_USER_ID as i64).is_none(),
        "BotFather must not be provisioned into FabushiBotHost"
    );
    for official_uid in [777_000_i64] {
        assert!(
            initial_bots.iter().any(|b| b.bot_user_id == official_uid),
            "Official bot {official_uid} missing from GET /api/v1/fabushi-bots"
        );
        let inst = state
            .fabushi_bots
            .get_by_user_id(official_uid)
            .expect("Official bot hydrated in FabushiBotHost");
        assert!(
            inst.agent_dir.join("profile.json").exists(),
            "Missing profile.json for official bot {official_uid}"
        );
        assert!(
            inst.agent_dir.join("settings.json").exists(),
            "Missing settings.json for official bot {official_uid}"
        );
    }
    for removed_flagship_uid in [777_010_i64, 777_011, 777_012] {
        assert!(
            initial_bots.iter().all(|b| b.bot_user_id != removed_flagship_uid),
            "Flagship bot {removed_flagship_uid} should not be seeded in GET /api/v1/fabushi-bots"
        );
        assert!(
            state.fabushi_bots.get_by_user_id(removed_flagship_uid).is_none(),
            "Flagship bot {removed_flagship_uid} must not be hydrated in FabushiBotHost"
        );
    }

    // 2) Flow A: POST /api/v1/fabushi-bots (first-class creator)
    let req = CreateFabushiBotRequest {
        display_name: "Alpha Analyst".to_string(),
        username: Some("alpha_analyst_bot".to_string()),
        agent_id: None,
        description: "First-class financial researcher".to_string(),
        title: "Senior Analyst".to_string(),
        avatar_shape: Some("hex".to_string()),
        avatar_color: Some("#FF6B6B".to_string()),
        preferred_model: Some("claude-sonnet-4".to_string()),
        thinking_level: Some("high".to_string()),
        approval_mode: Some("autonomous".to_string()),
        toolset_preset: Some("full".to_string()),
    };
    let (st_a, val_a) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&token),
        Some(serde_json::to_value(&req).unwrap()),
    )
    .await;
    assert_eq!(st_a, StatusCode::CREATED, "POST /api/v1/fabushi-bots: {val_a}");
    let created_a: FabushiBotCreateResponse =
        serde_json::from_value(val_a).expect("parse FabushiBotCreateResponse");
    assert_eq!(created_a.bot.avatar_shape, "hex");
    assert_eq!(created_a.bot.avatar_color, "#FF6B6B");
    assert_eq!(created_a.bot.thinking_level, "high");
    assert_eq!(created_a.bot.approval_mode, "autonomous");
    assert!(created_a.companion_chat_id > 0);
    let inst_a = state
        .fabushi_bots
        .get_by_user_id(created_a.bot.bot_user_id)
        .expect("Flow A bot in FabushiBotHost");
    assert!(inst_a.agent_dir.starts_with(dir.path().join("fabushi_agents")));
    assert!(inst_a.agent_dir.join("profile.json").exists());
    assert!(inst_a.agent_dir.join("settings.json").exists());

    // 3) Flow B: POST /api/v1/bots (legacy bot creator)
    let (st_b, val_b) = http_json(
        &router,
        "POST",
        "/api/v1/bots",
        Some(&token),
        Some(json!({
            "display_name": "Legacy Echo Bot",
            "username": "legacy_echo_bot",
            "description": "Created via POST /api/v1/bots"
        })),
    )
    .await;
    assert_eq!(st_b, StatusCode::CREATED, "POST /api/v1/bots: {val_b}");
    let bot_b_uid = val_b["bot"]["bot_user_id"].as_i64().expect("bot_user_id");
    let inst_b = state
        .fabushi_bots
        .get_by_user_id(bot_b_uid)
        .expect("POST /api/v1/bots must also provision FabushiBotHost");
    assert!(inst_b.agent_dir.join("profile.json").exists());
    assert!(inst_b.agent_dir.join("settings.json").exists());

    // 4) Flow C: POST /api/v1/botfather/nl-create
    let (st_c, val_c) = http_json(
        &router,
        "POST",
        "/api/v1/botfather/nl-create",
        Some(&token),
        Some(json!({
            "prompt": "帮我创建一个 Pomodoro 专注计时器助手 Bot"
        })),
    )
    .await;
    assert_eq!(st_c, StatusCode::CREATED, "POST /api/v1/botfather/nl-create: {val_c}");
    let bot_c_uid = val_c["bot"]["bot_user_id"].as_i64().expect("nl bot_user_id");
    let inst_c = state
        .fabushi_bots
        .get_by_user_id(bot_c_uid)
        .expect("nl-create must also provision FabushiBotHost");
    assert!(inst_c.agent_dir.join("profile.json").exists());
    assert!(inst_c.agent_dir.join("settings.json").exists());

    // 5) Flow D: POST /api/v1/contacts/add with is_bot: true
    let (st_d, val_d) = http_json(
        &router,
        "POST",
        "/api/v1/contacts/add",
        Some(&token),
        Some(json!({
            "first_name": "ContactBot",
            "username": "contact_custom_bot",
            "is_bot": true,
            "persona": "Helpful code reviewer contact bot"
        })),
    )
    .await;
    assert!(st_d.is_success(), "POST /api/v1/contacts/add: {val_d}");
    let bot_d_uid = val_d["user"]["user_id"]
        .as_i64()
        .or_else(|| val_d["contact"]["user_id"].as_i64())
        .or_else(|| val_d["user_id"].as_i64())
        .expect("contact bot user_id");
    let inst_d = state
        .fabushi_bots
        .get_by_user_id(bot_d_uid)
        .expect("contacts/add is_bot=true must also provision FabushiBotHost");
    assert!(inst_d.agent_dir.join("profile.json").exists());
    assert!(inst_d.agent_dir.join("settings.json").exists());

    // Verify all 4 newly created bots show up in GET /api/v1/fabushi-bots
    let (st_all, val_all) = http_json(&router, "GET", "/api/v1/fabushi-bots", Some(&token), None).await;
    assert_eq!(st_all, StatusCode::OK);
    let all_bots: Vec<FabushiBotSummaryDto> =
        serde_json::from_value(val_all["bots"].clone()).expect("all fabushi bots");
    for uid in [created_a.bot.bot_user_id, bot_b_uid, bot_c_uid, bot_d_uid] {
        assert!(
            all_bots.iter().any(|b| b.bot_user_id == uid),
            "Bot {uid} missing from GET /api/v1/fabushi-bots"
        );
    }
}

#[tokio::test]
async fn per_bot_hermes_isolation_memory_skills_soul_and_status() {
    let (_dir, _state, router) = boot_temp_server("fabushi_e2e_isolation");
    let (_uid, token) = register_user(&router, "fabushi_iso_owner").await;

    let (st_a, val_a) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&token),
        Some(json!({
            "display_name": "Bot One",
            "username": "bot_iso_one_bot",
            "description": "Isolated agent one"
        })),
    )
    .await;
    assert_eq!(st_a, StatusCode::CREATED, "create bot A: {val_a}");
    let bot_a = val_a["bot"]["bot_user_id"].as_u64().expect("bot A id");

    let (st_b, val_b) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&token),
        Some(json!({
            "display_name": "Bot Two",
            "username": "bot_iso_two_bot",
            "description": "Isolated agent two"
        })),
    )
    .await;
    assert_eq!(st_b, StatusCode::CREATED, "create bot B: {val_b}");
    let bot_b = val_b["bot"]["bot_user_id"].as_u64().expect("bot B id");

    // Mutate Memory, Skill, and SOUL.md ONLY on Bot A
    let (st_mem, _) = http_json(
        &router,
        "PUT",
        &format!("/api/v1/hermes/memory?bot_id={bot_a}"),
        Some(&token),
        Some(json!({
            "memory_markdown": "# Bot A Secret Memory\nSECRET_TOKEN_ALPHA_999\n",
            "user_markdown": "# Bot A User Prefs\nPrefers concise bullet points.\n"
        })),
    )
    .await;
    assert_eq!(st_mem, StatusCode::OK);

    let (st_sk, _) = http_json(
        &router,
        "POST",
        &format!("/api/v1/hermes/skills?bot_id={bot_a}"),
        Some(&token),
        Some(json!({
            "name": "bot_a_unique_skill",
            "description": "Skill exclusive to Bot A",
            "category": "workspace",
            "content": "# Bot A Skill\nRun alpha checks."
        })),
    )
    .await;
    assert!(st_sk.is_success(), "save skill for bot A failed: {st_sk}");

    let (st_soul, _) = http_json(
        &router,
        "PUT",
        "/api/v1/hermes/soul",
        Some(&token),
        Some(json!({
            "bot_id": bot_a,
            "persona_name": "Alpha Soul",
            "content": "# SOUL\nYou are Alpha Soul.\n"
        })),
    )
    .await;
    assert_eq!(st_soul, StatusCode::OK);

    // Inspect Bot A: must see all 3 artifacts
    let (st_ga, mem_a) = http_json(
        &router,
        "GET",
        &format!("/api/v1/hermes/memory?bot_id={bot_a}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_ga, StatusCode::OK);
    assert!(
        mem_a.to_string().contains("SECRET_TOKEN_ALPHA_999"),
        "Bot A memory did not persist: {mem_a}"
    );

    let (_, skills_a) = http_json(
        &router,
        "GET",
        &format!("/api/v1/hermes/skills?bot_id={bot_a}"),
        Some(&token),
        None,
    )
    .await;
    assert!(
        skills_a.to_string().contains("bot_a_unique_skill"),
        "Bot A skill missing: {skills_a}"
    );

    let (_, soul_a) = http_json(
        &router,
        "GET",
        &format!("/api/v1/hermes/soul?bot_id={bot_a}"),
        Some(&token),
        None,
    )
    .await;
    assert!(
        soul_a.to_string().contains("Alpha Soul"),
        "Bot A soul missing: {soul_a}"
    );

    // Inspect Bot B: ZERO leakage from Bot A
    let (_, mem_b) = http_json(
        &router,
        "GET",
        &format!("/api/v1/hermes/memory?bot_id={bot_b}"),
        Some(&token),
        None,
    )
    .await;
    assert!(
        !mem_b.to_string().contains("SECRET_TOKEN_ALPHA_999"),
        "Memory leaked from Bot A into Bot B: {mem_b}"
    );

    let (_, skills_b) = http_json(
        &router,
        "GET",
        &format!("/api/v1/hermes/skills?bot_id={bot_b}"),
        Some(&token),
        None,
    )
    .await;
    assert!(
        !skills_b.to_string().contains("bot_a_unique_skill"),
        "Skill leaked from Bot A into Bot B: {skills_b}"
    );

    let (_, soul_b) = http_json(
        &router,
        "GET",
        &format!("/api/v1/hermes/soul?bot_id={bot_b}"),
        Some(&token),
        None,
    )
    .await;
    assert!(
        !soul_b.to_string().contains("Alpha Soul"),
        "SOUL leaked from Bot A into Bot B: {soul_b}"
    );

    // Inspect global host memory (no bot_id): also isolated from Bot A
    let (_, mem_host) = http_json(&router, "GET", "/api/v1/hermes/memory", Some(&token), None).await;
    assert!(
        !mem_host.to_string().contains("SECRET_TOKEN_ALPHA_999"),
        "Bot A memory leaked into host Hermes engine"
    );

    let (st_status_a, status_a) = http_json(
        &router,
        "GET",
        &format!("/api/v1/hermes/status?bot_id={bot_a}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_status_a, StatusCode::OK, "status_a: {status_a}");
}

#[tokio::test]
async fn patch_fabushi_bot_and_provider_sync_propagation() {
    let (_dir, state, router) = boot_temp_server("fabushi_e2e_patch_sync");
    let (_uid, token) = register_user(&router, "fabushi_patch_owner").await;

    let (st_c, val_c) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&token),
        Some(json!({
            "display_name": "Mutable Agent",
            "username": "mutable_fabushi_bot",
            "avatar_shape": "squircle",
            "avatar_color": "#4F8EF7"
        })),
    )
    .await;
    assert_eq!(st_c, StatusCode::CREATED);
    let bot_id = val_c["bot"]["bot_user_id"].as_i64().expect("bot_user_id");

    // Patch shape, color, thinking level, approval mode, description
    let patch_req = PatchFabushiBotRequest {
        display_name: Some("Mutable Agent V2".to_string()),
        description: Some("Updated deep thinking researcher".to_string()),
        avatar_shape: Some("wedge".to_string()),
        avatar_color: Some("#8B5CF6".to_string()),
        preferred_model: Some("gpt-4o".to_string()),
        thinking_level: Some("high".to_string()),
        approval_mode: Some("autonomous".to_string()),
        ..Default::default()
    };
    let (st_p, val_p) = http_json(
        &router,
        "PATCH",
        &format!("/api/v1/fabushi-bots/{bot_id}"),
        Some(&token),
        Some(serde_json::to_value(&patch_req).unwrap()),
    )
    .await;
    assert_eq!(st_p, StatusCode::OK, "PATCH failed: {val_p}");
    let patched: FabushiBotSummaryDto = serde_json::from_value(val_p).expect("patched summary");
    assert_eq!(patched.display_name, "Mutable Agent V2");
    assert_eq!(patched.avatar_shape, "wedge");
    assert_eq!(patched.avatar_color, "#8B5CF6");
    assert_eq!(patched.preferred_model, "gpt-4o");
    assert_eq!(patched.thinking_level, "high");
    assert_eq!(patched.approval_mode, "autonomous");

    let (st_g, val_g) = http_json(
        &router,
        "GET",
        &format!("/api/v1/fabushi-bots/{bot_id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_g, StatusCode::OK);
    let fetched: FabushiBotSummaryDto = serde_json::from_value(val_g).expect("get summary");
    assert_eq!(fetched.avatar_shape, "wedge");
    assert_eq!(fetched.thinking_level, "high");

    // Verify host provider update propagates into provisioned bot instances
    let (st_prov, val_prov) = http_json(
        &router,
        "PUT",
        "/api/v1/hermes/providers",
        Some(&token),
        Some(json!({
            "provider_id": "openrouter",
            "name": "OpenRouter",
            "base_url": "https://openrouter.ai/api/v1",
            "default_model": "anthropic/claude-sonnet-4",
            "api_key": "sk-or-test-key-xyz",
            "set_active": true,
            "enabled": true
        })),
    )
    .await;
    assert_eq!(st_prov, StatusCode::OK, "PUT /api/v1/hermes/providers: {val_prov}");

    let inst = state
        .fabushi_bots
        .get_by_user_id(bot_id)
        .expect("hydrated instance");
    let bot_prov_snap = inst.hermes().providers().snapshot();
    assert_eq!(bot_prov_snap.active_provider_id, "openrouter");
    let or_cfg = bot_prov_snap
        .providers
        .get("openrouter")
        .expect("openrouter synced into bot");
    assert_eq!(or_cfg.default_model, "anthropic/claude-sonnet-4");
}

#[tokio::test]
async fn multi_bot_shared_room_mention_and_all_routing() {
    let (_dir, _state, router) = boot_temp_server("fabushi_e2e_multi_bot_room");
    let (_uid, token) = register_user(&router, "fabushi_room_owner").await;

    let (st_a, val_a) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&token),
        Some(json!({
            "display_name": "Coder Bot",
            "username": "room_coder_bot",
            "description": "Writes Rust code"
        })),
    )
    .await;
    assert_eq!(st_a, StatusCode::CREATED);
    let bot_a = val_a["bot"]["bot_user_id"].as_u64().expect("bot_a");

    let (st_b, val_b) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&token),
        Some(json!({
            "display_name": "Reviewer Bot",
            "username": "room_reviewer_bot",
            "description": "Reviews code"
        })),
    )
    .await;
    assert_eq!(st_b, StatusCode::CREATED);
    let bot_b = val_b["bot"]["bot_user_id"].as_u64().expect("bot_b");

    // Create shared group chat with both bots
    let (st_chat, val_chat) = http_json(
        &router,
        "POST",
        "/api/v1/chats",
        Some(&token),
        Some(json!({
            "title": "Fabushi War Room",
            "chat_type": 1,
            "member_ids": [bot_a, bot_b]
        })),
    )
    .await;
    assert_eq!(st_chat, StatusCode::CREATED, "create group chat: {val_chat}");
    let group_cid = val_chat["chat_id"].as_i64().expect("group chat_id");

    // 1) Mention ONLY @room_coder_bot -> only Coder Bot should respond
    let (st_m1, val_m1) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{group_cid}/messages"),
        Some(&token),
        Some(json!({ "text": "@room_coder_bot please check build" })),
    )
    .await;
    assert!(st_m1.is_success(), "send mention 1: {val_m1}");
    let m1_id = val_m1["message_id"].as_i64().unwrap_or(0);

    let mut saw_a = false;
    for _ in 0..80 {
        let msgs = list_chat_messages(&router, &token, group_cid).await;
        if msgs.iter().any(|m| {
            m["id"].as_i64().unwrap_or(0) > m1_id && m["sender_id"].as_u64() == Some(bot_a)
        }) {
            saw_a = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(saw_a, "Bot A should reply when explicitly @mentioned in group");
    tokio::time::sleep(Duration::from_millis(120)).await;
    let msgs_after_m1 = list_chat_messages(&router, &token, group_cid).await;
    assert!(
        !msgs_after_m1.iter().any(|m| {
            m["id"].as_i64().unwrap_or(0) > m1_id && m["sender_id"].as_u64() == Some(bot_b)
        }),
        "Bot B must NOT reply when only @room_coder_bot was mentioned"
    );

    // 2) Mention @all -> BOTH Bot A and Bot B should respond
    let (st_m2, val_m2) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{group_cid}/messages"),
        Some(&token),
        Some(json!({ "text": "@all standup status update" })),
    )
    .await;
    assert!(st_m2.is_success(), "send mention @all: {val_m2}");
    let m2_id = val_m2["message_id"].as_i64().unwrap_or(0);

    let (mut saw_a2, mut saw_b2) = (false, false);
    for _ in 0..100 {
        let msgs = list_chat_messages(&router, &token, group_cid).await;
        saw_a2 = msgs.iter().any(|m| {
            m["id"].as_i64().unwrap_or(0) > m2_id && m["sender_id"].as_u64() == Some(bot_a)
        });
        saw_b2 = msgs.iter().any(|m| {
            m["id"].as_i64().unwrap_or(0) > m2_id && m["sender_id"].as_u64() == Some(bot_b)
        });
        if saw_a2 && saw_b2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(saw_a2 && saw_b2, "Both Bot A ({saw_a2}) and Bot B ({saw_b2}) must reply to @all");
}

#[tokio::test]
async fn grok_dynamic_avatar_persists_across_all_bot_creation_flows() {
    let (_dir, state, router) = boot_temp_server("fabushi_e2e_grok_avatar");
    let (_uid, token) = register_user(&router, "grok_avatar_owner").await;

    // 1) POST /api/v1/fabushi-bots -> ("cloud", "violet")
    let (st_a, val_a) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&token),
        Some(json!({
            "display_name": "Cloud Violet Agent",
            "username": "cloud_violet_bot",
            "description": "Grok avatar cloud violet",
            "avatar_shape": "cloud",
            "avatar_color": "violet"
        })),
    )
    .await;
    assert_eq!(st_a, StatusCode::CREATED, "POST /api/v1/fabushi-bots: {val_a}");
    let created_a: FabushiBotCreateResponse =
        serde_json::from_value(val_a).expect("parse Flow A response");
    assert_eq!(created_a.bot.avatar_shape, "cloud");
    assert_eq!(created_a.bot.avatar_color, "violet");
    assert_eq!(created_a.registry.avatar_shape, "cloud");
    assert_eq!(created_a.registry.avatar_color, "violet");

    // 2) POST /api/v1/bots -> ("teardrop", "black")
    let (st_b, val_b) = http_json(
        &router,
        "POST",
        "/api/v1/bots",
        Some(&token),
        Some(json!({
            "display_name": "Teardrop Black Bot",
            "username": "teardrop_black_bot",
            "description": "Grok avatar teardrop black",
            "avatar_shape": "teardrop",
            "avatar_color": "black"
        })),
    )
    .await;
    assert_eq!(st_b, StatusCode::CREATED, "POST /api/v1/bots: {val_b}");
    let bot_b_uid = val_b["bot"]["bot_user_id"].as_i64().expect("bot_b uid");
    assert_eq!(val_b["bot"]["avatar_shape"].as_str(), Some("teardrop"));
    assert_eq!(val_b["bot"]["avatar_color"].as_str(), Some("black"));

    // 3) POST /api/v1/botfather/nl-create -> ("pebble", "orange")
    let (st_c, val_c) = http_json(
        &router,
        "POST",
        "/api/v1/botfather/nl-create",
        Some(&token),
        Some(json!({
            "prompt": "帮我创建一个每日新闻摘要机器人",
            "avatar_shape": "pebble",
            "avatar_color": "orange"
        })),
    )
    .await;
    assert_eq!(st_c, StatusCode::CREATED, "POST /api/v1/botfather/nl-create: {val_c}");
    let bot_c_uid = val_c["bot"]["bot_user_id"].as_i64().expect("bot_c uid");
    assert_eq!(val_c["bot"]["avatar_shape"].as_str(), Some("pebble"));
    assert_eq!(val_c["bot"]["avatar_color"].as_str(), Some("orange"));

    // 4) POST /api/v1/contacts/add (is_bot: true) -> ("squircle", "cyan")
    let (st_d, val_d) = http_json(
        &router,
        "POST",
        "/api/v1/contacts/add",
        Some(&token),
        Some(json!({
            "first_name": "Squircle Cyan Bot",
            "username": "squircle_cyan_bot",
            "is_bot": true,
            "persona": "Cyan squircle assistant",
            "avatar_shape": "squircle",
            "avatar_color": "cyan"
        })),
    )
    .await;
    assert!(st_d.is_success(), "POST /api/v1/contacts/add: {val_d}");
    let bot_d_uid = val_d["user_id"].as_i64().expect("bot_d uid");
    assert_eq!(val_d["avatar_shape"].as_str(), Some("squircle"));
    assert_eq!(val_d["avatar_color_id"].as_str(), Some("cyan"));

    // Verify GET /api/v1/fabushi-bots and disk profile.json for all 4 bots
    let (st_list, list_val) = http_json(&router, "GET", "/api/v1/fabushi-bots", Some(&token), None).await;
    assert_eq!(st_list, StatusCode::OK, "GET /api/v1/fabushi-bots: {list_val}");
    let all_bots: Vec<FabushiBotSummaryDto> =
        serde_json::from_value(list_val["bots"].clone()).expect("deserialize bots");

    let expected = [
        (created_a.bot.bot_user_id, "cloud", "violet"),
        (bot_b_uid, "teardrop", "black"),
        (bot_c_uid, "pebble", "orange"),
        (bot_d_uid, "squircle", "cyan"),
    ];
    for (uid, exp_shape, exp_color) in expected {
        let dto = all_bots
            .iter()
            .find(|b| b.bot_user_id == uid)
            .unwrap_or_else(|| panic!("Bot {uid} missing from GET /api/v1/fabushi-bots"));
        assert_eq!(dto.avatar_shape, exp_shape, "summary shape mismatch for {uid}");
        assert_eq!(dto.avatar_color, exp_color, "summary color mismatch for {uid}");

        let inst = state
            .fabushi_bots
            .get_by_user_id(uid)
            .unwrap_or_else(|| panic!("Bot {uid} missing from FabushiBotHost"));
        let profile_raw = std::fs::read_to_string(inst.agent_dir.join("profile.json"))
            .expect("read profile.json");
        let profile_json: Value =
            serde_json::from_str(&profile_raw).expect("parse profile.json");
        assert_eq!(
            profile_json["avatarShape"].as_str(),
            Some(exp_shape),
            "profile.json avatarShape mismatch for {uid}: {profile_json}"
        );
        assert_eq!(
            profile_json["avatarColor"].as_str(),
            Some(exp_color),
            "profile.json avatarColor mismatch for {uid}: {profile_json}"
        );
    }
}

#[tokio::test]
async fn botfather_newbot_and_setavatar_callbacks_persist_grok_avatar() {
    let (_dir, state, router) = boot_temp_server("fabushi_e2e_botfather_setavatar");
    let (_uid, token) = register_user(&router, "bf_avatar_owner").await;

    let (st_add, add_res) = http_json(
        &router,
        "POST",
        "/api/v1/contacts/add",
        Some(&token),
        Some(json!({ "username": "BotFather" })),
    )
    .await;
    assert!(st_add.is_success(), "add BotFather: {add_res}");
    let bf_cid = add_res["chat_id"].as_i64().expect("bf_cid");

    let send_bf_and_wait = |text: &str| {
        let router = router.clone();
        let token = token.clone();
        let text_owned = text.to_string();
        async move {
            let (st, res) = http_json(
                &router,
                "POST",
                &format!("/api/v1/chats/{bf_cid}/messages"),
                Some(&token),
                Some(json!({ "text": text_owned })),
            )
            .await;
            assert!(st.is_success(), "send BotFather msg failed: {res}");
            let sent_id = res["message_id"].as_i64().unwrap_or(0);
            for _ in 0..80 {
                let msgs = list_chat_messages(&router, &token, bf_cid).await;
                if let Some(reply) = msgs.iter().rev().find(|m| {
                    m["id"].as_i64().unwrap_or(0) > sent_id
                        && m["sender_id"].as_i64() == Some(BOTFATHER_USER_ID)
                }) {
                    return reply.clone();
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("Timed out waiting for BotFather reply to `{text_owned}`");
        }
    };

    let m1 = send_bf_and_wait("/newbot").await;
    assert!(m1["text"].as_str().unwrap_or("").contains("显示名称"));
    let m2 = send_bf_and_wait("Avatar Test Bot").await;
    assert!(m2["text"].as_str().unwrap_or("").contains("username"));
    let created_msg = send_bf_and_wait("avatar_menu_test_bot").await;
    assert!(
        created_msg["text"]
            .as_str()
            .unwrap_or("")
            .contains("@avatar_menu_test_bot"),
        "expected completion text: {created_msg}"
    );
    assert!(
        created_msg["inline_keyboard"]
            .to_string()
            .contains("bf:avatar_menu:avatar_menu_test_bot"),
        "expected bf:avatar_menu button on /newbot completion: {created_msg}"
    );
    let msg_id = created_msg["id"].as_i64().unwrap_or(1);

    let (st_menu, val_menu) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{bf_cid}/callback"),
        Some(&token),
        Some(json!({
            "message_id": msg_id,
            "callback_data": "bf:avatar_menu:avatar_menu_test_bot"
        })),
    )
    .await;
    assert_eq!(st_menu, StatusCode::OK, "bf:avatar_menu: {val_menu}");
    assert!(val_menu["inline_keyboard"]
        .to_string()
        .contains("bf:set_shape:avatar_menu_test_bot:classic"));

    let (st_sh, val_sh) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{bf_cid}/callback"),
        Some(&token),
        Some(json!({
            "message_id": msg_id,
            "callback_data": "bf:set_shape:avatar_menu_test_bot:classic"
        })),
    )
    .await;
    assert_eq!(st_sh, StatusCode::OK, "bf:set_shape: {val_sh}");
    assert_eq!(val_sh["companion_bot"]["avatar_shape"].as_str(), Some("classic"));

    let (st_co, val_co) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{bf_cid}/callback"),
        Some(&token),
        Some(json!({
            "message_id": msg_id,
            "callback_data": "bf:set_color:avatar_menu_test_bot:magenta"
        })),
    )
    .await;
    assert_eq!(st_co, StatusCode::OK, "bf:set_color: {val_co}");
    let bot_uid = val_co["companion_bot"]["bot_user_id"].as_i64().expect("bot_uid");
    assert_eq!(val_co["companion_bot"]["avatar_shape"].as_str(), Some("classic"));
    assert_eq!(val_co["companion_bot"]["avatar_color"].as_str(), Some("magenta"));

    // Verify GET /api/v1/fabushi-bots/{bot_uid} and profile.json reflect classic + magenta
    let (st_get, val_get) = http_json(
        &router,
        "GET",
        &format!("/api/v1/fabushi-bots/{bot_uid}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st_get, StatusCode::OK, "GET /api/v1/fabushi-bots/{bot_uid}: {val_get}");
    assert_eq!(val_get["avatar_shape"].as_str(), Some("classic"));
    assert_eq!(val_get["avatar_color"].as_str(), Some("magenta"));

    assert!(
        state.fabushi_bots.get_by_user_id(bot_uid).is_none(),
        "BotFather /newbot must create a standard Telegram bot without hydrating FabushiBotHost"
    );
    let db_bot = state
        .db
        .get_bot_by_user_id(bot_uid)
        .expect("query bot in db")
        .expect("bot record in db");
    assert_eq!(db_bot.avatar_shape, "classic");
    assert_eq!(db_bot.avatar_color, "magenta");

    // Also verify bf:set_shape/bf:set_color updates profile.json on a Fabushi bot created via POST /api/v1/fabushi-bots
    let fb_req = CreateFabushiBotRequest {
        display_name: "Fabushi Avatar Test Bot".to_string(),
        username: Some("fabushi_avatar_menu_bot".to_string()),
        agent_id: None,
        description: "Avatar callback persistence test bot".to_string(),
        title: "Avatar Tester".to_string(),
        avatar_shape: None,
        avatar_color: None,
        preferred_model: None,
        thinking_level: None,
        approval_mode: None,
        toolset_preset: None,
    };
    let (st_fb, val_fb) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&token),
        Some(serde_json::to_value(&fb_req).unwrap()),
    )
    .await;
    assert_eq!(st_fb, StatusCode::CREATED, "POST /api/v1/fabushi-bots: {val_fb}");
    let created_fb: FabushiBotCreateResponse =
        serde_json::from_value(val_fb).expect("parse FabushiBotCreateResponse");
    let (st_fb_sh, val_fb_sh) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{bf_cid}/callback"),
        Some(&token),
        Some(json!({
            "message_id": msg_id,
            "callback_data": "bf:set_shape:fabushi_avatar_menu_bot:classic"
        })),
    )
    .await;
    assert_eq!(st_fb_sh, StatusCode::OK, "fb bf:set_shape: {val_fb_sh}");
    let (st_fb_co, val_fb_co) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{bf_cid}/callback"),
        Some(&token),
        Some(json!({
            "message_id": msg_id,
            "callback_data": "bf:set_color:fabushi_avatar_menu_bot:magenta"
        })),
    )
    .await;
    assert_eq!(st_fb_co, StatusCode::OK, "fb bf:set_color: {val_fb_co}");
    let inst = state
        .fabushi_bots
        .get_by_user_id(created_fb.bot.bot_user_id)
        .expect("fabushi bot in host");
    let profile_json: Value = serde_json::from_str(
        &std::fs::read_to_string(inst.agent_dir.join("profile.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(profile_json["avatarShape"].as_str(), Some("classic"));
    assert_eq!(profile_json["avatarColor"].as_str(), Some("magenta"));

    // Also test /setavatar slash command -> @avatar_menu_test_bot
    let pick_prompt = send_bf_and_wait("/setavatar").await;
    assert!(
        pick_prompt["inline_keyboard"]
            .to_string()
            .contains("bf:pick:setavatar:avatar_menu_test_bot"),
        "expected pick keyboard from /setavatar: {pick_prompt}"
    );
    let menu_reply = send_bf_and_wait("@avatar_menu_test_bot").await;
    assert!(
        menu_reply["inline_keyboard"]
            .to_string()
            .contains("bf:set_shape:avatar_menu_test_bot:wedge"),
        "Expected /setavatar to emit shape/color inline keyboard: {menu_reply}"
    );
}
