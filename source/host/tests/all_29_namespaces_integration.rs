//! End-to-End Integration Test for `mahayana_app_host` across all 29 domain namespaces
//! backed by a live in-process `gpui_telegram_server` on `127.0.0.1:0`.

use gpui_telegram_server::{build_router_with_state, server::AppState, ServerConfig};
use mahayana_app_host::host::{IOS_DEFAULT_SERVER_URL, TelegramAppHost};
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;

async fn spawn_live_server() -> (TempDir, String, tokio::task::JoinHandle<()>) {
    let dir = TempDir::new().expect("server tempdir");
    let cfg = ServerConfig {
        database_url: dir.path().join("server.db").to_string_lossy().into_owned(),
        media_storage_path: dir.path().join("cas").to_string_lossy().into_owned(),
        ..Default::default()
    };
    let state = Arc::new(AppState::try_new(cfg).expect("server AppState"));
    let router = build_router_with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local_addr");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (dir, format!("http://{addr}"), handle)
}

fn rpc(host: &TelegramAppHost, command: &str, params: Value) -> Value {
    let raw = host.dispatch(&json!({ "command": command, "params": params }).to_string());
    serde_json::from_str(&raw).expect("valid JSON envelope")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_all_29_namespaces_against_live_loopback_server() {
    assert_eq!(IOS_DEFAULT_SERVER_URL, "http://34.46.77.133:8080");

    let (_srv_dir, server_url, srv_task) = spawn_live_server().await;
    let client_dir = TempDir::new().expect("client tempdir");
    let host = TelegramAppHost::new(client_dir.path().to_path_buf(), None, true)
        .expect("TelegramAppHost::new");

    // 1. config.*
    let set_srv = rpc(&host, "config.set_server_url", json!({ "server_url": server_url, "language": "zh-CN" }));
    assert_eq!(set_srv["ok"], true, "{set_srv}");
    let health = rpc(&host, "config.ping_health", json!({}));
    assert_eq!(health["ok"], true);
    assert_eq!(health["result"]["healthy"], true, "{health}");

    // 2. auth.* & 3. accounts.*
    let reg = rpc(&host, "auth.register", json!({
        "username": "alice_m1",
        "password": "StrongPassword!123",
        "first_name": "Alice M1"
    }));
    assert_eq!(reg["ok"], true, "{reg}");
    let alice_uid = reg["result"]["user_id"].as_u64().expect("alice user_id");

    let accts = rpc(&host, "accounts.list", json!({}));
    assert_eq!(accts["ok"], true);
    assert_eq!(accts["result"]["active_user_id"], alice_uid);

    // 4. sync.*
    let diff = rpc(&host, "sync.get_difference", json!({ "pts": 0 }));
    assert_eq!(diff["ok"], true, "{diff}");

    // 5. chats.* & 6. folders.*
    let created_chat = rpc(&host, "chats.create_group", json!({ "title": "M1 Core Group" }));
    assert_eq!(created_chat["ok"], true, "{created_chat}");
    let chat_id = created_chat["result"]["chat_id"]
        .as_i64()
        .or_else(|| created_chat["result"]["id"].as_i64())
        .expect("chat_id");

    let folder = rpc(&host, "folders.create", json!({
        "id": 10,
        "title": "Work",
        "include_chats": [chat_id]
    }));
    assert_eq!(folder["ok"], true, "{folder}");

    // 7. stories.*
    let stories = rpc(&host, "stories.list", json!({}));
    assert_eq!(stories["ok"], true, "{stories}");

    // 9. messages.* & 8. search.* & 10. drafts.* & 11. outbox.*
    let sent = rpc(&host, "messages.send", json!({
        "chat_id": chat_id,
        "text": "Hello **world** from `mahayana_app_host`!",
        "parse_markdown": true,
        "client_nonce": 424242
    }));
    assert_eq!(sent["ok"], true, "{sent}");
    let msg_id = sent["result"]["id"].as_i64().expect("msg_id");

    let search = rpc(&host, "search.global", json!({ "query": "mahayana_app_host" }));
    assert_eq!(search["ok"], true, "{search}");

    let draft = rpc(&host, "drafts.save", json!({ "chat_id": chat_id, "text": "WIP draft" }));
    assert_eq!(draft["ok"], true, "{draft}");
    let ime = rpc(&host, "drafts.ime_handle_key", json!({ "has_marked_text": true }));
    assert_eq!(ime["result"]["decision"], "consume_marked_composition");

    let outbox = rpc(&host, "outbox.list", json!({}));
    assert_eq!(outbox["ok"], true, "{outbox}");

    // 12. e2ee.*
    let prekeys = rpc(&host, "e2ee.bootstrap", json!({ "otpk_batch_size": 8 }));
    assert_eq!(prekeys["ok"], true, "{prekeys}");
    let e2ee_st = rpc(&host, "e2ee.status", json!({}));
    assert_eq!(e2ee_st["result"]["initialized"], true);

    // 13. media.* & 14. stickers.* & 15. reactions.* & 16. polls.*
    let wf = rpc(&host, "media.encode_waveform", json!({ "samples": [0.1, 0.5, 0.9, 0.3] }));
    assert_eq!(wf["ok"], true);
    let wf_dec = rpc(&host, "media.decode_waveform", json!({
        "waveform_b64": wf["result"]["waveform_b64"],
        "bar_count": 32
    }));
    assert_eq!(wf_dec["result"]["sample_count"], 32);

    let stickers = rpc(&host, "stickers.packs", json!({}));
    assert_eq!(stickers["ok"], true, "{stickers}");

    let react = rpc(&host, "reactions.toggle", json!({
        "chat_id": chat_id,
        "msg_id": msg_id,
        "emoji": "🔥",
        "add": true
    }));
    assert_eq!(react["ok"], true, "{react}");

    let poll = rpc(&host, "polls.create", json!({
        "chat_id": chat_id,
        "question": "Ready for M1?",
        "options": ["Yes", "100%"]
    }));
    assert_eq!(poll["ok"], true, "{poll}");

    // 17. peers.* & 18. contacts.*
    let members = rpc(&host, "peers.members", json!({ "chat_id": chat_id }));
    assert_eq!(members["ok"], true, "{members}");
    let contacts = rpc(&host, "contacts.list", json!({}));
    assert_eq!(contacts["ok"], true, "{contacts}");

    // 19. calls.* & 20. talk.*
    let fp = rpc(&host, "calls.emoji_fingerprint", json!({ "key_material": "shared-dtls-secret" }));
    assert_eq!(fp["ok"], true);
    assert!(!fp["result"]["emojis"].as_str().unwrap_or("").is_empty());

    let talk_start = rpc(&host, "talk.start", json!({ "peer_id": "hermes_voice", "tts_playing": true }));
    assert_eq!(talk_start["ok"], true);
    let loud_frame: Vec<i16> = vec![18_000; 320]; // 20ms @ 16kHz
    let _ = rpc(&host, "talk.feed_pcm", json!({ "pcm_i16": loud_frame }));
    let _ = rpc(&host, "talk.feed_pcm", json!({ "pcm_i16": loud_frame }));
    let third = rpc(&host, "talk.feed_pcm", json!({ "pcm_i16": loud_frame }));
    assert_eq!(third["result"]["barge_in"], true, "<60ms 3-frame VAD barge-in must fire");

    // 21. settings.* & 22. wallet.* & 23. stars.* & 24. alipay.*
    let pc_set = rpc(&host, "settings.passcode_set", json!({ "passcode": "2580" }));
    assert_eq!(pc_set["ok"], true);
    assert_eq!(rpc(&host, "settings.passcode_verify", json!({ "passcode": "2580" }))["result"]["valid"], true);
    assert_eq!(rpc(&host, "settings.passcode_verify", json!({ "passcode": "9999" }))["result"]["valid"], false);

    let wallet = rpc(&host, "wallet.create", json!({ "initial_nanotons": 5_000_000_000u64 }));
    assert_eq!(wallet["ok"], true, "{wallet}");
    let stars = rpc(&host, "stars.topup", json!({ "amount_stars": 250 }));
    assert_eq!(stars["ok"], true, "{stars}");
    let plans = rpc(&host, "alipay.plans", json!({}));
    assert_eq!(plans["result"]["plans"].as_array().unwrap().len(), 3);

    // 25. hermes.* & 26. actions.* & 27. magpie.*
    let cmds = rpc(&host, "hermes.commands_list", json!({}));
    assert!(cmds["result"]["count"].as_u64().unwrap_or(0) >= 50);

    let act_disc = rpc(&host, "actions.discover", json!({}));
    assert_eq!(act_disc["ok"], true, "{act_disc}");

    let mp = rpc(&host, "magpie.presets_list", json!({}));
    assert!(mp["result"]["preset_count"].as_u64().unwrap_or(0) >= 40);
    let masked = rpc(&host, "magpie.mask_pii", json!({ "text": "My key is sk-proj-1234567890abcdefghijklmnop" }));
    assert_eq!(masked["ok"], true);
    assert!(!masked["result"]["masked_text"].as_str().unwrap().contains("sk-proj-1234567890"));

    // 28. bots.*, miniapps.*, fabushi_miniapp.*
    let bots = rpc(&host, "bots.list", json!({}));
    assert_eq!(bots["ok"], true, "{bots}");
    let dharma_html = rpc(&host, "fabushi_miniapp.bundle_html", json!({}));
    assert!(dharma_html["result"]["html_bytes"].as_u64().unwrap() > 1000);
    let dharma_wasm = rpc(&host, "fabushi_miniapp.bundle_wasm_base64", json!({}));
    assert_eq!(dharma_wasm["result"]["magic_valid"], true);
    let zen = rpc(&host, "fabushi_miniapp.tap_zen", json!({ "sutra": "心经", "duration_minutes": 20, "chant_count": 108 }));
    assert_eq!(zen["ok"], true, "{zen}");
    let beam = rpc(&host, "fabushi_miniapp.emit_beam", json!({ "content_title": "金刚经", "bytes_sent": 4096 }));
    assert_eq!(beam["ok"], true, "{beam}");

    // 29. computers.*, tailnet.*, semantic.*
    let lease = rpc(&host, "computers.lease_acquire", json!({ "agent_id": "hermes", "device_id": "ios_1" }));
    assert_eq!(lease["ok"], true);
    let takeover = rpc(&host, "computers.emergency_takeover", json!({}));
    assert_eq!(takeover["result"]["preempted"], true);

    let tn = rpc(&host, "tailnet.status", json!({}));
    assert_eq!(tn["ok"], true, "{tn}");

    let snap1 = rpc(&host, "semantic.snapshot", json!({}));
    let hash1 = snap1["result"]["state_hash"].as_str().unwrap().to_string();
    assert_eq!(hash1.len(), 64);
    let act_res = rpc(&host, "semantic.act", json!({
        "action": "ComposeText",
        "params": { "text": "Deterministic state hash test" }
    }));
    assert_eq!(act_res["ok"], true);
    let hash2 = act_res["result"]["resulting_state_hash"].as_str().unwrap().to_string();
    assert_ne!(hash1, hash2);

    // ScenePhase background & foreground transitions (0xdead10cc prevention)
    let bg = rpc(&host, "config.lifecycle_background", json!({}));
    assert_eq!(bg["result"]["wal_checkpointed"], true);
    let fg = rpc(&host, "config.lifecycle_foreground", json!({}));
    assert_eq!(fg["result"]["backgrounded"], false);

    // Unknown command MUST fail closed
    let unk = rpc(&host, "nonexistent.domain.method", json!({}));
    assert_eq!(unk["ok"], false);
    assert_eq!(unk["error"]["code"], "unknown_command");

    host.shutdown();
    srv_task.abort();
}
