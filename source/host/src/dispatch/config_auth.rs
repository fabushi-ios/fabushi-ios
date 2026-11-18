use crate::desktop_helpers::{compare_semver, translate_key, AppLanguage};
use crate::error::{ok_envelope, BridgeError};
use crate::host::TelegramAppHost;
use gpui_telegram_client::config::ClientConfig;
use gpui_telegram_client::{
    AuthResponse, ClientProxyConfig, EmailRegisterRequest, LoginRequest, PhoneVerifyRequest,
    RegisterRequest, TelegramClient, TwoFactorSetRequest, TwoFactorVerifyRequest,
};
use gpui_telegram_proto::sync::SyncGetDifferenceRequest;
use reqwest::Method;
use serde_json::{json, Value};
use std::cmp::Ordering as CmpOrdering;
use std::sync::Arc;

pub fn handle(host: &TelegramAppHost, cmd: &str, params: &Value) -> Option<String> {
    let client = host.current_client();
    match cmd {
        // ====================================================================
        // Namespace 1: `config.*` (+ `telegram.ping` / `telegram.proto.*`)
        // ====================================================================
        "config.ping" | "telegram.ping" | "ping" => {
            let nonce = params.get("nonce").and_then(Value::as_u64).unwrap_or(0);
            Some(ok_envelope(json!({
                "pong": true,
                "nonce": nonce,
                "timestamp": chrono::Utc::now().timestamp_millis(),
                "version": env!("CARGO_PKG_VERSION"),
                "server_url": host.server_url(),
                "is_test_mode": host.is_test_mode
            })))
        }

        "telegram.proto.getConfig" | "proto.getConfig" | "telegram.proto.maxPacketSize" => {
            Some(ok_envelope(json!({
                "max_packet_size": 16 * 1024 * 1024,
                "current_layer": 188,
                "min_layer": 18
            })))
        }

        "config.get" | "config.get_server_config" => {
            let cfg = host.host_config.lock().clone();
            Some(ok_envelope(json!({
                "server_url": cfg.server_url,
                "language": cfg.preferences.language,
                "locale": cfg.preferences.language,
                "preferences": cfg.preferences,
                "proxy": cfg.proxy,
                "simulate_offline": *host.simulate_offline.lock(),
                "device_id": client.device_id(),
                "version": env!("CARGO_PKG_VERSION")
            })))
        }

        "config.set_server_url" | "config.set_server_config" => {
            let Some(raw_url) = params
                .get("server_url")
                .or_else(|| params.get("url"))
                .and_then(Value::as_str)
            else {
                return Some(BridgeError::invalid_params("Missing server_url").to_envelope_string());
            };
            if raw_url.trim().is_empty() {
                return Some(
                    BridgeError::invalid_params("server_url cannot be empty").to_envelope_string(),
                );
            }
            let normalized = ClientConfig::normalize_url(raw_url);
            let prev_url = {
                let mut cfg = host.host_config.lock();
                let prev = cfg.server_url.clone();
                cfg.server_url = normalized.clone();
                if let Some(loc) = params
                    .get("locale")
                    .or_else(|| params.get("language"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                {
                    cfg.preferences.language = AppLanguage::from_code(loc).code().to_string();
                }
                prev
            };
            if let Err(e) = host.persist_host_config() {
                return Some(e.to_envelope_string());
            }

            if prev_url != normalized {
                client.disconnect_ws();
                client.stop_tailnet_mesh();
                client.clear_session();
                client.local_store.clear_all();

                let mut next_cfg = ClientConfig::default();
                next_cfg.server_url = normalized.clone();
                next_cfg.proxy = host.host_config.lock().proxy.clone();
                let store_dir = host.app_data_dir.join("cache");
                match TelegramClient::open_with_dir(next_cfg, &store_dir) {
                    Ok(new_client) => {
                        if *host.simulate_offline.lock() {
                            new_client.network_listener.set_online(false);
                        }
                        *host.client.write() = Arc::new(new_client);
                    }
                    Err(e) => {
                        return Some(
                            BridgeError::internal(format!(
                                "Failed to rebind TelegramClient to {normalized}: {e}"
                            ))
                            .to_envelope_string(),
                        );
                    }
                }
            }
            let language = host.host_config.lock().preferences.language.clone();
            Some(ok_envelope(json!({
                "server_url": normalized,
                "locale": language,
                "switched": prev_url != normalized
            })))
        }

        "config.set_proxy" => {
            let proxy_opt = match params.get("proxy_url").and_then(Value::as_str) {
                Some(u) if !u.trim().is_empty() => match ClientProxyConfig::from_url(u) {
                    Ok(p) => Some(p),
                    Err(e) => {
                        return Some(
                            BridgeError::invalid_params(format!("Invalid proxy_url: {e}"))
                                .to_envelope_string(),
                        )
                    }
                },
                _ => params
                    .get("proxy")
                    .and_then(|v| serde_json::from_value::<ClientProxyConfig>(v.clone()).ok()),
            };
            if let Err(e) = client.apply_proxy(proxy_opt.clone()) {
                return Some(BridgeError::from_http_error(e).to_envelope_string());
            }
            host.host_config.lock().proxy = proxy_opt.clone();
            if let Err(e) = host.persist_host_config() {
                return Some(e.to_envelope_string());
            }
            Some(ok_envelope(json!({ "proxy": proxy_opt })))
        }

        "config.ping_health" | "config.test_proxy" => {
            let http = client.http_client.clone();
            let start = std::time::Instant::now();
            let res = host.block_on(async move { http.check_health().await });
            let latency_ms = start.elapsed().as_secs_f64() * 1000.0;
            match res {
                Ok(ok) => Some(ok_envelope(json!({
                    "healthy": ok,
                    "latency_ms": latency_ms,
                    "server_url": host.server_url()
                }))),
                Err(err) => Some(ok_envelope(json!({
                    "healthy": false,
                    "latency_ms": latency_ms,
                    "server_url": host.server_url(),
                    "error": err.to_string()
                }))),
            }
        }

        "config.set_simulate_offline" => {
            let offline = params.get("offline").and_then(Value::as_bool).unwrap_or(false);
            *host.simulate_offline.lock() = offline;
            client.network_listener.set_online(!offline);
            let state_str = if offline { "offline" } else { "online" };
            host.events
                .emit("connectionStateChanged", &json!({ "state": state_str }));
            Some(ok_envelope(json!({
                "simulate_offline": offline,
                "state": state_str
            })))
        }

        "config.lifecycle_background" => Some(ok_envelope(host.enter_background())),
        "config.lifecycle_foreground" => Some(ok_envelope(host.resume_foreground())),

        "config.set_language" => {
            let code = params
                .get("language")
                .or_else(|| params.get("locale"))
                .and_then(Value::as_str)
                .unwrap_or("en-US");
            let lang = AppLanguage::from_code(code);
            host.host_config.lock().preferences.language = lang.code().to_string();
            let _ = host.persist_host_config();
            host.events
                .emit("languageChanged", &json!({ "language": lang.code() }));
            Some(ok_envelope(json!({
                "language": lang.code(),
                "sample_tabs": {
                    "contacts": translate_key(lang, "tab.contacts"),
                    "calls": translate_key(lang, "tab.calls"),
                    "chats": translate_key(lang, "tab.chats"),
                    "settings": translate_key(lang, "tab.settings"),
                }
            })))
        }

        "config.preferences_get" => {
            let prefs = host.host_config.lock().preferences.clone();
            Some(ok_envelope(serde_json::to_value(prefs).unwrap_or_default()))
        }

        "config.preferences_set" => {
            {
                let mut cfg = host.host_config.lock();
                if let Some(mode) = params.get("theme_mode").and_then(Value::as_str) {
                    cfg.preferences.theme_mode = mode.to_string();
                }
                if let Some(acc) = params.get("accent_hex").and_then(Value::as_str) {
                    cfg.preferences.accent_hex = acc.to_string();
                }
                if let Some(fsz) = params.get("font_size_pt").and_then(Value::as_u64) {
                    cfg.preferences.font_size_pt = (fsz as u8).clamp(13, 23);
                }
                if let Some(bf) = params.get("bubble_free_mode").and_then(Value::as_bool) {
                    cfg.preferences.bubble_free_mode = bf;
                }
                if let Some(lg) = params.get("liquid_glass_enabled").and_then(Value::as_bool) {
                    cfg.preferences.liquid_glass_enabled = lg;
                }
                if let Some(sce) = params.get("send_by_cmd_enter").and_then(Value::as_bool) {
                    cfg.preferences.send_by_cmd_enter = sce;
                }
                if let Some(lang) = params.get("language").and_then(Value::as_str) {
                    cfg.preferences.language = AppLanguage::from_code(lang).code().to_string();
                }
            }
            if let Err(e) = host.persist_host_config() {
                return Some(e.to_envelope_string());
            }
            let prefs = host.host_config.lock().preferences.clone();
            Some(ok_envelope(serde_json::to_value(prefs).unwrap_or_default()))
        }

        "config.check_release_update" => {
            let current = env!("CARGO_PKG_VERSION");
            let latest = params
                .get("candidate_version")
                .and_then(Value::as_str)
                .unwrap_or(current);
            let update_available = compare_semver(current, latest) == CmpOrdering::Less;
            Some(ok_envelope(json!({
                "current_version": current,
                "latest_version": latest,
                "update_available": update_available,
                "channel": "ios-testflight"
            })))
        }

        // ====================================================================
        // Namespace 2: `auth.*` (All 7 Cloud Auth Flows + Passkeys + 2FA + Sessions)
        // ====================================================================
        "auth.login" | "auth.login_password" => {
            let Some(username) = params.get("username").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing username").to_envelope_string());
            };
            let Some(password) = params.get("password").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing password").to_envelope_string());
            };
            let req = LoginRequest {
                username: username.to_string(),
                password: password.to_string(),
            };
            let c = Arc::clone(&client);
            match host.block_on(async move { c.login(&req).await }) {
                Ok(auth) => {
                    host.record_authenticated_session(&auth);
                    Some(ok_envelope(serde_json::to_value(auth).unwrap_or_default()))
                }
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "auth.register" => {
            let Some(username) = params.get("username").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing username").to_envelope_string());
            };
            let Some(password) = params.get("password").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing password").to_envelope_string());
            };
            let first_name = params
                .get("first_name")
                .and_then(Value::as_str)
                .unwrap_or(username)
                .to_string();
            let req = RegisterRequest {
                username: username.to_string(),
                password: password.to_string(),
                first_name,
            };
            let c = Arc::clone(&client);
            match host.block_on(async move { c.register(&req).await }) {
                Ok(auth) => {
                    host.record_authenticated_session(&auth);
                    Some(ok_envelope(serde_json::to_value(auth).unwrap_or_default()))
                }
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "auth.auto_welcome" => {
            let suffix = uuid::Uuid::new_v4().simple().to_string();
            let username = format!("ios_guest_{}", &suffix[..8]);
            let password = format!("Pw!{}", &suffix[8..24]);
            let req = RegisterRequest {
                username: username.clone(),
                password,
                first_name: "Fabushi Explorer".to_string(),
            };
            let c = Arc::clone(&client);
            match host.block_on(async move {
                let auth = c.register(&req).await?;
                let _ = c.provision_welcome_chat().await;
                Ok::<AuthResponse, _>(auth)
            }) {
                Ok(auth) => {
                    host.record_authenticated_session(&auth);
                    Some(ok_envelope(serde_json::to_value(auth).unwrap_or_default()))
                }
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "auth.session_status" | "auth.status" => {
            let sess = client.current_auth_session();
            Some(ok_envelope(json!({
                "authenticated": sess.is_some(),
                "session": sess,
                "server_url": host.server_url(),
                "generation": host.generation_fence.lock().current()
            })))
        }

        "auth.hydrate_session" => {
            let Ok(auth) = serde_json::from_value::<AuthResponse>(params.clone()) else {
                return Some(
                    BridgeError::invalid_params("Invalid AuthResponse payload")
                        .to_envelope_string(),
                );
            };
            client.set_auth_session(auth.clone());
            host.record_authenticated_session(&auth);
            Some(ok_envelope(json!({
                "hydrated": true,
                "user_id": auth.user_id,
                "username": auth.username
            })))
        }

        "auth.me" => match host.api_json(Method::GET, "/api/v1/users/me", None, true) {
            Ok(v) => Some(ok_envelope(v)),
            Err(e) => Some(e.to_envelope_string()),
        },

        "auth.logout" => {
            let uid = client.current_user_id();
            let c = Arc::clone(&client);
            let _ = host.block_on(async move { c.logout().await });
            client.clear_session();
            if let Some(id) = uid {
                let mut acc = host.accounts.lock();
                acc.accounts.remove(&id);
                acc.active_user_id = acc.accounts.keys().next().copied();
                let _ = host.persist_accounts();
            }
            host.generation_fence.lock().bump();
            host.events
                .emit("authStateChanged", &json!({ "authenticated": false }));
            Some(ok_envelope(json!({ "logged_out": true })))
        }

        "auth.phone_send_code" => {
            let Some(phone) = params.get("phone").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing phone").to_envelope_string());
            };
            let phone_s = phone.to_string();
            let c = Arc::clone(&client);
            match host.block_on(async move { c.phone_send_code(&phone_s).await }) {
                Ok(r) => Some(ok_envelope(serde_json::to_value(r).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "auth.phone_verify_code" => {
            let Ok(req) = serde_json::from_value::<PhoneVerifyRequest>(params.clone()) else {
                return Some(
                    BridgeError::invalid_params("Invalid PhoneVerifyRequest").to_envelope_string(),
                );
            };
            let c = Arc::clone(&client);
            match host.block_on(async move { c.phone_verify(&req).await }) {
                Ok(auth) => {
                    host.record_authenticated_session(&auth);
                    Some(ok_envelope(serde_json::to_value(auth).unwrap_or_default()))
                }
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "auth.email_send_code" => {
            let Some(email) = params.get("email").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing email").to_envelope_string());
            };
            let email_s = email.to_string();
            let c = Arc::clone(&client);
            match host.block_on(async move { c.email_send_code(&email_s).await }) {
                Ok(r) => Some(ok_envelope(serde_json::to_value(r).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "auth.email_verify_code" => {
            let Some(email) = params.get("email").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing email").to_envelope_string());
            };
            let Some(code) = params.get("code").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing code").to_envelope_string());
            };
            let email_s = email.to_string();
            let code_s = code.to_string();
            let c = Arc::clone(&client);
            match host.block_on(async move { c.email_verify(&email_s, &code_s).await }) {
                Ok(auth) => {
                    host.record_authenticated_session(&auth);
                    Some(ok_envelope(serde_json::to_value(auth).unwrap_or_default()))
                }
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "auth.email_register" => {
            let Ok(req) = serde_json::from_value::<EmailRegisterRequest>(params.clone()) else {
                return Some(
                    BridgeError::invalid_params("Invalid EmailRegisterRequest")
                        .to_envelope_string(),
                );
            };
            let c = Arc::clone(&client);
            match host.block_on(async move { c.email_register(&req).await }) {
                Ok(auth) => {
                    host.record_authenticated_session(&auth);
                    Some(ok_envelope(serde_json::to_value(auth).unwrap_or_default()))
                }
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "auth.alipay_qr_create" | "auth.alipay_oauth_url" => {
            match host.api_json(Method::POST, "/api/v1/auth/alipay/start", Some(params.clone()), false) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "auth.alipay_qr_status" => {
            let Some(attempt_id) = params.get("attempt_id").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing attempt_id").to_envelope_string());
            };
            match host.api_json(
                Method::GET,
                &format!("/api/v1/auth/alipay/poll/{attempt_id}"),
                None,
                false,
            ) {
                Ok(v) => {
                    if let Some(sess_val) = v.get("session") {
                        if let Ok(auth) = serde_json::from_value::<AuthResponse>(sess_val.clone()) {
                            client.set_auth_session(auth.clone());
                            host.record_authenticated_session(&auth);
                        }
                    }
                    Some(ok_envelope(v))
                }
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "auth.alipay_exchange" => {
            match host.api_json(Method::POST, "/api/v1/auth/alipay/callback", Some(params.clone()), false) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "auth.qr_pair_create" => {
            let c = Arc::clone(&client);
            match host.block_on(async move { c.qr_pair_init().await }) {
                Ok(r) => Some(ok_envelope(serde_json::to_value(r).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "auth.qr_pair_status" => {
            let Some(pair_token) = params.get("pair_token").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing pair_token").to_envelope_string());
            };
            let tok_s = pair_token.to_string();
            let c = Arc::clone(&client);
            match host.block_on(async move { c.qr_pair_poll(&tok_s).await }) {
                Ok(r) => {
                    if let Some(auth) = client.current_auth_session() {
                        host.record_authenticated_session(&auth);
                    }
                    Some(ok_envelope(serde_json::to_value(r).unwrap_or_default()))
                }
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "auth.qr_pair_confirm" => {
            let Some(pair_token) = params.get("pair_token").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing pair_token").to_envelope_string());
            };
            let tok_s = pair_token.to_string();
            let c = Arc::clone(&client);
            match host.block_on(async move { c.qr_pair_accept(&tok_s).await }) {
                Ok(()) => Some(ok_envelope(json!({ "accepted": true }))),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "auth.setup_2fa" => {
            let Ok(req) = serde_json::from_value::<TwoFactorSetRequest>(params.clone()) else {
                return Some(
                    BridgeError::invalid_params("Invalid TwoFactorSetRequest").to_envelope_string(),
                );
            };
            let c = Arc::clone(&client);
            match host.block_on(async move { c.set_2fa(&req).await }) {
                Ok(r) => Some(ok_envelope(serde_json::to_value(r).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "auth.verify_2fa" => {
            let Ok(req) = serde_json::from_value::<TwoFactorVerifyRequest>(params.clone()) else {
                return Some(
                    BridgeError::invalid_params("Invalid TwoFactorVerifyRequest")
                        .to_envelope_string(),
                );
            };
            let c = Arc::clone(&client);
            match host.block_on(async move { c.verify_2fa(&req).await }) {
                Ok(r) => Some(ok_envelope(serde_json::to_value(r).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "auth.disable_2fa" => {
            let pw = params
                .get("current_password")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let c = Arc::clone(&client);
            match host.block_on(async move { c.disable_2fa(pw.as_deref()).await }) {
                Ok(()) => Some(ok_envelope(json!({ "disabled": true }))),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "auth.passkey_register_begin" => {
            match host.api_json(Method::POST, "/api/v1/auth/passkey/register/start", Some(params.clone()), true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }
        "auth.passkey_register_finish" => {
            match host.api_json(Method::POST, "/api/v1/auth/passkey/register/finish", Some(params.clone()), true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }
        "auth.passkey_login_begin" => {
            match host.api_json(Method::POST, "/api/v1/auth/passkey/login/start", Some(params.clone()), false) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }
        "auth.passkey_login_finish" => {
            match host.api_json(Method::POST, "/api/v1/auth/passkey/login/finish", Some(params.clone()), false) {
                Ok(v) => {
                    if let Ok(auth) = serde_json::from_value::<AuthResponse>(v.clone()) {
                        client.set_auth_session(auth.clone());
                        host.record_authenticated_session(&auth);
                    }
                    Some(ok_envelope(v))
                }
                Err(e) => Some(e.to_envelope_string()),
            }
        }
        "auth.passkey_list" => {
            match host.api_json(Method::GET, "/api/v1/auth/passkeys", None, true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }
        "auth.passkey_delete" => {
            let Some(cid) = params.get("credential_id").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing credential_id").to_envelope_string());
            };
            match host.api_json(Method::DELETE, &format!("/api/v1/auth/passkeys/{cid}"), None, true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "auth.sessions_list" => {
            match host.api_json(Method::GET, "/api/v1/auth/sessions", None, true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }
        "auth.session_revoke" => {
            let Some(sid) = params.get("session_id").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing session_id").to_envelope_string());
            };
            match host.api_json(Method::DELETE, &format!("/api/v1/auth/sessions/{sid}"), None, true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }
        "auth.sessions_revoke_others" => {
            match host.api_json(Method::POST, "/api/v1/auth/sessions/revoke-others", Some(json!({})), true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        // ====================================================================
        // Namespace 3: `accounts.*` (+ `telegram.generation.*`)
        // ====================================================================
        "accounts.list" => {
            let store = host.accounts.lock();
            let items: Vec<Value> = store
                .accounts
                .values()
                .map(|a| {
                    json!({
                        "user_id": a.user_id,
                        "username": a.username,
                        "server_url": a.server_url,
                        "session_id": a.session_id,
                        "is_active": store.active_user_id == Some(a.user_id)
                    })
                })
                .collect();
            Some(ok_envelope(json!({
                "active_user_id": store.active_user_id,
                "accounts": items,
                "max_accounts": 6
            })))
        }

        "accounts.switch" => {
            let Some(user_id) = params.get("user_id").and_then(Value::as_u64) else {
                return Some(BridgeError::invalid_params("Missing user_id").to_envelope_string());
            };
            let target_sess = {
                let mut store = host.accounts.lock();
                let Some(sess) = store.accounts.get(&user_id).cloned() else {
                    return Some(
                        BridgeError::not_found(format!("Account {user_id} not found"))
                            .to_envelope_string(),
                    );
                };
                store.active_user_id = Some(user_id);
                sess
            };
            let _ = host.persist_accounts();
            let gen = host.generation_fence.lock().bump();
            client.set_auth_session(AuthResponse {
                user_id: target_sess.user_id,
                username: target_sess.username.clone(),
                session_id: target_sess.session_id.clone(),
                token: target_sess.token.clone(),
                first_name: target_sess.username.clone(),
                created_at: target_sess.created_at_ms,
                expires_at: target_sess.created_at_ms + 30 * 86_400_000,
            });
            host.events.emit(
                "accountSwitched",
                &json!({ "user_id": user_id, "generation": gen }),
            );
            Some(ok_envelope(json!({
                "active_user_id": user_id,
                "username": target_sess.username,
                "generation": gen
            })))
        }

        "accounts.remove" => {
            let Some(user_id) = params.get("user_id").and_then(Value::as_u64) else {
                return Some(BridgeError::invalid_params("Missing user_id").to_envelope_string());
            };
            let active_after = {
                let mut store = host.accounts.lock();
                store.accounts.remove(&user_id);
                if store.active_user_id == Some(user_id) {
                    store.active_user_id = store.accounts.keys().next().copied();
                }
                store.active_user_id
            };
            let _ = host.persist_accounts();
            Some(ok_envelope(json!({
                "removed_user_id": user_id,
                "active_user_id": active_after
            })))
        }

        "telegram.generation.current" => {
            let gen = host.generation_fence.lock().current();
            Some(ok_envelope(json!({ "generation": gen })))
        }

        "telegram.generation.bump" => {
            let gen = host.generation_fence.lock().bump();
            Some(ok_envelope(json!({ "generation": gen })))
        }

        // ====================================================================
        // Namespace 4: `sync.*` (PTS Gap-Healing & WebSocket Status)
        // ====================================================================
        "sync.get_state" | "sync.ws_status" => {
            let ws_status = format!("{:?}", client.status());
            let gap_chats: Vec<i64> = client.pending_gap_chats().into_iter().map(|c| c.0).collect();
            Some(ok_envelope(json!({
                "ws_connected": client.is_connected(),
                "ws_status": ws_status,
                "gap_chats": gap_chats,
                "local_pts": host.next_pts.load(std::sync::atomic::Ordering::Relaxed)
            })))
        }

        "sync.reconnect" => {
            client.connect_ws();
            Some(ok_envelope(json!({
                "reconnecting": true,
                "ws_status": format!("{:?}", client.status())
            })))
        }

        "sync.get_difference" | "telegram.syncDifference" | "sync.getDifference" => {
            let pts = params.get("pts").and_then(Value::as_u64).unwrap_or(0);
            let chat_id = params.get("chat_id").and_then(Value::as_i64);
            if let Some(token) = client.token() {
                let http = client.http_client.clone();
                let req = SyncGetDifferenceRequest {
                    pts,
                    limit: params.get("limit").and_then(Value::as_u64).map(|n| n as usize),
                    chat_id: chat_id.map(gpui_telegram_core::types::ChatId),
                    filter: None,
                };
                if let Ok(diff) = host.block_on(async move { http.sync_get_difference(&token, &req).await }) {
                    host.events.emit(
                        "syncDifference",
                        &json!({ "pts": diff.new_pts, "has_more": diff.has_more }),
                    );
                    return Some(ok_envelope(serde_json::to_value(diff).unwrap_or_default()));
                }
            }
            let next_pts = pts.saturating_add(1);
            host.next_pts
                .store(next_pts, std::sync::atomic::Ordering::Relaxed);
            host.events.emit(
                "syncDifference",
                &json!({ "pts": next_pts, "has_more": false }),
            );
            Some(ok_envelope(json!({
                "pts": next_pts,
                "new_pts": next_pts,
                "pts_count": 0,
                "has_more": false,
                "updates": []
            })))
        }

        _ => None,
    }
}
