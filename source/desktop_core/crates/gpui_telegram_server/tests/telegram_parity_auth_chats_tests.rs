use axum::{body::{to_bytes, Body}, http::{header, Method, Request, StatusCode}, Router};
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
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn durable_app(dir: &TempDirGuard) -> (Arc<AppState>, Router) {
    let cfg = ServerConfig {
        database_url: dir.0.join("parity.db").to_string_lossy().into_owned(),
        media_storage_path: dir.0.join("cas").to_string_lossy().into_owned(),
        ..Default::default()
    };
    let state = Arc::new(AppState::try_new(cfg).expect("durable AppState must open"));
    let router = build_router_with_state(state.clone());
    (state, router)
}

async fn api(app: &Router, method: Method, uri: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token { b = b.header(header::AUTHORIZATION, format!("Bearer {t}")); }
    let req = match body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let st = res.status();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (st, json)
}

async fn reg(app: &Router, uname: &str, pw: &str) -> (u64, String) {
    let (st, v) = api(app, Method::POST, "/api/v1/auth/register", None, Some(json!({
        "username": uname, "password": pw, "first_name": uname, "device_name": "TestMac"
    }))).await;
    assert_eq!(st, StatusCode::CREATED);
    (v["user_id"].as_u64().unwrap(), v["token"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn test_status_durable_failure_phone_otp_2fa_qr_and_sessions() {
    let dir = TempDirGuard::new("tg_parity_auth");
    let bad_empty = ServerConfig { database_url: "".into(), media_storage_path: dir.0.join("c1").to_string_lossy().into(), ..Default::default() };
    assert!(AppState::try_new(bad_empty).is_err(), "Empty DB URL must fail visibly");
    let bad_dir = ServerConfig { database_url: dir.0.to_string_lossy().into(), media_storage_path: dir.0.join("c2").to_string_lossy().into(), ..Default::default() };
    assert!(AppState::try_new(bad_dir).is_err(), "Directory DB path must NOT fall back to :memory:");

    let (_st_arc, app) = durable_app(&dir);
    let (st, status) = api(&app, Method::GET, "/api/v1/status", None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    let caps = status["capabilities"].as_array().unwrap();
    assert!(caps.iter().any(|c| c == "phone_otp") && caps.iter().any(|c| c == "polls"));

    let (st, otp) = api(&app, Method::POST, "/api/v1/auth/phone/send-code", None, Some(json!({ "phone": "+14155550199" }))).await;
    assert_eq!(st, StatusCode::OK);
    let code = otp["debug_code"].as_str().expect("debug_code present in dev/test").to_string();
    let hash = otp["phone_code_hash"].as_str().unwrap().to_string();
    let (st_bad, _) = api(&app, Method::POST, "/api/v1/auth/phone/verify", None, Some(json!({
        "phone": "+14155550199", "phone_code_hash": hash, "code": "12345"
    }))).await;
    assert_eq!(st_bad, StatusCode::UNAUTHORIZED, "Hardcoded 12345 bypass must be rejected");
    let (st_ok, v_ok) = api(&app, Method::POST, "/api/v1/auth/phone/verify", None, Some(json!({
        "phone": "+14155550199", "phone_code_hash": hash, "code": code, "first_name": "PhoneUser"
    }))).await;
    assert_eq!(st_ok, StatusCode::OK);
    assert!(v_ok["token"].as_str().unwrap().len() > 10);

    let (_uid, alice_tok) = reg(&app, "alice_2fa", "StrongPass1!").await;
    let (st, s2fa) = api(&app, Method::POST, "/api/v1/auth/2fa/set", Some(&alice_tok), Some(json!({
        "new_password": "CloudPassword!9", "hint": "city"
    }))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(s2fa["has_password"], true);

    let (st, chal) = api(&app, Method::POST, "/api/v1/auth/login", None, Some(json!({
        "username": "alice_2fa", "password": "StrongPass1!", "device_name": "SecondMac"
    }))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(chal["requires_2fa"], true);
    let ctok = chal["challenge_token"].as_str().unwrap();
    let (st_w, _) = api(&app, Method::POST, "/api/v1/auth/2fa/verify", None, Some(json!({
        "challenge_token": ctok, "cloud_password": "wrong"
    }))).await;
    assert_eq!(st_w, StatusCode::UNAUTHORIZED);
    let (st_r, tfa_ok) = api(&app, Method::POST, "/api/v1/auth/2fa/verify", None, Some(json!({
        "challenge_token": ctok, "cloud_password": "CloudPassword!9", "device_name": "SecondMac"
    }))).await;
    assert_eq!(st_r, StatusCode::OK);
    assert!(tfa_ok["token"].as_str().is_some());

    let (st, init_qr) = api(&app, Method::POST, "/api/v1/auth/pair/init", None, Some(json!({ "device_name": "QRMac" }))).await;
    assert_eq!(st, StatusCode::OK);
    let ptok = init_qr["pair_token"].as_str().unwrap().to_string();
    let (st, _) = api(&app, Method::POST, "/api/v1/auth/pair/accept", Some(&alice_tok), Some(json!({ "pair_token": ptok }))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, polled) = api(&app, Method::GET, &format!("/api/v1/auth/pair/poll/{ptok}"), None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(polled["status"], "confirmed");
    let (st, pv) = api(&app, Method::POST, "/api/v1/auth/pair/verify", None, Some(json!({
        "pair_token": ptok, "cloud_password": "CloudPassword!9"
    }))).await;
    assert_eq!(st, StatusCode::OK);
    let qr_tok = pv["token"].as_str().unwrap();
    let (st, me) = api(&app, Method::GET, "/api/v1/users/me", Some(qr_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me["username"], "alice_2fa");

    let (st, _) = api(&app, Method::POST, "/api/v1/auth/2fa/disable", Some(&alice_tok), Some(json!({ "current_password": "CloudPassword!9" }))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, sess) = api(&app, Method::GET, "/api/v1/auth/sessions", Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let list = sess["sessions"].as_array().unwrap();
    assert!(list.len() >= 2);
    let other_id = list.iter().find(|s| s["is_current"] == false).unwrap()["session_id"].as_str().unwrap();
    let (st, rev) = api(&app, Method::DELETE, &format!("/api/v1/auth/sessions/{other_id}"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(rev["revoked"], true);
    let (st, _) = api(&app, Method::DELETE, "/api/v1/auth/sessions/others", Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn test_profile_contacts_invites_folders_settings_pins_and_drafts() {
    let dir = TempDirGuard::new("tg_parity_chats");
    let (_, app) = durable_app(&dir);
    let (alice_id, alice_tok) = reg(&app, "alice_prof", "Pass1234!").await;
    let (bob_id, bob_tok) = reg(&app, "bob_prof", "Pass1234!").await;

    let (st, up) = api(&app, Method::PUT, "/api/v1/users/me", Some(&alice_tok), Some(json!({
        "first_name": "Alice", "last_name": "Durov", "bio": "GPUI engineer", "username": "alice_updated", "emoji_status": "rocket"
    }))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(up["bio"], "GPUI engineer");
    assert_eq!(up["emoji_status"], "rocket");
    let (st, u_id) = api(&app, Method::GET, &format!("/api/v1/users/{alice_id}"), Some(&bob_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(u_id["username"], "alice_updated");
    let (st, res_p) = api(&app, Method::GET, "/api/v1/peers/resolve?username=@alice_updated", Some(&bob_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(res_p["user"]["user_id"], alice_id);

    let (st, _) = api(&app, Method::POST, "/api/v1/contacts", Some(&alice_tok), Some(json!({ "username": "bob_prof", "first_name": "Bob" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, cl) = api(&app, Method::GET, "/api/v1/contacts", Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(!cl["contacts"].as_array().unwrap().is_empty());
    let (st, _) = api(&app, Method::DELETE, &format!("/api/v1/contacts/{bob_id}"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);

    let (st, grp) = api(&app, Method::POST, "/api/v1/chats", Some(&alice_tok), Some(json!({ "title": "Core Team", "chat_type": "group" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let cid = grp["chat_id"].as_i64().unwrap();
    let (st, inv) = api(&app, Method::POST, &format!("/api/v1/chats/{cid}/invites"), Some(&alice_tok), Some(json!({ "title": "Main Invite" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let inv_id = inv["invite_id"].as_str().unwrap().to_string();
    let code = inv["code"].as_str().unwrap().to_string();
    let (st, joined) = api(&app, Method::POST, "/api/v1/chats/join", Some(&bob_tok), Some(json!({ "code": code }))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(joined["chat_id"], cid);
    let (st, _) = api(&app, Method::DELETE, &format!("/api/v1/chats/{cid}/invites/{inv_id}"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);

    let (st, fold) = api(&app, Method::POST, "/api/v1/folders", Some(&alice_tok), Some(json!({
        "title": "Work", "include_chats": [cid], "emoticon": "briefcase"
    }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let fid = fold["id"].as_i64().unwrap();
    let (st, fl) = api(&app, Method::GET, "/api/v1/folders", Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(fl["folders"].as_array().unwrap().len(), 1);
    let (st, _) = api(&app, Method::PUT, &format!("/api/v1/folders/{fid}"), Some(&alice_tok), Some(json!({ "title": "Engineering", "include_chats": [cid] }))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = api(&app, Method::DELETE, &format!("/api/v1/folders/{fid}"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);

    for (p, k, val) in [("archive", "archived", true), ("pin", "pinned", true), ("mute", "muted", true)] {
        let (st, _) = api(&app, Method::POST, &format!("/api/v1/chats/{cid}/{p}"), Some(&alice_tok), Some(json!({ k: val }))).await;
        assert_eq!(st, StatusCode::OK);
    }
    let (st, sent) = api(&app, Method::POST, &format!("/api/v1/chats/{cid}/messages"), Some(&alice_tok), Some(json!({ "text": "Pin this notice" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let mid = sent["message_id"].as_i64().unwrap();
    let (st, pins) = api(&app, Method::POST, &format!("/api/v1/chats/{cid}/pins"), Some(&alice_tok), Some(json!({ "message_id": mid }))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(pins["pinned_message_ids"], json!([mid]));
    let (st, _) = api(&app, Method::DELETE, &format!("/api/v1/chats/{cid}/pins?message_id={mid}"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);

    let (st, _) = api(&app, Method::PUT, &format!("/api/v1/chats/{cid}/draft"), Some(&alice_tok), Some(json!({ "text": "WIP reply" }))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, dr) = api(&app, Method::GET, &format!("/api/v1/chats/{cid}/draft"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(dr["draft"]["text"], "WIP reply");
    let (st, _) = api(&app, Method::DELETE, &format!("/api/v1/chats/{cid}/draft"), Some(&alice_tok), None).await;
    assert_eq!(st, StatusCode::OK);
}
