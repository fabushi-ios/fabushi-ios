use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use futures::{stream, StreamExt};
use gpui_telegram_agent::hermes::providers::{
    ApiMode, CredentialEntry, CredentialPool, MagpiePresetApplyInput, ModelProfile, ProviderChatMessage,
    ProviderConfig, ProviderError, ProviderMessageRole, ProviderRequest, ProviderResponse,
    ProviderRouter, ProviderRouterState, ProviderStreamEvent, ProviderToolCall, ProviderToolSpec,
    SubscriptionAccount, SubscriptionKind,
};
use gpui_telegram_core::types::ChatId;
use magpie_gateway::{
    all_presets, inbound_anthropic_to_unified, inbound_chat_completions_to_unified,
    inbound_responses_to_unified, parse_dotenv_providers, parse_magpie_import_link,
    unified_to_anthropic_json, unified_to_chat_completions_json, unified_to_responses_json,
    ImportedProviderSpec, MaskingConfig, MaskingEngine, RoutingGroup, RoutingStrategy,
    UnifiedRequest, UnifiedResponse, UnifiedRole, UnifiedToolCall, UnifiedToolSpec, UnifiedUsage,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::routes::{check_active_membership, err_resp};
use crate::server::{AppState, AuthBearer};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MagpieProviderView {
    pub provider_id: String,
    pub name: String,
    pub api_mode: String,
    pub base_url: Option<String>,
    pub default_model: String,
    pub models: Vec<String>,
    pub enabled: bool,
    pub has_api_key: bool,
    pub masked_key: Option<String>,
    pub is_active: bool,
    pub preset_id: Option<String>,
    pub region_id: Option<String>,
    pub is_subscription: bool,
    pub no_key_required: bool,
    pub routing_strategy: Option<String>,
    pub headers: HashMap<String, String>,
}

fn redact_provider_headers(headers: &HashMap<String, String>) -> HashMap<String, String> {
    headers
        .iter()
        .map(|(k, v)| (k.clone(), magpie_gateway::redact_header_value(k, v)))
        .collect()
}

fn sanitize_subscription_account(sub: &SubscriptionAccount) -> SubscriptionAccount {
    let mut sanitized = sub.clone();
    let hdr_map: HashMap<String, String> = sanitized
        .headers
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    sanitized.headers = redact_provider_headers(&hdr_map).into_iter().collect();
    sanitized
}

fn provider_to_magpie_view(cfg: &ProviderConfig, snap: &ProviderRouterState) -> MagpieProviderView {
    let mut models = Vec::new();
    if !cfg.default_model.trim().is_empty() {
        models.push(cfg.default_model.clone());
    }
    for m in &cfg.fallback_models {
        if !models.contains(m) {
            models.push(m.clone());
        }
    }
    for prof in snap.model_profiles.values() {
        if prof.provider_id == cfg.id && !models.contains(&prof.model) {
            models.push(prof.model.clone());
        }
    }
    let masked_key = cfg.credentials.entries.first().map(|e| e.masked_key());
    MagpieProviderView {
        provider_id: cfg.id.clone(),
        name: cfg.name.clone(),
        api_mode: cfg.api_mode.as_str().to_string(),
        base_url: if cfg.base_url.trim().is_empty() {
            None
        } else {
            Some(cfg.base_url.clone())
        },
        default_model: cfg.default_model.clone(),
        models,
        enabled: cfg.enabled,
        has_api_key: !cfg.credentials.is_empty(),
        masked_key,
        is_active: cfg.id == snap.active_provider_id,
        preset_id: cfg.preset_id.clone(),
        region_id: cfg.region_id.clone(),
        is_subscription: cfg.is_subscription,
        no_key_required: cfg.no_key_required,
        routing_strategy: cfg.routing_strategy.map(|s| s.as_str().to_string()),
        headers: redact_provider_headers(&cfg.headers),
    }
}

fn build_magpie_state_json(state: &AppState) -> Value {
    let router = state.hermes.providers();
    let snap = router.snapshot();
    let mut provider_list: Vec<&ProviderConfig> = snap.providers.values().collect();
    provider_list.sort_by(|a, b| a.id.cmp(&b.id));
    let providers: Vec<MagpieProviderView> = provider_list
        .into_iter()
        .map(|cfg| provider_to_magpie_view(cfg, &snap))
        .collect();

    let mut groups: Vec<&RoutingGroup> = snap.routing_groups.values().collect();
    groups.sort_by(|a, b| a.id.cmp(&b.id));
    let subscriptions: Vec<SubscriptionAccount> = snap
        .subscriptions
        .iter()
        .map(sanitize_subscription_account)
        .collect();

    json!({
        "presets": all_presets(),
        "providers": providers,
        "active_provider": snap.active_provider_id,
        "active_model": snap.active_model,
        "allow_local_fallback": snap.allow_local_fallback,
        "has_active_subscription": router.has_active_subscription(),
        "has_configured_provider": router.has_configured_provider(),
        "needs_provider_selection": router.needs_provider_selection(),
        "subscriptions": subscriptions,
        "routing_groups": groups,
        "routing_strategy": snap.routing_strategy,
        "masking": snap.masking,
        "recent_route_traces": snap.recent_route_traces,
    })
}

fn persist_and_sync_all(state: &AppState) {
    state.hermes.persist_providers_state();
    state.fabushi_bots.sync_all_providers();
}

/// GET /api/v1/hermes/magpie/state
pub async fn magpie_state_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    (StatusCode::OK, Json(build_magpie_state_json(&state))).into_response()
}

#[derive(Debug, Deserialize)]
pub struct MagpieUpsertProviderRequest {
    #[serde(default)]
    pub preset_id: Option<String>,
    #[serde(default, alias = "id")]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub region_id: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_mode: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub custom_headers: HashMap<String, String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub no_key_required: Option<bool>,
    #[serde(default = "default_true", alias = "set_active")]
    pub activate: bool,
    #[serde(default)]
    pub allow_local_fallback: Option<bool>,
}

fn default_true() -> bool {
    true
}

fn apply_custom_or_existing_provider(
    router: &ProviderRouter,
    req: &MagpieUpsertProviderRequest,
    path_id_override: Option<&str>,
) -> Result<ProviderConfig, Response> {
    let pid = path_id_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            req.provider_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("")
        .to_string();
    if pid.is_empty() {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_provider_id",
            "preset_id or provider_id is required",
        ));
    }

    let snap_before = router.snapshot();
    let mut cfg = snap_before
        .providers
        .get(&pid)
        .cloned()
        .unwrap_or_else(|| ProviderConfig {
            id: pid.clone(),
            name: pid.clone(),
            api_mode: ApiMode::ChatCompletions,
            base_url: "https://api.openai.com/v1".to_string(),
            default_model: "gpt-4o-mini".to_string(),
            ..Default::default()
        });

    if let Some(name) = req.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        cfg.name = name.to_string();
    }
    if let Some(raw_mode) = req.api_mode.as_deref() {
        let Some(mode) = ApiMode::parse(raw_mode) else {
            return Err(err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_api_mode",
                &format!("Unsupported api_mode '{raw_mode}'"),
            ));
        };
        cfg.api_mode = mode;
    }
    if let Some(base_url) = req
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cfg.base_url = base_url.to_string();
    }
    if let Some(model) = req
        .default_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cfg.default_model = model.to_string();
    }
    if let Some(enabled) = req.enabled {
        cfg.enabled = enabled;
    }
    if let Some(no_key) = req.no_key_required {
        cfg.no_key_required = no_key;
    }
    if let Some(region_id) = req.region_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        cfg.region_id = Some(region_id.to_string());
    }
    for (hk, hv) in &req.custom_headers {
        let k = hk.trim();
        let v = hv.trim();
        if !k.is_empty() && !v.is_empty() {
            cfg.headers.insert(k.to_string(), v.to_string());
        }
    }
    if let Some(api_key) = req.api_key.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        cfg.credentials = CredentialPool::with_single_key(
            format!("{pid}_api_key"),
            api_key.to_string(),
        );
    }

    let chosen_model = cfg.default_model.clone();
    let mode = cfg.api_mode;
    router.register_provider(cfg.clone());
    if !chosen_model.trim().is_empty() {
        router.register_model_profile(ModelProfile {
            id: chosen_model.clone(),
            provider_id: pid.clone(),
            model: chosen_model,
            api_mode: mode,
            input_cost_per_1m: 0.0,
            output_cost_per_1m: 0.0,
            context_window: 128_000,
            supports_tools: true,
            supports_reasoning: true,
            supports_caching: true,
        });
    }
    if req.activate {
        if let Err(err) = router.set_active(&pid, Some(&cfg.default_model)) {
            return Err(err_resp(
                StatusCode::BAD_REQUEST,
                "provider_activation_error",
                &err.to_string(),
            ));
        }
    }
    Ok(cfg)
}

async fn upsert_provider_impl(
    state: Arc<AppState>,
    req: MagpieUpsertProviderRequest,
    path_id_override: Option<String>,
) -> Response {
    let router = state.hermes.providers();
    let preset_id_clean = req
        .preset_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let saved_cfg = if let Some(preset_id) = preset_id_clean {
        let protocol_override = match req.api_mode.as_deref() {
            Some(raw) if !raw.trim().is_empty() => match ApiMode::parse(raw) {
                Some(m) => Some(m),
                None => {
                    return err_resp(
                        StatusCode::BAD_REQUEST,
                        "invalid_api_mode",
                        &format!("Unsupported api_mode '{raw}'"),
                    );
                }
            },
            _ => None,
        };
        let input = MagpiePresetApplyInput {
            preset_id: preset_id.to_string(),
            region_id: req.region_id.clone(),
            api_key: req.api_key.clone(),
            model_override: req.default_model.clone(),
            base_url_override: req.base_url.clone(),
            protocol_override,
            custom_headers: req.custom_headers.clone(),
            provider_id_override: path_id_override.clone().or_else(|| req.provider_id.clone()),
            display_name_override: req.name.clone(),
            activate: req.activate,
        };
        match router.apply_magpie_preset(input) {
            Ok(mut cfg) => {
                if let Some(enabled) = req.enabled {
                    cfg.enabled = enabled;
                    router.register_provider(cfg.clone());
                }
                if let Some(no_key) = req.no_key_required {
                    cfg.no_key_required = no_key;
                    router.register_provider(cfg.clone());
                }
                cfg
            }
            Err(err) => {
                return err_resp(
                    StatusCode::BAD_REQUEST,
                    "invalid_magpie_preset",
                    &err.to_string(),
                );
            }
        }
    } else {
        match apply_custom_or_existing_provider(router, &req, path_id_override.as_deref()) {
            Ok(cfg) => cfg,
            Err(resp) => return resp,
        }
    };

    if let Some(allow_local) = req.allow_local_fallback {
        router.set_allow_local_fallback(allow_local);
    }

    persist_and_sync_all(&state);
    let snap = router.snapshot();
    let view = provider_to_magpie_view(&saved_cfg, &snap);
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "provider": view,
            "state": build_magpie_state_json(&state),
        })),
    )
        .into_response()
}

/// POST / PUT /api/v1/hermes/magpie/providers
pub async fn magpie_providers_upsert(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<MagpieUpsertProviderRequest>,
) -> Response {
    upsert_provider_impl(state, req, None).await
}

/// GET /api/v1/hermes/magpie/providers
pub async fn magpie_providers_list(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let snap = state.hermes.providers().snapshot();
    let mut list: Vec<&ProviderConfig> = snap.providers.values().collect();
    list.sort_by(|a, b| a.id.cmp(&b.id));
    let providers: Vec<MagpieProviderView> = list
        .into_iter()
        .map(|cfg| provider_to_magpie_view(cfg, &snap))
        .collect();
    (
        StatusCode::OK,
        Json(json!({
            "providers": providers,
            "active_provider": snap.active_provider_id,
            "active_model": snap.active_model,
        })),
    )
        .into_response()
}

/// GET /api/v1/hermes/magpie/providers/{id}
pub async fn magpie_provider_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let snap = state.hermes.providers().snapshot();
    let Some(cfg) = snap.providers.get(id.trim()) else {
        return err_resp(StatusCode::NOT_FOUND, "provider_not_found", "Provider not found");
    };
    (
        StatusCode::OK,
        Json(json!({
            "provider": provider_to_magpie_view(cfg, &snap),
            "state": build_magpie_state_json(&state),
        })),
    )
        .into_response()
}

/// PUT / PATCH /api/v1/hermes/magpie/providers/{id}
pub async fn magpie_provider_update(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<MagpieUpsertProviderRequest>,
) -> Response {
    upsert_provider_impl(state, req, Some(id)).await
}

/// DELETE /api/v1/hermes/magpie/providers/{id}
pub async fn magpie_provider_delete(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let clean_id = id.trim().to_string();
    if clean_id == "local_hermes" {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "protected_provider",
            "Cannot remove built-in local_hermes fallback provider",
        );
    }
    let router = state.hermes.providers();
    if !router.remove_provider(&clean_id) {
        return err_resp(
            StatusCode::NOT_FOUND,
            "provider_not_found",
            &format!("Provider '{clean_id}' not found"),
        );
    }

    // If another configured provider remains and active fell back to local_hermes, promote it cleanly.
    let snap_mid = router.snapshot();
    if snap_mid.active_provider_id == "local_hermes" {
        let next_configured = snap_mid
            .providers
            .values()
            .filter(|p| {
                p.enabled
                    && p.id != "local_hermes"
                    && p.api_mode != ApiMode::LocalHermes
                    && (!p.credentials.is_empty() || p.no_key_required)
            })
            .min_by(|a, b| a.id.cmp(&b.id))
            .cloned();
        if let Some(next_p) = next_configured {
            let _ = router.set_active(&next_p.id, Some(&next_p.default_model));
        }
    }

    persist_and_sync_all(&state);
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "deleted_id": clean_id,
            "state": build_magpie_state_json(&state),
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize, Default)]
pub struct MagpieProbeRequest {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
}

/// POST /api/v1/hermes/magpie/providers/{id}/probe
pub async fn magpie_provider_probe(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<MagpieProbeRequest>>,
) -> Response {
    let clean_id = id.trim();
    let snap = state.hermes.providers().snapshot();
    let Some(cfg) = snap.providers.get(clean_id).cloned() else {
        return err_resp(
            StatusCode::NOT_FOUND,
            "provider_not_found",
            &format!("Provider '{clean_id}' not found"),
        );
    };

    let req_opts = body.map(|b| b.0).unwrap_or_default();
    let model = req_opts
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&cfg.default_model)
        .to_string();
    let prompt = req_opts
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Reply with OK.")
        .to_string();

    if cfg.api_mode == ApiMode::LocalHermes {
        return (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "provider_id": cfg.id,
                "model": model,
                "latency_ms": 0,
                "reply_preview": "Local Hermes runtime ready.",
                "error": Value::Null,
            })),
        )
            .into_response();
    }

    let tmp_router = ProviderRouter::new();
    tmp_router.set_allow_local_fallback(false);
    tmp_router.register_provider(ProviderConfig {
        enabled: true,
        ..cfg.clone()
    });
    if let Err(err) = tmp_router.set_active(&cfg.id, Some(&model)) {
        return (
            StatusCode::OK,
            Json(json!({
                "ok": false,
                "provider_id": cfg.id,
                "model": model,
                "latency_ms": 0,
                "reply_preview": Value::Null,
                "error": err.to_string(),
            })),
        )
            .into_response();
    }

    let mut prov_req = ProviderRequest::new(model.clone(), vec![ProviderChatMessage::user(prompt)]);
    prov_req.max_tokens = Some(32);
    prov_req.stream = false;

    let started = Instant::now();
    match tmp_router.complete(prov_req, None).await {
        Ok(resp) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            let preview: String = resp.content.trim().chars().take(160).collect();
            (
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "provider_id": resp.provider_id,
                    "model": resp.model,
                    "latency_ms": latency_ms,
                    "reply_preview": preview,
                    "error": Value::Null,
                })),
            )
                .into_response()
        }
        Err(err) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            (
                StatusCode::OK,
                Json(json!({
                    "ok": false,
                    "provider_id": cfg.id,
                    "model": model,
                    "latency_ms": latency_ms,
                    "reply_preview": Value::Null,
                    "error": err.to_string(),
                })),
            )
                .into_response()
        }
    }
}

/// POST /api/v1/hermes/magpie/subscriptions/scan
pub async fn magpie_subscriptions_scan(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let router = state.hermes.providers();
    let reg = router.refresh_local_subscriptions();
    persist_and_sync_all(&state);
    let subscriptions: Vec<SubscriptionAccount> = reg
        .accounts
        .iter()
        .map(sanitize_subscription_account)
        .collect();
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "subscriptions": subscriptions,
            "scanned_at_ms": reg.scanned_at_ms,
            "has_active_subscription": router.has_active_subscription(),
            "needs_provider_selection": router.needs_provider_selection(),
        })),
    )
        .into_response()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionAccountDto {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    pub kind: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    pub models: Vec<String>,
    pub active: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct SubscriptionActivateRequest {
    #[serde(default)]
    pub model: Option<String>,
}

/// GET /api/v1/hermes/magpie/subscriptions
pub async fn magpie_subscriptions_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let router = state.hermes.providers();
    let snap = router.snapshot();
    let subs = if snap.subscriptions.is_empty() {
        router.refresh_discovered_subscriptions().accounts
    } else {
        snap.subscriptions.clone()
    };
    let current_snap = router.snapshot();

    let dtos: Vec<SubscriptionAccountDto> = subs
        .iter()
        .map(|s| {
            let models = if let Some(cfg) = current_snap.providers.get(&s.id) {
                let mut m = vec![cfg.default_model.clone()];
                for fb in &cfg.fallback_models {
                    if !m.contains(fb) {
                        m.push(fb.clone());
                    }
                }
                m
            } else {
                match s.kind {
                    SubscriptionKind::GoogleVertex => vec![
                        "gemini-2.5-flash".to_string(),
                        "gemini-2.5-pro".to_string(),
                        "gemini-1.5-pro".to_string(),
                    ],
                    SubscriptionKind::CodexChatGpt => vec![
                        s.default_model.clone().unwrap_or_else(|| "codex-mini-latest".to_string()),
                        "gpt-4o".to_string(),
                    ],
                    SubscriptionKind::ClaudeCode => vec![
                        "claude-sonnet-4-20250514".to_string(),
                        "claude-3-5-haiku-20241022".to_string(),
                    ],
                    SubscriptionKind::GeminiCodeAssist => vec![
                        "gemini-2.5-flash".to_string(),
                        "gemini-2.5-pro".to_string(),
                    ],
                    _ => s.default_model.clone().into_iter().collect(),
                }
            };

            let is_active = s.id == current_snap.active_provider_id;

            SubscriptionAccountDto {
                id: s.id.clone(),
                account_id: s.account_id.clone().or_else(|| s.email.clone()),
                kind: s.kind.as_str().to_string(),
                label: s.label.clone(),
                plan: s.plan.clone(),
                models,
                active: is_active,
            }
        })
        .collect();

    (StatusCode::OK, Json(dtos)).into_response()
}

static ACTIVATE_SYNC_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// POST /api/v1/hermes/magpie/subscriptions/{id}/activate
pub async fn magpie_subscription_activate(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body_bytes: axum::body::Bytes,
) -> Response {
    let req: SubscriptionActivateRequest = if body_bytes.is_empty() {
        SubscriptionActivateRequest::default()
    } else {
        match serde_json::from_slice(&body_bytes) {
            Ok(r) => r,
            Err(e) => {
                return err_resp(
                    StatusCode::BAD_REQUEST,
                    "invalid_json",
                    &format!("Malformed activation request JSON: {e}"),
                );
            }
        }
    };

    let _sync_guard = ACTIVATE_SYNC_LOCK.lock().await;
    let router = state.hermes.providers();
    let res = router.activate_subscription_atomic(&id, req.model.as_deref());
    match res {
        Ok((cfg, active_model)) => {
            persist_and_sync_all(&state);
            (
                StatusCode::OK,
                Json(json!({
                    "status": "ok",
                    "active_provider_id": cfg.id,
                    "active_model": active_model,
                })),
            )
                .into_response()
        }
        Err(err) => err_resp(
            StatusCode::NOT_FOUND,
            "subscription_not_found",
            &err.to_string(),
        ),
    }
}

/// GET /api/v1/hermes/magpie/groups
pub async fn magpie_groups_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let snap = state.hermes.providers().snapshot();
    let mut groups: Vec<&RoutingGroup> = snap.routing_groups.values().collect();
    groups.sort_by(|a, b| a.id.cmp(&b.id));
    (
        StatusCode::OK,
        Json(json!({
            "routing_groups": groups,
            "routing_strategy": snap.routing_strategy,
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct MagpieGroupsUpsertRequest {
    #[serde(default)]
    pub group: Option<RoutingGroup>,
    #[serde(default)]
    pub routing_strategy: Option<String>,
    // Allow top-level RoutingGroup fields when `group` wrapper is omitted
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub strategy: Option<RoutingStrategy>,
    #[serde(default)]
    pub members: Option<Vec<magpie_gateway::RoutingGroupMember>>,
    #[serde(default)]
    pub intent_fast_model: Option<String>,
    #[serde(default)]
    pub intent_deep_model: Option<String>,
    #[serde(default)]
    pub sticky_ttl_ms: Option<i64>,
}

/// POST / PUT /api/v1/hermes/magpie/groups
pub async fn magpie_groups_upsert(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<MagpieGroupsUpsertRequest>,
) -> Response {
    let router = state.hermes.providers();

    if let Some(raw_strategy) = req.routing_strategy.as_deref() {
        let Some(strategy) = RoutingStrategy::parse(raw_strategy) else {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_routing_strategy",
                &format!("Unsupported routing strategy '{raw_strategy}'"),
            );
        };
        router.set_routing_strategy(strategy);
    }

    let group_opt = if let Some(g) = req.group {
        Some(g)
    } else if let Some(id) = req.id.filter(|s| !s.trim().is_empty()) {
        let mut g = RoutingGroup::new(
            id.clone(),
            req.name.unwrap_or_else(|| id.clone()),
            req.strategy.unwrap_or_default(),
        );
        if let Some(members) = req.members {
            g.members = members;
        }
        g.intent_fast_model = req.intent_fast_model;
        g.intent_deep_model = req.intent_deep_model;
        if let Some(ttl) = req.sticky_ttl_ms {
            g.sticky_ttl_ms = ttl;
        }
        Some(g)
    } else {
        None
    };

    if let Some(group) = group_opt {
        if let Err(err) = router.upsert_routing_group(group) {
            return err_resp(StatusCode::BAD_REQUEST, "invalid_group", &err.to_string());
        }
    } else if req.routing_strategy.is_none() {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_group",
            "Expected routing group payload or routing_strategy",
        );
    }

    persist_and_sync_all(&state);
    let snap = router.snapshot();
    let mut groups: Vec<&RoutingGroup> = snap.routing_groups.values().collect();
    groups.sort_by(|a, b| a.id.cmp(&b.id));
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "routing_groups": groups,
            "routing_strategy": snap.routing_strategy,
            "state": build_magpie_state_json(&state),
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize, Default)]
pub struct DeleteGroupQuery {
    pub id: Option<String>,
}

/// DELETE /api/v1/hermes/magpie/groups/{id}
pub async fn magpie_group_delete_path(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    delete_group_impl(&state, &id)
}

/// DELETE /api/v1/hermes/magpie/groups?id=...
pub async fn magpie_groups_delete_query(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(q): Query<DeleteGroupQuery>,
) -> Response {
    let Some(id) = q.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return err_resp(StatusCode::BAD_REQUEST, "missing_group_id", "id query parameter required");
    };
    delete_group_impl(&state, id)
}

fn delete_group_impl(state: &AppState, id: &str) -> Response {
    let router = state.hermes.providers();
    if !router.delete_routing_group(id) {
        return err_resp(
            StatusCode::NOT_FOUND,
            "group_not_found",
            &format!("Routing group '{id}' not found"),
        );
    }
    persist_and_sync_all(state);
    let snap = router.snapshot();
    let mut groups: Vec<&RoutingGroup> = snap.routing_groups.values().collect();
    groups.sort_by(|a, b| a.id.cmp(&b.id));
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "routing_groups": groups,
            "routing_strategy": snap.routing_strategy,
            "state": build_magpie_state_json(state),
        })),
    )
        .into_response()
}

/// GET /api/v1/hermes/magpie/masking
pub async fn magpie_masking_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let snap = state.hermes.providers().snapshot();
    (StatusCode::OK, Json(snap.masking)).into_response()
}

/// PUT / POST /api/v1/hermes/magpie/masking
pub async fn magpie_masking_put(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(cfg): Json<MaskingConfig>,
) -> Response {
    let router = state.hermes.providers();
    router.set_masking_config(cfg.clone());
    persist_and_sync_all(&state);
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "masking": cfg,
            "state": build_magpie_state_json(&state),
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct MagpieMaskingPreviewRequest {
    pub text: String,
    #[serde(default)]
    pub config: Option<MaskingConfig>,
}

/// POST /api/v1/hermes/magpie/masking/preview
pub async fn magpie_masking_preview(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<MagpieMaskingPreviewRequest>,
) -> Response {
    let cfg = req
        .config
        .unwrap_or_else(|| state.hermes.providers().snapshot().masking);
    let engine = MaskingEngine::new(cfg);
    let res = engine.preview_text(&req.text);
    (
        StatusCode::OK,
        Json(json!({
            "masked_text": res.masked_text,
            "hits": res.hits,
            "hit_count": res.hits.len(),
            "blocked": res.blocked,
            "block_reason": res.block_reason,
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct MagpieImportRequest {
    pub raw: String,
    #[serde(default)]
    pub activate: bool,
}

fn register_imported_spec(
    router: &ProviderRouter,
    spec: &ImportedProviderSpec,
    activate: bool,
) -> Result<String, ProviderError> {
    if let Some(ref preset_id) = spec.preset_id {
        let cfg = router.apply_magpie_preset(MagpiePresetApplyInput {
            preset_id: preset_id.clone(),
            region_id: spec.region_id.clone(),
            api_key: spec.api_key.clone(),
            model_override: (!spec.default_model.trim().is_empty())
                .then(|| spec.default_model.clone()),
            base_url_override: (!spec.base_url.trim().is_empty()).then(|| spec.base_url.clone()),
            protocol_override: Some(ApiMode::from_protocol_kind(spec.protocol)),
            custom_headers: HashMap::new(),
            provider_id_override: Some(spec.id.clone()),
            display_name_override: Some(spec.name.clone()),
            activate,
        })?;
        return Ok(cfg.id);
    }

    let mode = ApiMode::from_protocol_kind(spec.protocol);
    let mut credentials = CredentialPool::new();
    if let Some(ref key) = spec.api_key {
        if !key.trim().is_empty() {
            credentials.add_entry(CredentialEntry::new(
                format!("{}_imported_key", spec.id),
                key.trim().to_string(),
                "import",
            ));
        }
    }
    let fallback_models: Vec<String> = spec
        .models
        .iter()
        .filter(|m| *m != &spec.default_model)
        .cloned()
        .collect();
    let cfg = ProviderConfig {
        id: spec.id.clone(),
        name: spec.name.clone(),
        api_mode: mode,
        base_url: spec.base_url.clone(),
        default_model: spec.default_model.clone(),
        headers: HashMap::new(),
        credentials,
        fallback_models,
        timeout_secs: 90,
        enabled: true,
        preset_id: spec.preset_id.clone(),
        region_id: spec.region_id.clone(),
        is_subscription: false,
        no_key_required: spec.api_key.as_ref().is_none_or(|k| k.trim().is_empty()),
        routing_strategy: None,
    };
    router.register_provider(cfg);
    for m in spec.models.iter().chain(std::iter::once(&spec.default_model)) {
        if m.trim().is_empty() {
            continue;
        }
        router.register_model_profile(ModelProfile {
            id: m.clone(),
            provider_id: spec.id.clone(),
            model: m.clone(),
            api_mode: mode,
            input_cost_per_1m: 0.0,
            output_cost_per_1m: 0.0,
            context_window: 128_000,
            supports_tools: true,
            supports_reasoning: true,
            supports_caching: true,
        });
    }
    if activate {
        router.set_active(&spec.id, Some(&spec.default_model))?;
    }
    Ok(spec.id.clone())
}

/// POST /api/v1/hermes/magpie/import
pub async fn magpie_import(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<MagpieImportRequest>,
) -> Response {
    let raw = req.raw.trim();
    if raw.is_empty() {
        return err_resp(StatusCode::BAD_REQUEST, "empty_import", "Import text cannot be empty");
    }

    let specs = if raw.starts_with("magpie://") || raw.starts_with("https://usemagpie.ai/import") {
        match parse_magpie_import_link(raw) {
            Ok(spec) => vec![spec],
            Err(err) => {
                return err_resp(StatusCode::BAD_REQUEST, "invalid_import_link", &err.to_string());
            }
        }
    } else {
        parse_dotenv_providers(raw)
    };

    if specs.is_empty() {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "no_providers_found",
            "No valid Magpie import link or .env provider keys were detected",
        );
    }

    let router = state.hermes.providers();
    let mut imported_ids = Vec::with_capacity(specs.len());
    for (idx, spec) in specs.iter().enumerate() {
        let should_activate = req.activate && idx == 0;
        match register_imported_spec(router, spec, should_activate) {
            Ok(pid) => imported_ids.push(pid),
            Err(err) => {
                return err_resp(StatusCode::BAD_REQUEST, "import_failed", &err.to_string());
            }
        }
    }

    persist_and_sync_all(&state);
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "imported_count": imported_ids.len(),
            "imported_provider_ids": imported_ids,
            "state": build_magpie_state_json(&state),
        })),
    )
        .into_response()
}

// ============================================================================
// Built-in Magpie Gateway Endpoints (/api/v1/magpie/v1/*)
// ============================================================================

#[derive(Clone, Copy)]
enum GatewayResponseFormat {
    ChatCompletions,
    Responses,
    AnthropicMessages,
}

fn verify_optional_chat_scope(
    headers: &HeaderMap,
    state: &AppState,
    payload: &Value,
) -> Result<(), Response> {
    let Some(cid) = payload.get("chat_id").and_then(|v| v.as_i64()) else {
        return Ok(());
    };
    let Some(uid) = crate::routes::hermes::resolve_optional_bearer_user(headers, state) else {
        return Err(err_resp(
            StatusCode::UNAUTHORIZED,
            "missing_token",
            "Authenticated Bearer token is required when referencing chat_id",
        ));
    };
    check_active_membership(state, ChatId(cid), uid)?;
    Ok(())
}

fn unified_to_provider_request(req: UnifiedRequest) -> ProviderRequest {
    let mut system_tiers = Vec::new();
    let mut messages = Vec::with_capacity(req.messages.len());

    if !req.system_prompt.trim().is_empty() {
        system_tiers.push(gpui_telegram_agent::hermes::providers::SystemPromptTier::volatile(
            "gateway_system",
            req.system_prompt,
        ));
    }

    for msg in req.messages {
        match msg.role {
            UnifiedRole::System => {
                if !msg.content.trim().is_empty() {
                    system_tiers.push(
                        gpui_telegram_agent::hermes::providers::SystemPromptTier::volatile(
                            "system_msg",
                            msg.content,
                        ),
                    );
                }
            }
            UnifiedRole::User => messages.push(ProviderChatMessage {
                role: ProviderMessageRole::User,
                content: msg.content,
                name: msg.name,
                tool_call_id: None,
                tool_calls: Vec::new(),
                reasoning: None,
                cache_control: None,
            }),
            UnifiedRole::Assistant => messages.push(ProviderChatMessage {
                role: ProviderMessageRole::Assistant,
                content: msg.content,
                name: msg.name,
                tool_call_id: None,
                tool_calls: msg
                    .tool_calls
                    .into_iter()
                    .map(|tc| ProviderToolCall {
                        id: tc.id,
                        name: tc.name,
                        arguments: tc.arguments,
                    })
                    .collect(),
                reasoning: msg.reasoning,
                cache_control: None,
            }),
            UnifiedRole::Tool => messages.push(ProviderChatMessage {
                role: ProviderMessageRole::Tool,
                content: msg.content,
                name: msg.name,
                tool_call_id: msg.tool_call_id,
                tool_calls: Vec::new(),
                reasoning: None,
                cache_control: None,
            }),
        }
    }

    let tools = req
        .tools
        .into_iter()
        .map(|t: UnifiedToolSpec| ProviderToolSpec {
            name: t.name,
            description: t.description,
            parameters: t.parameters,
        })
        .collect();

    ProviderRequest {
        model: req.model,
        system_tiers,
        messages,
        tools,
        temperature: req.temperature,
        max_tokens: req.max_tokens,
        thinking_budget_tokens: req.thinking_budget_tokens,
        stream: req.stream,
        metadata: json!({ "source": "magpie_gateway" }),
    }
}

fn provider_response_to_unified(resp: ProviderResponse) -> UnifiedResponse {
    UnifiedResponse {
        provider_id: resp.provider_id,
        model: resp.model,
        content: resp.content,
        reasoning: resp.reasoning,
        tool_calls: resp
            .tool_calls
            .into_iter()
            .map(|tc| UnifiedToolCall {
                id: tc.id,
                name: tc.name,
                arguments: tc.arguments,
            })
            .collect(),
        finish_reason: resp.finish_reason,
        usage: UnifiedUsage {
            input_tokens: resp.usage.input_tokens,
            output_tokens: resp.usage.output_tokens,
            cache_read_tokens: resp.usage.cache_read_tokens,
            total_tokens: resp.usage.total_tokens,
        },
    }
}

fn map_gateway_provider_error(err: ProviderError, router: &ProviderRouter) -> Response {
    match err {
        ProviderError::Configuration(msg) => {
            let needs_sel = router.needs_provider_selection();
            let status = if needs_sel {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::BAD_REQUEST
            };
            (
                status,
                Json(json!({
                    "type": "error",
                    "error": {
                        "code": if needs_sel { "provider_not_configured" } else { "configuration_error" },
                        "type": "configuration_error",
                        "message": msg,
                        "needs_provider_selection": needs_sel,
                        "action": if needs_sel { Some("hermes:open_ai_providers") } else { None },
                    }
                })),
            )
                .into_response()
        }
        ProviderError::Authentication(msg) => {
            err_resp(StatusCode::UNAUTHORIZED, "authentication_error", &msg)
        }
        ProviderError::RateLimited { message, .. } => {
            err_resp(StatusCode::TOO_MANY_REQUESTS, "rate_limit_exceeded", &message)
        }
        other => err_resp(StatusCode::BAD_GATEWAY, "upstream_error", &other.to_string()),
    }
}

struct StreamSseState {
    emitted_role: bool,
    emitted_text: bool,
    emitted_thinking: bool,
    emitted_tools: bool,
    tool_index: usize,
}

impl StreamSseState {
    fn new() -> Self {
        Self {
            emitted_role: false,
            emitted_text: false,
            emitted_thinking: false,
            emitted_tools: false,
            tool_index: 0,
        }
    }
}

struct AbortTaskOnDrop<T>(Option<tokio::task::JoinHandle<T>>);

impl<T> Drop for AbortTaskOnDrop<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

fn format_sse_delta(
    format: GatewayResponseFormat,
    id: &str,
    model: &str,
    delta: &str,
    created: i64,
) -> String {
    match format {
        GatewayResponseFormat::ChatCompletions => {
            let chunk = json!({
                "id": id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": { "content": delta },
                    "finish_reason": Value::Null
                }]
            });
            format!("data: {chunk}\n\n")
        }
        GatewayResponseFormat::Responses => {
            let chunk = json!({
                "type": "response.output_text.delta",
                "response_id": id,
                "delta": delta
            });
            format!("event: response.output_text.delta\ndata: {chunk}\n\n")
        }
        GatewayResponseFormat::AnthropicMessages => {
            let chunk = json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": delta }
            });
            format!("event: content_block_delta\ndata: {chunk}\n\n")
        }
    }
}

fn format_sse_event(
    format: GatewayResponseFormat,
    id: &str,
    model: &str,
    ev: ProviderStreamEvent,
    created: i64,
    st: &mut StreamSseState,
) -> Option<String> {
    match ev {
        ProviderStreamEvent::TextDelta(delta) => {
            if delta.is_empty() {
                return None;
            }
            st.emitted_text = true;
            match format {
                GatewayResponseFormat::ChatCompletions => {
                    let mut delta_obj = serde_json::Map::new();
                    if !st.emitted_role {
                        delta_obj.insert("role".to_string(), json!("assistant"));
                        st.emitted_role = true;
                    }
                    delta_obj.insert("content".to_string(), json!(delta));
                    let chunk = json!({
                        "id": id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": model,
                        "choices": [{
                            "index": 0,
                            "delta": Value::Object(delta_obj),
                            "finish_reason": Value::Null
                        }]
                    });
                    Some(format!("data: {chunk}\n\n"))
                }
                _ => Some(format_sse_delta(format, id, model, &delta, created)),
            }
        }
        ProviderStreamEvent::ThinkingDelta(thinking) => {
            if thinking.is_empty() {
                return None;
            }
            st.emitted_thinking = true;
            match format {
                GatewayResponseFormat::ChatCompletions => {
                    let mut delta_obj = serde_json::Map::new();
                    if !st.emitted_role {
                        delta_obj.insert("role".to_string(), json!("assistant"));
                        st.emitted_role = true;
                    }
                    delta_obj.insert("reasoning_content".to_string(), json!(thinking));
                    let chunk = json!({
                        "id": id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": model,
                        "choices": [{
                            "index": 0,
                            "delta": Value::Object(delta_obj),
                            "finish_reason": Value::Null
                        }]
                    });
                    Some(format!("data: {chunk}\n\n"))
                }
                GatewayResponseFormat::Responses => {
                    let chunk = json!({
                        "type": "response.reasoning_summary_text.delta",
                        "response_id": id,
                        "delta": thinking
                    });
                    Some(format!("event: response.reasoning_summary_text.delta\ndata: {chunk}\n\n"))
                }
                GatewayResponseFormat::AnthropicMessages => {
                    let chunk = json!({
                        "type": "content_block_delta",
                        "index": 0,
                        "delta": { "type": "thinking_delta", "thinking": thinking }
                    });
                    Some(format!("event: content_block_delta\ndata: {chunk}\n\n"))
                }
            }
        }
        ProviderStreamEvent::ToolCallReady(tc) => {
            st.emitted_tools = true;
            let idx = st.tool_index;
            st.tool_index += 1;
            match format {
                GatewayResponseFormat::ChatCompletions => {
                    let mut delta_obj = serde_json::Map::new();
                    if !st.emitted_role {
                        delta_obj.insert("role".to_string(), json!("assistant"));
                        st.emitted_role = true;
                    }
                    delta_obj.insert(
                        "tool_calls".to_string(),
                        json!([{
                            "index": idx,
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": tc.arguments
                            }
                        }]),
                    );
                    let chunk = json!({
                        "id": id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": model,
                        "choices": [{
                            "index": 0,
                            "delta": Value::Object(delta_obj),
                            "finish_reason": Value::Null
                        }]
                    });
                    Some(format!("data: {chunk}\n\n"))
                }
                GatewayResponseFormat::Responses => {
                    let chunk = json!({
                        "type": "response.output_item.done",
                        "response_id": id,
                        "item": {
                            "type": "function_call",
                            "call_id": tc.id,
                            "name": tc.name,
                            "arguments": tc.arguments
                        }
                    });
                    Some(format!("event: response.output_item.done\ndata: {chunk}\n\n"))
                }
                GatewayResponseFormat::AnthropicMessages => {
                    let block_idx = idx + 1;
                    let start = json!({
                        "type": "content_block_start",
                        "index": block_idx,
                        "content_block": {
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": {}
                        }
                    });
                    let delta = json!({
                        "type": "content_block_delta",
                        "index": block_idx,
                        "delta": { "type": "input_json_delta", "partial_json": tc.arguments }
                    });
                    Some(format!(
                        "event: content_block_start\ndata: {start}\n\nevent: content_block_delta\ndata: {delta}\n\n"
                    ))
                }
            }
        }
        ProviderStreamEvent::Usage(u) => match format {
            GatewayResponseFormat::ChatCompletions => {
                let total = if u.total_tokens > 0 {
                    u.total_tokens
                } else {
                    u.input_tokens.saturating_add(u.output_tokens)
                };
                let chunk = json!({
                    "id": id,
                    "object": "chat.completion.chunk",
                    "created": created,
                    "model": model,
                    "choices": [],
                    "usage": {
                        "prompt_tokens": u.input_tokens,
                        "completion_tokens": u.output_tokens,
                        "total_tokens": total,
                        "prompt_tokens_details": {
                            "cached_tokens": u.cache_read_tokens
                        }
                    }
                });
                Some(format!("data: {chunk}\n\n"))
            }
            _ => None,
        },
    }
}

fn format_sse_done(
    format: GatewayResponseFormat,
    id: &str,
    model: &str,
    resp: &UnifiedResponse,
    st: &StreamSseState,
    created: i64,
) -> String {
    let actual_finish = if resp.finish_reason.trim().is_empty() {
        "stop"
    } else {
        resp.finish_reason.as_str()
    };
    let total_tokens = if resp.usage.total_tokens > 0 {
        resp.usage.total_tokens
    } else {
        resp.usage.input_tokens.saturating_add(resp.usage.output_tokens)
    };
    match format {
        GatewayResponseFormat::ChatCompletions => {
            let mut fallback_delta = serde_json::Map::new();
            if !st.emitted_role {
                fallback_delta.insert("role".to_string(), json!("assistant"));
            }
            if !st.emitted_thinking {
                if let Some(ref r) = resp.reasoning {
                    if !r.is_empty() {
                        fallback_delta.insert("reasoning_content".to_string(), json!(r));
                    }
                }
            }
            if !st.emitted_text && !resp.content.is_empty() {
                fallback_delta.insert("content".to_string(), json!(resp.content));
            }
            if !st.emitted_tools && !resp.tool_calls.is_empty() {
                let tcs: Vec<Value> = resp
                    .tool_calls
                    .iter()
                    .enumerate()
                    .map(|(idx, tc)| {
                        json!({
                            "index": idx,
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": tc.arguments
                            }
                        })
                    })
                    .collect();
                fallback_delta.insert("tool_calls".to_string(), Value::Array(tcs));
            }
            let final_chunk = json!({
                "id": id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": Value::Object(fallback_delta),
                    "finish_reason": actual_finish
                }],
                "usage": {
                    "prompt_tokens": resp.usage.input_tokens,
                    "completion_tokens": resp.usage.output_tokens,
                    "total_tokens": total_tokens,
                    "prompt_tokens_details": {
                        "cached_tokens": resp.usage.cache_read_tokens
                    }
                }
            });
            format!("data: {final_chunk}\n\ndata: [DONE]\n\n")
        }
        GatewayResponseFormat::Responses => {
            let completed = json!({
                "type": "response.completed",
                "response": unified_to_responses_json(resp)
            });
            format!("event: response.completed\ndata: {completed}\n\ndata: [DONE]\n\n")
        }
        GatewayResponseFormat::AnthropicMessages => {
            let mut out = String::new();
            if !st.emitted_text && !resp.content.is_empty() {
                out.push_str(&format_sse_delta(
                    format,
                    id,
                    model,
                    &resp.content,
                    created,
                ));
            }
            let stop_reason = match actual_finish {
                "stop" => "end_turn",
                "tool_calls" => "tool_use",
                "length" => "max_tokens",
                other => other,
            };
            let msg_delta = json!({
                "type": "message_delta",
                "delta": { "stop_reason": stop_reason, "stop_sequence": Value::Null },
                "usage": {
                    "input_tokens": resp.usage.input_tokens,
                    "output_tokens": resp.usage.output_tokens
                }
            });
            out.push_str(&format!("event: message_delta\ndata: {msg_delta}\n\n"));
            let stop = json!({ "type": "message_stop" });
            out.push_str(&format!("event: message_stop\ndata: {stop}\n\n"));
            out
        }
    }
}

fn format_sse_error(format: GatewayResponseFormat, err_type: &str, message: &str) -> String {
    match format {
        GatewayResponseFormat::ChatCompletions => {
            let err_json = json!({
                "error": {
                    "message": message,
                    "type": err_type,
                }
            });
            format!("data: {err_json}\n\n")
        }
        GatewayResponseFormat::Responses => {
            let err_json = json!({
                "type": "error",
                "error": {
                    "type": err_type,
                    "message": message,
                }
            });
            format!("event: error\ndata: {err_json}\n\n")
        }
        GatewayResponseFormat::AnthropicMessages => {
            let anth_type = match err_type {
                "authentication_error" | "rate_limit_error" | "invalid_request_error" => err_type,
                _ => "api_error",
            };
            let err_json = json!({
                "type": "error",
                "error": {
                    "type": anth_type,
                    "message": message,
                }
            });
            format!("event: error\ndata: {err_json}\n\n")
        }
    }
}

fn stream_gateway_sse(
    router: ProviderRouter,
    prov_req: ProviderRequest,
    format: GatewayResponseFormat,
) -> Response {
    let model_hint = if prov_req.model.trim().is_empty() {
        router.active_info().1
    } else {
        prov_req.model.clone()
    };
    let req_id = format!("magpie_{}", Uuid::new_v4().simple());
    let created = Utc::now().timestamp();

    let (tx, rx) = mpsc::channel::<ProviderStreamEvent>(64);
    let task = tokio::spawn(async move { router.complete(prov_req, Some(tx)).await });

    let sse_stream = stream::unfold(
        (rx, AbortTaskOnDrop(Some(task)), false, StreamSseState::new(), req_id, model_hint),
        move |(mut rx, mut task_guard, finished, mut sse_st, id, mdl)| async move {
            if finished {
                return None;
            }
            while let Some(ev) = rx.recv().await {
                if let Some(payload) = format_sse_event(format, &id, &mdl, ev, created, &mut sse_st) {
                    return Some((
                        Ok::<axum::body::Bytes, Infallible>(axum::body::Bytes::from(payload)),
                        (rx, task_guard, false, sse_st, id, mdl),
                    ));
                }
            }

            let out = if let Some(handle) = task_guard.0.take() {
                match handle.await {
                    Ok(Ok(resp)) => {
                        let u = provider_response_to_unified(resp);
                        format_sse_done(format, &id, &u.model, &u, &sse_st, created)
                    }
                    Ok(Err(err)) => {
                        let err_type = match &err {
                            ProviderError::Authentication(_) => "authentication_error",
                            ProviderError::RateLimited { .. } => "rate_limit_error",
                            ProviderError::Configuration(_) => "invalid_request_error",
                            _ => "upstream_error",
                        };
                        format_sse_error(format, err_type, &err.to_string())
                    }
                    Err(join_err) => {
                        format_sse_error(format, "upstream_error", &join_err.to_string())
                    }
                }
            } else {
                return None;
            };

            Some((
                Ok::<axum::body::Bytes, Infallible>(axum::body::Bytes::from(out)),
                (rx, task_guard, true, sse_st, id, mdl),
            ))
        },
    )
    .boxed();

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(sse_stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn handle_gateway_completion(
    headers: HeaderMap,
    state: Arc<AppState>,
    payload: Value,
    format: GatewayResponseFormat,
) -> Response {
    if let Err(resp) = verify_optional_chat_scope(&headers, &state, &payload) {
        return resp;
    }

    let unified_res = match format {
        GatewayResponseFormat::ChatCompletions => inbound_chat_completions_to_unified(&payload),
        GatewayResponseFormat::Responses => inbound_responses_to_unified(&payload),
        GatewayResponseFormat::AnthropicMessages => inbound_anthropic_to_unified(&payload),
    };
    let unified = match unified_res {
        Ok(u) => u,
        Err(err) => {
            return err_resp(StatusCode::BAD_REQUEST, "invalid_request", &err.to_string());
        }
    };

    let router = state.hermes.providers().clone();
    let stream_flag = unified.stream;
    let prov_req = unified_to_provider_request(unified);

    if stream_flag {
        return stream_gateway_sse(router, prov_req, format);
    }

    match router.complete(prov_req, None).await {
        Ok(resp) => {
            let u = provider_response_to_unified(resp);
            let out = match format {
                GatewayResponseFormat::ChatCompletions => unified_to_chat_completions_json(&u),
                GatewayResponseFormat::Responses => unified_to_responses_json(&u),
                GatewayResponseFormat::AnthropicMessages => unified_to_anthropic_json(&u),
            };
            (StatusCode::OK, Json(out)).into_response()
        }
        Err(err) => map_gateway_provider_error(err, &router),
    }
}

/// GET /api/v1/magpie/v1/models
pub async fn magpie_gateway_models(State(state): State<Arc<AppState>>) -> Response {
    let snap = state.hermes.providers().snapshot();
    let mut seen = HashSet::new();
    let mut data = Vec::new();
    let now = Utc::now().timestamp();

    for group_id in snap.routing_groups.keys() {
        let model_id = format!("group/{group_id}");
        if seen.insert(model_id.clone()) {
            data.push(json!({
                "id": model_id,
                "object": "model",
                "created": now,
                "owned_by": "magpie-group",
            }));
        }
    }
    for prof in snap.model_profiles.values() {
        if seen.insert(prof.model.clone()) {
            data.push(json!({
                "id": prof.model,
                "object": "model",
                "created": now,
                "owned_by": prof.provider_id,
            }));
        }
    }
    for p in snap.providers.values() {
        if !p.default_model.trim().is_empty() && seen.insert(p.default_model.clone()) {
            data.push(json!({
                "id": p.default_model,
                "object": "model",
                "created": now,
                "owned_by": p.id,
            }));
        }
    }

    (
        StatusCode::OK,
        Json(json!({
            "object": "list",
            "data": data,
        })),
    )
        .into_response()
}

/// POST /api/v1/magpie/v1/chat/completions
pub async fn magpie_gateway_chat_completions(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    handle_gateway_completion(headers, state, payload, GatewayResponseFormat::ChatCompletions)
        .await
}

/// POST /api/v1/magpie/v1/responses
pub async fn magpie_gateway_responses(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    handle_gateway_completion(headers, state, payload, GatewayResponseFormat::Responses).await
}

/// POST /api/v1/magpie/v1/messages
pub async fn magpie_gateway_messages(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    handle_gateway_completion(
        headers,
        state,
        payload,
        GatewayResponseFormat::AnthropicMessages,
    )
    .await
}
