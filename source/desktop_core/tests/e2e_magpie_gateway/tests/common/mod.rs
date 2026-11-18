#![allow(dead_code, unused_imports)]

pub mod mock_oauth;
pub mod mock_providers;

pub use mock_oauth::{spawn_mock_oauth, MockOAuthForm, MockOAuthState};
pub use mock_providers::{spawn_mock_providers, MockProviderState};

use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    Router,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use gpui_telegram_server::{
    build_router_with_state, config::ServerConfig, server::AppState,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

/// Create a test RAII temporary directory under `std::env::temp_dir()`.
pub fn new_temp_dir(prefix: &str) -> TempDir {
    tempfile::Builder::new()
        .prefix(&format!("e2e_magpie_{prefix}_"))
        .tempdir_in(std::env::temp_dir())
        .expect("create temp dir")
}

/// Generate a mock JWT string with standard OpenAI auth claims.
pub fn create_mock_jwt(
    email: &str,
    plan: &str,
    account_id: &str,
    exp_secs_from_now: i64,
) -> String {
    let header = json!({ "alg": "RS256", "typ": "JWT" });
    let now = chrono::Utc::now().timestamp();
    let payload = json!({
        "email": email,
        "exp": now + exp_secs_from_now,
        "iat": now,
        "https://api.openai.com/auth": {
            "chatgpt_account_id": account_id,
            "chatgpt_plan_type": plan,
            "user_id": format!("user_{account_id}")
        }
    });

    let h_b64 = URL_SAFE_NO_PAD.encode(header.to_string());
    let p_b64 = URL_SAFE_NO_PAD.encode(payload.to_string());
    format!("{h_b64}.{p_b64}.mock_sig_part_3")
}

/// Create a mock Google Vertex AI Application Default Credentials file in `dir`.
pub fn create_mock_vertex_adc_dir(
    dir: &Path,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
    quota_project_id: Option<&str>,
    account: Option<&str>,
) -> PathBuf {
    let gcloud_dir = dir.join(".config").join("gcloud");
    std::fs::create_dir_all(&gcloud_dir).expect("create gcloud dir");

    let adc_file = gcloud_dir.join("application_default_credentials.json");
    let mut val = json!({
        "client_id": client_id,
        "client_secret": client_secret,
        "refresh_token": refresh_token,
        "type": "authorized_user"
    });
    if let Some(qp) = quota_project_id {
        val["quota_project_id"] = json!(qp);
    }
    if let Some(acct) = account {
        val["account"] = json!(acct);
    }
    std::fs::write(&adc_file, serde_json::to_string_pretty(&val).unwrap())
        .expect("write adc file");

    if quota_project_id.is_some() || account.is_some() {
        let conf_dir = gcloud_dir.join("configurations");
        std::fs::create_dir_all(&conf_dir).expect("create configurations dir");
        let conf_file = conf_dir.join("config_default");
        let content = format!(
            "[core]\nproject = {}\naccount = {}\n",
            quota_project_id.unwrap_or("test-quota-proj"),
            account.unwrap_or("test-user@example.com")
        );
        std::fs::write(conf_file, content).expect("write config_default");
    }

    adc_file
}

/// Create mock Codex `auth.json` and `config.toml` in `dir/.codex`.
pub fn create_mock_codex_auth_dir(
    dir: &Path,
    email: &str,
    plan: &str,
    access_token: &str,
    account_id: &str,
) -> PathBuf {
    let codex_dir = dir.join(".codex");
    std::fs::create_dir_all(&codex_dir).expect("create codex dir");

    let auth_file = codex_dir.join("auth.json");
    let val = json!({
        "tokens": {
            "access_token": access_token,
            "account_id": account_id,
            "id_token": access_token
        },
        "email": email,
        "plan": plan
    });
    std::fs::write(&auth_file, serde_json::to_string_pretty(&val).unwrap())
        .expect("write auth.json");

    let cfg_file = codex_dir.join("config.toml");
    let cfg_content = format!("email = \"{email}\"\nplan = \"{plan}\"\n");
    std::fs::write(cfg_file, cfg_content).expect("write config.toml");

    auth_file
}

/// Create mock Claude Code settings and credentials in `dir/.claude`.
pub fn create_mock_claude_dir(
    dir: &Path,
    token: &str,
    email: Option<&str>,
    plan: Option<&str>,
    model: Option<&str>,
) -> PathBuf {
    let claude_dir = dir.join(".claude");
    std::fs::create_dir_all(&claude_dir).expect("create claude dir");

    let settings_file = claude_dir.join("settings.json");
    let s_val = json!({
        "env": {
            "ANTHROPIC_AUTH_TOKEN": token,
            "ANTHROPIC_BASE_URL": "https://api.anthropic.com/v1"
        },
        "model": model.unwrap_or("claude-3-5-sonnet-20241022")
    });
    std::fs::write(&settings_file, serde_json::to_string_pretty(&s_val).unwrap())
        .expect("write settings.json");

    let creds_file = claude_dir.join(".credentials.json");
    let mut c_val = json!({
        "claudeAiOauth": {
            "accessToken": token,
            "subscriptionType": plan.unwrap_or("team"),
            "rateLimitTier": "default",
            "expiresAt": (chrono::Utc::now().timestamp() + 3600) * 1000
        }
    });
    if let Some(em) = email {
        c_val["oauthAccount"] = json!({ "emailAddress": em });
    }
    std::fs::write(&creds_file, serde_json::to_string_pretty(&c_val).unwrap())
        .expect("write .credentials.json");

    settings_file
}

/// Create mock Gemini CLI / Code Assist files in `dir/.gemini`.
pub fn create_mock_gemini_dir(
    dir: &Path,
    token: &str,
    email: Option<&str>,
    refresh_token: Option<&str>,
) -> PathBuf {
    let gemini_dir = dir.join(".gemini");
    std::fs::create_dir_all(&gemini_dir).expect("create gemini dir");

    let jetski_file = gemini_dir.join("jetski-standalone-oauth-token");
    let j_val = json!({
        "id_token_email": email.unwrap_or("gemini-dev@example.com"),
        "auth_method": "consumer",
        "token": {
            "access_token": token,
            "refresh_token": refresh_token.unwrap_or("mock_refresh_token_gemini"),
            "expires_in": 3600
        }
    });
    std::fs::write(&jetski_file, serde_json::to_string_pretty(&j_val).unwrap())
        .expect("write jetski token");

    let oauth_file = gemini_dir.join("oauth_creds.json");
    let o_val = json!({
        "access_token": token,
        "refresh_token": refresh_token.unwrap_or("mock_refresh_token_gemini"),
        "email": email.unwrap_or("gemini-dev@example.com"),
        "expiry_date": (chrono::Utc::now().timestamp() + 3600) * 1000
    });
    std::fs::write(&oauth_file, serde_json::to_string_pretty(&o_val).unwrap())
        .expect("write oauth_creds.json");

    jetski_file
}

/// Create mock Magpie configs in `dir/.config/magpie`.
pub fn create_mock_magpie_dir(
    dir: &Path,
    providers_val: Value,
    logins_val: Value,
) -> (PathBuf, PathBuf) {
    let magpie_dir = dir.join(".config").join("magpie");
    std::fs::create_dir_all(&magpie_dir).expect("create magpie config dir");

    let prov_file = magpie_dir.join("providers.json");
    std::fs::write(&prov_file, serde_json::to_string_pretty(&providers_val).unwrap())
        .expect("write providers.json");

    let logins_file = magpie_dir.join("logins.json");
    std::fs::write(&logins_file, serde_json::to_string_pretty(&logins_val).unwrap())
        .expect("write logins.json");

    (prov_file, logins_file)
}

// --- Test Server Boot Helpers ---

pub fn build_temp_server_config(dir: &Path) -> ServerConfig {
    let db_path = dir.join("magpie_gateway_e2e.db");
    let media_dir = dir.join("media_cas");
    std::fs::create_dir_all(&media_dir).expect("create media dir");
    let mut cfg = ServerConfig::default();
    cfg.database_url = format!("sqlite://{}", db_path.display());
    cfg.media_storage_path = media_dir.to_string_lossy().to_string();
    cfg.host = "127.0.0.1".to_string();
    cfg.port = 0;
    cfg
}

pub fn boot_test_server_in(dir: &Path) -> (Arc<AppState>, Router) {
    let cfg = build_temp_server_config(dir);
    let state = Arc::new(AppState::try_new(cfg).expect("boot AppState"));
    let router = build_router_with_state(state.clone());
    (state, router)
}

pub fn boot_test_server() -> (TempDir, Arc<AppState>, Router) {
    let dir = new_temp_dir("server");
    let (state, router) = boot_test_server_in(dir.path());
    (dir, state, router)
}

pub fn isolate_empty_providers(state: &AppState, empty_home: &Path) {
    let providers = state.hermes.providers();
    for p in providers.list_providers() {
        if p.id != "local_hermes" {
            providers.remove_provider(&p.id);
        }
    }
    providers.refresh_local_subscriptions_in_home(empty_home, Some(empty_home));
    providers.set_allow_local_fallback(false);
    state.hermes.persist_providers_state();
    state.fabushi_bots.sync_all_providers();
}

pub async fn http_raw(
    router: &Router,
    method: &str,
    uri: &str,
    bearer: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, String, Vec<u8>) {
    let mut builder = Request::builder()
        .method(Method::from_bytes(method.as_bytes()).expect("valid method"))
        .uri(uri);
    if let Some(tok) = bearer {
        builder = builder.header("authorization", format!("Bearer {tok}"));
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

pub async fn http_json(
    router: &Router,
    method: &str,
    uri: &str,
    bearer: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, _ct, bytes) = http_raw(router, method, uri, bearer, body).await;
    let val = serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|_| {
        json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() })
    });
    (status, val)
}

pub async fn register_user(router: &Router, username: &str) -> (u64, String) {
    let (st, val) = http_json(
        router,
        "POST",
        "/api/v1/auth/register",
        None,
        Some(json!({
            "username": username,
            "password": "StrongPassword#2026",
            "first_name": username,
            "device_name": "magpie_e2e_test",
            "auto_welcome": true
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "register {username}: {val}");
    let uid = val["user_id"].as_u64().expect("user_id");
    let tok = val["token"].as_str().expect("token").to_string();
    (uid, tok)
}
