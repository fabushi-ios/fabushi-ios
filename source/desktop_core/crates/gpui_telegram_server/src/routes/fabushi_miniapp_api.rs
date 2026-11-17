use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use fabushi_miniapp::state::{
    dharma_ai_fallback_reply, AiChatMessage, HttpResponsePayload, LockedPractice, UserProfileDto,
};
use fabushi_miniapp::storage;
use fabushi_miniapp::sutras::{format_leaderboard_bytes, sutra_by_title};
use gpui_telegram_agent::hermes::providers::ApiMode;
use gpui_telegram_agent::hermes::HermesTurnRequest;
use gpui_telegram_core::types::{ChatId, UserId};
use gpui_telegram_proto::message::InlineKeyboardButton;
use gpui_telegram_proto::miniapp_contracts::MiniAppActionRequest;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::db::miniapp_store::{verify_webapp_init_data, BotRegistryRecord, MiniAppRecord};
use crate::routes::err_resp;
use crate::server::{AppState, AuthBearer};

const DEFAULT_AI_SYSTEM_PROMPT: &str = "\
你现在是「全球法布施 (@fabushi_bot)」小程序内的慈悲修行助手。\
请以温和、恭敬、简明、正念的语言回答用户的佛法修行、诵经、禅修与全球法布施问题，\
适当引用经典原文或日常修行建议。切勿编造虚假的实时数据。";

#[derive(Clone)]
pub struct FabushiAuth {
    pub user_id: UserId,
    pub bot: Option<BotRegistryRecord>,
    pub init_username: Option<String>,
    pub init_display_name: Option<String>,
}

fn unauthorized(code: &str, msg: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "type": "error",
            "error": code,
            "message": msg,
        })),
    )
}

fn bad_request(msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "type": "error",
            "error": "bad_request",
            "message": msg,
        })),
    )
        .into_response()
}

fn not_found(msg: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "type": "error",
            "error": "not_found",
            "message": msg,
        })),
    )
        .into_response()
}

fn internal_err(msg: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "type": "error",
            "error": "internal_error",
            "message": msg,
        })),
    )
        .into_response()
}

fn extract_init_data_header(parts: &Parts) -> Option<String> {
    if let Some(raw) = parts
        .headers
        .get("x-telegram-init-data")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Some(raw.to_string());
    }
    if let Some(auth) = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
    {
        if let Some(rest) = auth.strip_prefix("tma ").or_else(|| auth.strip_prefix("TMA ")) {
            let trimmed = rest.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn parse_init_data_user(
    params: &BTreeMap<String, String>,
) -> Option<(UserId, Option<String>, String)> {
    let user_raw = params.get("user")?;
    let user_val: Value = serde_json::from_str(user_raw).ok()?;
    let id = user_val.get("id").and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_u64().and_then(|u| i64::try_from(u).ok()))
            .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
    })?;
    if id <= 0 {
        return None;
    }
    let username = user_val
        .get("username")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let first = user_val
        .get("first_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let last = user_val
        .get("last_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let display = match (first.is_empty(), last.is_empty()) {
        (false, false) => format!("{first} {last}"),
        (false, true) => first.to_string(),
        (true, false) => last.to_string(),
        (true, true) => username.clone().unwrap_or_else(|| format!("user_{id}")),
    };
    Some((UserId(id as u64), username, display))
}

pub(crate) fn resolve_fabushi_bot(state: &AppState) -> Option<BotRegistryRecord> {
    if let Ok(Some(app)) = state.db.get_mini_app("fabushi_bot", "fabushi") {
        if let Ok(Some(bot)) = state.db.get_bot_by_user_id(app.bot_user_id) {
            return Some(bot);
        }
    }
    if let Ok(Some(bot)) = state.db.get_bot_by_username("fabushi_bot") {
        return Some(bot);
    }
    if let Ok(bots) = state.db.list_all_bots() {
        for bot in bots {
            if state
                .db
                .get_mini_app(&bot.username, "fabushi")
                .ok()
                .flatten()
                .is_some()
            {
                return Some(bot);
            }
        }
    }
    None
}

fn verify_init_data_against_bots(
    state: &AppState,
    raw: &str,
) -> Option<(BotRegistryRecord, BTreeMap<String, String>)> {
    if let Some(primary) = resolve_fabushi_bot(state) {
        if let Ok(parsed) = verify_webapp_init_data(&primary.bot_token, raw) {
            return Some((primary, parsed));
        }
    }
    if let Ok(bots) = state.db.list_all_bots() {
        for bot in bots {
            if let Ok(parsed) = verify_webapp_init_data(&bot.bot_token, raw) {
                return Some((bot, parsed));
            }
        }
    }
    None
}

impl FromRequestParts<Arc<AppState>> for FabushiAuth {
    type Rejection = (StatusCode, Json<Value>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        if let Some(raw_init_data) = extract_init_data_header(parts) {
            let Some((bot, parsed)) = verify_init_data_against_bots(state, &raw_init_data) else {
                return Err(unauthorized(
                    "invalid_init_data",
                    "Invalid or expired X-Telegram-Init-Data HMAC signature",
                ));
            };
            let Some((user_id, init_username, init_display)) = parse_init_data_user(&parsed) else {
                return Err(unauthorized(
                    "missing_init_data_user",
                    "Signed X-Telegram-Init-Data does not contain a valid user payload",
                ));
            };
            return Ok(FabushiAuth {
                user_id,
                bot: Some(bot),
                init_username,
                init_display_name: Some(init_display),
            });
        }

        let bearer = AuthBearer::from_request_parts(parts, state).await?;
        let bot = resolve_fabushi_bot(state);
        Ok(FabushiAuth {
            user_id: bearer.user_id,
            bot,
            init_username: None,
            init_display_name: None,
        })
    }
}

fn ensure_fabushi_user(state: &AppState, auth: &FabushiAuth) -> Result<UserProfileDto, String> {
    let uid = auth.user_id.0 as i64;
    let user_opt = state.db.get_user(auth.user_id).ok().flatten();
    let (username, display_name, avatar_url) = if let Some(u) = user_opt {
        let uname = u
            .username
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| auth.init_username.clone())
            .unwrap_or_else(|| format!("user_{uid}"));
        let dname = {
            let combined = format!(
                "{} {}",
                u.first_name.trim(),
                u.last_name.as_deref().unwrap_or("").trim()
            )
            .trim()
            .to_string();
            if combined.is_empty() {
                auth.init_display_name
                    .clone()
                    .unwrap_or_else(|| uname.clone())
            } else {
                combined
            }
        };
        let avatar = u
            .profile_photo_file_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|fid| format!("/api/v1/files/{fid}"))
            .unwrap_or_default();
        (uname, dname, avatar)
    } else {
        let uname = auth
            .init_username
            .clone()
            .unwrap_or_else(|| format!("user_{uid}"));
        let dname = auth
            .init_display_name
            .clone()
            .unwrap_or_else(|| uname.clone());
        (uname, dname, String::new())
    };
    let is_admin = username == "fabushi_official_publisher"
        || username == "admin"
        || auth.user_id.0 == 1;

    let membership = state.db.get_user_membership(auth.user_id).ok();

    state
        .db
        .pool()
        .with_conn(|conn| {
            let mut profile =
                storage::ensure_profile(conn, uid, &username, &display_name, &avatar_url, is_admin)
                    .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))?;
            if let Some(m) = membership.as_ref().filter(|m| m.is_active && !profile.is_vip) {
                profile = storage::activate_membership(conn, uid, &m.membership_type).map_err(
                    |e| gpui_telegram_core::error::TelegramError::Database(e.to_string()),
                )?;
            }
            Ok(profile)
        })
        .map_err(|e| e.to_string())
}

fn sync_summary_and_maybe_notify(
    state: &Arc<AppState>,
    uid: i64,
    bot_hint: Option<&BotRegistryRecord>,
    receipt_message: Option<&str>,
) {
    let resolved_bot = bot_hint.cloned().or_else(|| resolve_fabushi_bot(state));
    let Some(bot) = resolved_bot else {
        return;
    };

    let summary_json = state.db.pool().with_conn(|conn| {
        let profile = storage::get_profile(conn, uid)
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))?;
        let locked = storage::get_locked_practice(conn, uid)
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))?;
        Ok(json!({
            "lockedPractice": locked.as_ref().map(|p| &p.title),
            "totalTransferBytes": profile.total_transfer_bytes,
            "totalPracticeMinutes": profile.total_practice_minutes,
            "totalChants": profile.total_practice_chants,
            "isVip": profile.is_vip,
            "locale": profile.locale,
        })
        .to_string())
    });

    if let Ok(summary_str) = summary_json {
        let mut entries = BTreeMap::new();
        entries.insert("fabushi_summary".to_string(), summary_str);
        let _ = state
            .db
            .put_cloud_storage_entries(uid, bot.bot_user_id, &entries);
    }

    let Some(text) = receipt_message.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };

    let allows_write = state
        .db
        .get_user_bot_permission(uid, bot.bot_user_id)
        .ok()
        .is_some_and(|p| p.allows_write_to_pm);
    if !allows_write {
        return;
    }

    let user_id = UserId(uid.max(0) as u64);
    let bot_uid = UserId(bot.bot_user_id.max(0) as u64);
    if state.db.is_user_blocked(bot_uid, user_id).unwrap_or(false) {
        return;
    }

    let companion_cid = state
        .db
        .find_companion_chat_id_for_user(bot.bot_user_id, uid)
        .ok()
        .flatten()
        .filter(|c| c.0 > 0)
        .or_else(|| {
            state
                .db
                .ensure_companion_bot_chat_record(user_id, bot_uid)
                .ok()
                .map(|(cid, _)| cid)
        })
        .unwrap_or(ChatId(0));

    if companion_cid.0 > 0 {
        let kb = vec![vec![InlineKeyboardButton {
            text: "🪷 打开 全球法布施".to_string(),
            callback_data: None,
            url: None,
            web_app_url: Some(format!("gpui-miniapp://{}/fabushi", bot.username)),
        }]];
        let _ = crate::routes::bots_miniapps::send_bot_chat_reply(
            state,
            companion_cid,
            bot_uid,
            text,
            kb,
        );
    }
}

fn format_targets_label(targets: &[String]) -> String {
    if targets.is_empty() || targets.iter().any(|c| c.eq_ignore_ascii_case("ALL")) {
        "全球所有节点".to_string()
    } else if targets.len() <= 4 {
        targets.join(", ")
    } else {
        format!("{} 等 {} 个国家/地区", targets[..3].join(", "), targets.len())
    }
}

pub async fn serve_app_wasm() -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/wasm"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        fabushi_miniapp::app_wasm_bytes(),
    )
        .into_response()
}

pub async fn serve_asset(Path(name): Path<String>) -> Response {
    let clean = name.trim().trim_start_matches('/');
    match fabushi_miniapp::asset_bytes(clean) {
        Some((bytes, content_type)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, "public, max-age=300"),
            ],
            bytes,
        )
            .into_response(),
        None => err_resp(StatusCode::NOT_FOUND, "asset_not_found", "Mini App asset not found"),
    }
}

pub async fn bootstrap(
    auth: FabushiAuth,
    State(state): State<Arc<AppState>>,
) -> Response {
    let profile = match ensure_fabushi_user(&state, &auth) {
        Ok(p) => p,
        Err(e) => return internal_err(&e),
    };
    let uid = auth.user_id.0 as i64;
    let bundle = state.db.pool().with_conn(|conn| {
        storage::get_bootstrap_bundle(
            conn,
            uid,
            &profile.username,
            &profile.display_name,
            &profile.avatar_url,
            profile.is_admin,
        )
        .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
    });
    match bundle {
        Ok(b) => (
            StatusCode::OK,
            Json(HttpResponsePayload::Bootstrap {
                profile: b.profile,
                locked_practice: b.locked_practice,
                leaderboard: b.leaderboard,
                practice_leaderboard: b.practice_leaderboard,
                practice_records: b.practice_records,
                practice_stats: b.practice_stats,
                unlocked_achievement_ids: b.unlocked_achievement_ids,
                online_global_sending_count: b.online_global_sending_count,
                online_zen_room_count: b.online_zen_room_count,
            }),
        )
            .into_response(),
        Err(e) => internal_err(&e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct TransferRequest {
    pub content_title: String,
    #[serde(default = "default_content_kind")]
    pub content_kind: String,
    #[serde(default = "default_target_countries")]
    pub target_countries: Vec<String>,
    #[serde(default)]
    pub bytes_sent: u64,
    #[serde(default)]
    pub beam_count: u32,
}

fn default_content_kind() -> String {
    "sutra".to_string()
}

fn default_target_countries() -> Vec<String> {
    vec!["ALL".to_string()]
}

pub async fn record_transfer_handler(
    auth: FabushiAuth,
    State(state): State<Arc<AppState>>,
    Json(req): Json<TransferRequest>,
) -> Response {
    let title = req.content_title.trim();
    if title.is_empty() {
        return bad_request("content_title must not be empty");
    }
    if let Err(e) = ensure_fabushi_user(&state, &auth) {
        return internal_err(&e);
    }
    let uid = auth.user_id.0 as i64;
    let targets = if req.target_countries.is_empty() {
        default_target_countries()
    } else {
        req.target_countries
    };
    let beams = req.beam_count.max(targets.len().max(1) as u32);
    let bytes = if req.bytes_sent > 0 {
        req.bytes_sent
    } else {
        sutra_by_title(title)
            .map(|s| s.content.len() as u64)
            .unwrap_or(1024)
            .saturating_mul(beams as u64)
            .max(1024)
    };
    let kind = if req.content_kind.trim().is_empty() {
        "sutra"
    } else {
        req.content_kind.trim()
    };

    let res = state.db.pool().with_conn(|conn| {
        storage::record_transfer(conn, uid, title, kind, &targets, bytes, beams)
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
    });

    match res {
        Ok(tr) => {
            let receipt = format!(
                "🌍 **全球法布施功德回执**\n• 布施内容：《{}》\n• 送达节点：{}（{} 束法光，本次 {}）\n• 累计法布施：{}（共 {} 次）\n愿法音遍传十方，一切有情同沾法喜 🙏",
                title,
                format_targets_label(&targets),
                beams,
                format_leaderboard_bytes(bytes),
                format_leaderboard_bytes(tr.total_transfer_bytes),
                tr.total_transfer_count
            );
            sync_summary_and_maybe_notify(&state, uid, auth.bot.as_ref(), Some(&receipt));
            (
                StatusCode::OK,
                Json(HttpResponsePayload::TransferRecorded {
                    total_transfer_bytes: tr.total_transfer_bytes,
                    total_transfer_count: tr.total_transfer_count,
                    leaderboard: Some(tr.leaderboard),
                }),
            )
                .into_response()
        }
        Err(e) => internal_err(&e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct PracticeSessionRequest {
    #[serde(alias = "sutra_name")]
    pub sutra: String,
    #[serde(default)]
    pub duration_minutes: u32,
    #[serde(default)]
    pub chant_count: u32,
    #[serde(default)]
    pub start_time_ms: Option<i64>,
    #[serde(default)]
    pub end_time_ms: Option<i64>,
    #[serde(default)]
    pub notes: Option<String>,
}

pub async fn practice_session_handler(
    auth: FabushiAuth,
    State(state): State<Arc<AppState>>,
    Json(req): Json<PracticeSessionRequest>,
) -> Response {
    let sutra = req.sutra.trim();
    if sutra.is_empty() {
        return bad_request("sutra must not be empty");
    }
    if let Err(e) = ensure_fabushi_user(&state, &auth) {
        return internal_err(&e);
    }
    let uid = auth.user_id.0 as i64;
    let res = state.db.pool().with_conn(|conn| {
        storage::record_practice_session(
            conn,
            uid,
            sutra,
            req.duration_minutes,
            req.chant_count,
            req.start_time_ms,
            req.end_time_ms,
            req.notes.as_deref(),
        )
        .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
    });

    match res {
        Ok(ps) => {
            let notes_suffix = ps
                .record
                .notes
                .as_deref()
                .map(|n| format!("\n• 修行感悟：{n}"))
                .unwrap_or_default();
            let receipt = format!(
                "🪷 **禅室共修功德回执**\n• 修行功课：《{}》\n• 本次记录：诵持 {} 遍 · {} 分钟\n• 累计共修：{} 次 · 连续 {} 天{}\n随喜赞叹，愿以此功德庄严佛净土 🙏",
                ps.record.sutra_name,
                ps.record.chant_count,
                ps.record.duration_minutes,
                ps.profile.total_practice_sessions,
                ps.profile.consecutive_days,
                notes_suffix
            );
            sync_summary_and_maybe_notify(&state, uid, auth.bot.as_ref(), Some(&receipt));
            (
                StatusCode::OK,
                Json(HttpResponsePayload::PracticeSessionSaved {
                    record: ps.record,
                    practice_stats: ps.practice_stats,
                    profile: ps.profile,
                }),
            )
                .into_response()
        }
        Err(e) => internal_err(&e.to_string()),
    }
}

pub async fn list_practice_records_handler(
    auth: FabushiAuth,
    State(state): State<Arc<AppState>>,
) -> Response {
    if let Err(e) = ensure_fabushi_user(&state, &auth) {
        return internal_err(&e);
    }
    let uid = auth.user_id.0 as i64;
    let res = state.db.pool().with_reader(|conn| {
        let records = storage::list_practice_records(conn, uid, 100, 0)
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))?;
        let stats = storage::compute_practice_stats(conn, uid)
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))?;
        Ok((records, stats))
    });
    match res {
        Ok((practice_records, practice_stats)) => (
            StatusCode::OK,
            Json(HttpResponsePayload::PracticeRecordsRefreshed {
                practice_records,
                practice_stats,
            }),
        )
            .into_response(),
        Err(e) => internal_err(&e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct ManualPracticeRecordRequest {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(alias = "sutra")]
    pub sutra_name: String,
    #[serde(default)]
    pub chant_count: u32,
    #[serde(default)]
    pub duration_minutes: u32,
    #[serde(default)]
    pub record_date: String,
    #[serde(default)]
    pub local_time: String,
    #[serde(default)]
    pub notes: Option<String>,
}

pub async fn create_practice_record_handler(
    auth: FabushiAuth,
    State(state): State<Arc<AppState>>,
    Json(req): Json<ManualPracticeRecordRequest>,
) -> Response {
    let sutra = req.sutra_name.trim();
    if sutra.is_empty() {
        return bad_request("sutra_name must not be empty");
    }
    if let Err(e) = ensure_fabushi_user(&state, &auth) {
        return internal_err(&e);
    }
    let uid = auth.user_id.0 as i64;
    let res = state.db.pool().with_conn(|conn| {
        let (_created, stats) = storage::create_manual_practice_record(
            conn,
            uid,
            sutra,
            req.chant_count,
            req.duration_minutes,
            &req.record_date,
            &req.local_time,
            req.notes.as_deref(),
        )
        .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))?;
        let records = storage::list_practice_records(conn, uid, 100, 0)
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))?;
        Ok((records, stats))
    });
    match res {
        Ok((practice_records, practice_stats)) => {
            sync_summary_and_maybe_notify(&state, uid, auth.bot.as_ref(), None);
            (
                StatusCode::OK,
                Json(HttpResponsePayload::PracticeRecordsRefreshed {
                    practice_records,
                    practice_stats,
                }),
            )
                .into_response()
        }
        Err(e) => internal_err(&e.to_string()),
    }
}

pub async fn update_practice_record_by_id(
    auth: FabushiAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(req): Json<ManualPracticeRecordRequest>,
) -> Response {
    update_practice_record_impl(&auth, &state, id, req).await
}

pub async fn update_practice_record_body(
    auth: FabushiAuth,
    State(state): State<Arc<AppState>>,
    Json(req): Json<ManualPracticeRecordRequest>,
) -> Response {
    let Some(id) = req.id else {
        return bad_request("record id is required");
    };
    update_practice_record_impl(&auth, &state, id, req).await
}

async fn update_practice_record_impl(
    auth: &FabushiAuth,
    state: &Arc<AppState>,
    id: i64,
    req: ManualPracticeRecordRequest,
) -> Response {
    let sutra = req.sutra_name.trim();
    if sutra.is_empty() {
        return bad_request("sutra_name must not be empty");
    }
    if let Err(e) = ensure_fabushi_user(state, auth) {
        return internal_err(&e);
    }
    let uid = auth.user_id.0 as i64;
    let res = state.db.pool().with_conn(|conn| {
        let updated = storage::update_practice_record(
            conn,
            uid,
            id,
            sutra,
            req.chant_count,
            req.duration_minutes,
            &req.record_date,
            &req.local_time,
            req.notes.as_deref(),
        )
        .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))?;
        if updated.is_none() {
            return Ok(None);
        }
        let records = storage::list_practice_records(conn, uid, 100, 0)
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))?;
        let stats = storage::compute_practice_stats(conn, uid)
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))?;
        Ok(Some((records, stats)))
    });
    match res {
        Ok(Some((practice_records, practice_stats))) => {
            sync_summary_and_maybe_notify(state, uid, auth.bot.as_ref(), None);
            (
                StatusCode::OK,
                Json(HttpResponsePayload::PracticeRecordsRefreshed {
                    practice_records,
                    practice_stats,
                }),
            )
                .into_response()
        }
        Ok(None) => not_found("Practice record not found"),
        Err(e) => internal_err(&e.to_string()),
    }
}

pub async fn delete_practice_record_handler(
    auth: FabushiAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Response {
    if let Err(e) = ensure_fabushi_user(&state, &auth) {
        return internal_err(&e);
    }
    let uid = auth.user_id.0 as i64;
    let res = state.db.pool().with_conn(|conn| {
        let (deleted, stats) = storage::delete_practice_record(conn, uid, id)
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))?;
        if !deleted {
            return Ok(None);
        }
        let records = storage::list_practice_records(conn, uid, 100, 0)
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))?;
        Ok(Some((records, stats)))
    });
    match res {
        Ok(Some((practice_records, practice_stats))) => {
            sync_summary_and_maybe_notify(&state, uid, auth.bot.as_ref(), None);
            (
                StatusCode::OK,
                Json(HttpResponsePayload::PracticeRecordsRefreshed {
                    practice_records,
                    practice_stats,
                }),
            )
                .into_response()
        }
        Ok(None) => not_found("Practice record not found"),
        Err(e) => internal_err(&e.to_string()),
    }
}

pub async fn get_profile_handler(
    auth: FabushiAuth,
    State(state): State<Arc<AppState>>,
) -> Response {
    let profile = match ensure_fabushi_user(&state, &auth) {
        Ok(p) => p,
        Err(e) => return internal_err(&e),
    };
    let uid = auth.user_id.0 as i64;
    let locked_practice = state
        .db
        .pool()
        .with_reader(|conn| {
            storage::get_locked_practice(conn, uid)
                .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
        })
        .unwrap_or(None);
    (
        StatusCode::OK,
        Json(HttpResponsePayload::ProfileUpdated {
            profile,
            locked_practice,
        }),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct UpdateProfileRequest {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub bio: Option<String>,
    #[serde(default)]
    pub locked_practice: Option<LockedPractice>,
    #[serde(default)]
    pub locale: Option<String>,
}

pub async fn update_profile_handler(
    auth: FabushiAuth,
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateProfileRequest>,
) -> Response {
    if let Err(e) = ensure_fabushi_user(&state, &auth) {
        return internal_err(&e);
    }
    let uid = auth.user_id.0 as i64;
    let res = state.db.pool().with_conn(|conn| {
        storage::update_profile(
            conn,
            uid,
            req.display_name.as_deref(),
            req.bio.as_deref(),
            req.locked_practice.as_ref(),
            req.locale.as_deref(),
        )
        .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
    });

    match res {
        Ok((profile, locked_practice)) => {
            sync_summary_and_maybe_notify(&state, uid, auth.bot.as_ref(), None);
            (
                StatusCode::OK,
                Json(HttpResponsePayload::ProfileUpdated {
                    profile,
                    locked_practice,
                }),
            )
                .into_response()
        }
        Err(e) => internal_err(&e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct ActivateMembershipRequest {
    #[serde(default = "default_vip_plan")]
    pub plan: String,
}

fn default_vip_plan() -> String {
    "yearly".to_string()
}

pub async fn activate_membership_handler(
    auth: FabushiAuth,
    State(state): State<Arc<AppState>>,
    Json(req): Json<ActivateMembershipRequest>,
) -> Response {
    if let Err(e) = ensure_fabushi_user(&state, &auth) {
        return internal_err(&e);
    }
    let uid = auth.user_id.0 as i64;
    let plan_norm = match req.plan.trim().to_ascii_lowercase().as_str() {
        "monthly" => "monthly",
        "quarterly" => "quarterly",
        _ => "yearly",
    };
    let days: i64 = match plan_norm {
        "monthly" => 30,
        "quarterly" => 90,
        _ => 365,
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    let new_expiry_ms = now_ms + days * 86_400_000;

    let res = state.db.pool().with_conn(|conn| {
        let profile = storage::activate_membership(conn, uid, plan_norm)
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))?;
        let _ = conn.execute(
            r#"
            INSERT INTO user_memberships (user_id, is_active, membership_type, expires_at_ms, updated_at_ms)
            VALUES (?1, 1, ?2, ?3, ?4)
            ON CONFLICT(user_id) DO UPDATE SET
                is_active = 1,
                membership_type = excluded.membership_type,
                expires_at_ms = CASE
                    WHEN user_memberships.expires_at_ms IS NOT NULL AND user_memberships.expires_at_ms > ?4
                    THEN user_memberships.expires_at_ms + ?5
                    ELSE excluded.expires_at_ms
                END,
                updated_at_ms = excluded.updated_at_ms
            "#,
            params![uid, plan_norm, new_expiry_ms, now_ms, days * 86_400_000],
        );
        Ok(profile)
    });

    match res {
        Ok(profile) => {
            sync_summary_and_maybe_notify(&state, uid, auth.bot.as_ref(), None);
            (
                StatusCode::OK,
                Json(HttpResponsePayload::MembershipActivated { profile }),
            )
                .into_response()
        }
        Err(e) => internal_err(&e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct FollowRequest {
    pub target_username: String,
}

pub async fn follow_handler(
    auth: FabushiAuth,
    State(state): State<Arc<AppState>>,
    Json(req): Json<FollowRequest>,
) -> Response {
    let target = req.target_username.trim();
    if target.is_empty() {
        return bad_request("target_username must not be empty");
    }
    if let Err(e) = ensure_fabushi_user(&state, &auth) {
        return internal_err(&e);
    }
    let uid = auth.user_id.0 as i64;
    let res = state.db.pool().with_conn(|conn| {
        storage::toggle_follow(conn, uid, target)
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
    });
    match res {
        Ok(f) => (
            StatusCode::OK,
            Json(HttpResponsePayload::FollowToggled {
                target_username: f.target_username,
                is_following: f.is_following,
                follower_count: f.follower_count,
            }),
        )
            .into_response(),
        Err(e) => internal_err(&e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct LeaderboardQuery {
    #[serde(default = "default_leaderboard_kind")]
    pub kind: String,
    #[serde(default = "default_leaderboard_limit")]
    pub limit: usize,
}

fn default_leaderboard_kind() -> String {
    "global".to_string()
}

fn default_leaderboard_limit() -> usize {
    50
}

pub async fn leaderboard_handler(
    auth: FabushiAuth,
    State(state): State<Arc<AppState>>,
    Query(q): Query<LeaderboardQuery>,
) -> Response {
    if let Err(e) = ensure_fabushi_user(&state, &auth) {
        return internal_err(&e);
    }
    let uid = auth.user_id.0 as i64;
    let limit = q.limit.clamp(1, 100);
    let kind_norm = if q.kind.trim().eq_ignore_ascii_case("practice") {
        "practice".to_string()
    } else {
        "global".to_string()
    };

    let res = state.db.pool().with_reader(|conn| {
        let entries = if kind_norm == "practice" {
            storage::get_practice_leaderboard(conn, uid, limit)
        } else {
            storage::get_global_leaderboard(conn, uid, limit)
        }
        .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))?;
        Ok(entries)
    });

    match res {
        Ok(entries) => (
            StatusCode::OK,
            Json(HttpResponsePayload::LeaderboardFetched {
                kind: kind_norm,
                entries,
            }),
        )
            .into_response(),
        Err(e) => internal_err(&e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct HeartbeatRequest {
    #[serde(default = "default_activity_type")]
    pub activity_type: String,
}

fn default_activity_type() -> String {
    "global_sending".to_string()
}

pub async fn heartbeat_handler(
    auth: FabushiAuth,
    State(state): State<Arc<AppState>>,
    Json(req): Json<HeartbeatRequest>,
) -> Response {
    if let Err(e) = ensure_fabushi_user(&state, &auth) {
        return internal_err(&e);
    }
    let uid = auth.user_id.0 as i64;
    let activity = if req.activity_type.trim() == "zen_room" {
        "zen_room".to_string()
    } else {
        "global_sending".to_string()
    };
    let res = state.db.pool().with_conn(|conn| {
        storage::upsert_online_heartbeat(conn, uid, &activity)
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
    });
    match res {
        Ok(online_count) => (
            StatusCode::OK,
            Json(HttpResponsePayload::HeartbeatAck {
                activity_type: activity,
                online_count,
            }),
        )
            .into_response(),
        Err(e) => internal_err(&e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct AiChatRequest {
    #[serde(alias = "prompt")]
    pub message: String,
    #[serde(default)]
    pub history: Vec<AiChatMessage>,
}

pub async fn ai_chat_handler(
    auth: FabushiAuth,
    State(state): State<Arc<AppState>>,
    Json(req): Json<AiChatRequest>,
) -> Response {
    let question = req.message.trim().to_string();
    if question.is_empty() {
        return bad_request("message must not be empty");
    }

    let profile = match ensure_fabushi_user(&state, &auth) {
        Ok(p) => p,
        Err(e) => return internal_err(&e),
    };
    let uid = auth.user_id.0 as i64;
    let locked_title = state
        .db
        .pool()
        .with_conn(|conn| {
            storage::get_locked_practice(conn, uid)
                .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
        })
        .ok()
        .flatten()
        .map(|lp| lp.title);

    let bot_id_opt = auth
        .bot
        .as_ref()
        .map(|b| b.bot_user_id.max(0) as u64);
    let hermes = crate::routes::hermes::resolve_hermes_for_bot(&state, bot_id_opt);
    let snap = hermes.providers().snapshot();
    let has_external_llm = hermes.providers().has_configured_provider()
        || hermes.providers().has_active_subscription()
        || snap.providers.get(&snap.active_provider_id).is_some_and(|cfg| {
            cfg.enabled
                && !matches!(cfg.api_mode, ApiMode::LocalHermes)
                && (!cfg.credentials.is_empty() || cfg.no_key_required)
        })
        || (snap.active_model.starts_with("group/")
            && snap
                .routing_groups
                .contains_key(snap.active_model.trim_start_matches("group/")));

    if has_external_llm {
        let mut ctx_lines = vec![DEFAULT_AI_SYSTEM_PROMPT.to_string()];
        if let Some(ref lp) = locked_title {
            ctx_lines.push(format!("用户当前一门深入的主修功课：《{lp}》。"));
        }
        if !req.history.is_empty() {
            ctx_lines.push("最近对话摘录：".to_string());
            for msg in req.history.iter().rev().take(6).rev() {
                let role = if msg.is_user { "行者" } else { "法布施助手" };
                ctx_lines.push(format!("{role}: {}", msg.text.trim()));
            }
        }
        let system_prompt = ctx_lines.join("\n");
        let mut turn_req = HermesTurnRequest::new(question.clone())
            .with_session(format!("fabushi-miniapp-u{uid}"))
            .with_system_prompt(system_prompt);
        turn_req.user_id = Some(auth.user_id.0.to_string());
        if let Some(ref b) = auth.bot {
            turn_req.bot_id = Some(b.bot_user_id.to_string());
            turn_req.bot_name = Some(b.display_name.clone());
        }
        if let Ok(result) = hermes.run_turn(turn_req).await {
            let reply = result.response_text.trim().to_string();
            if !reply.is_empty() {
                return (
                    StatusCode::OK,
                    Json(HttpResponsePayload::AiChatReply { reply }),
                )
                    .into_response();
            }
        }
    }

    let fallback = dharma_ai_fallback_reply(
        &question,
        locked_title.as_deref(),
        &profile.locale,
    );
    (
        StatusCode::OK,
        Json(HttpResponsePayload::AiChatReply { reply: fallback }),
    )
        .into_response()
}

/// Headless GPUI Mini App Schema fallback reducer for `POST /api/v1/miniapps/{bot}/fabushi/action`.
pub fn handle_headless_fabushi_action(
    state: &Arc<AppState>,
    bot: &BotRegistryRecord,
    _app: &MiniAppRecord,
    user_id: UserId,
    req: &MiniAppActionRequest,
    current_state: Value,
) -> (Value, String, Option<String>, bool) {
    let mut map = match current_state {
        Value::Object(m) => m,
        _ => Map::new(),
    };

    let uid = user_id.0 as i64;
    let user_opt = state.db.get_user(user_id).ok().flatten();
    let username = user_opt
        .as_ref()
        .and_then(|u| u.username.clone())
        .unwrap_or_else(|| format!("user_{uid}"));
    let display_name = user_opt
        .as_ref()
        .map(|u| {
            format!(
                "{} {}",
                u.first_name.trim(),
                u.last_name.as_deref().unwrap_or("").trim()
            )
            .trim()
            .to_string()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| username.clone());
    let is_admin = username == "fabushi_official_publisher" || user_id.0 == 1;

    let _ = state.db.pool().with_conn(|conn| {
        storage::ensure_profile(conn, uid, &username, &display_name, "", is_admin)
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
    });

    let action = req.action_type.trim();
    if action == "widget_event" {
        if let Some(wid) = req.widget_id.as_deref() {
            let val = req.value.clone().unwrap_or(Value::Null);
            map.insert(wid.to_string(), val);
        }
        return (
            Value::Object(map),
            "已更新全球法布施无头工作区参数".to_string(),
            None,
            false,
        );
    }

    let sutra = map
        .get("sutra")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("心经")
        .to_string();
    let mode = map
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("全球法布施")
        .to_string();
    let target_code = map
        .get("target_country")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("ALL")
        .to_string();
    let chants = map
        .get("chant_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(108)
        .max(1) as u32;

    if matches!(action, "secondary_button_click" | "send_data") {
        let profile = state
            .db
            .pool()
            .with_conn(|conn| {
                storage::get_profile(conn, uid)
                    .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
            })
            .unwrap_or_default();
        sync_summary_and_maybe_notify(state, uid, Some(bot), None);
        let summary_text = format!(
            "🪷 来自小程序 **全球法布施** 的功德同步：\n• 累计法布施：{}（{} 次）\n• 禅室共修：{} 次（{} 遍，连续 {} 天）\n• 主修功课：{}",
            format_leaderboard_bytes(profile.total_transfer_bytes),
            profile.total_transfer_count,
            profile.total_practice_sessions,
            profile.total_practice_chants,
            profile.consecutive_days,
            profile
                .locked_practice_title
                .as_deref()
                .unwrap_or("尚未锁定（自由共修）")
        );
        map.insert("ai_summary".into(), json!(summary_text));
        return (
            Value::Object(map),
            "已同步法布施功德摘要到伴生机器人对话".to_string(),
            Some(summary_text),
            action == "send_data",
        );
    }

    if mode.contains("禅室") || action.contains("practice") {
        let saved = state.db.pool().with_conn(|conn| {
            storage::record_practice_session(
                conn,
                uid,
                &sutra,
                15,
                chants,
                None,
                None,
                Some("GPUI 无头模式共修记录"),
            )
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
        });
        if let Ok(ps) = saved {
            sync_summary_and_maybe_notify(state, uid, Some(bot), None);
            let toast = format!(
                "🪷 已记录《{}》禅室共修 {} 遍（累计 {} 次）",
                ps.record.sutra_name, ps.record.chant_count, ps.profile.total_practice_sessions
            );
            map.insert("total_practice_sessions".into(), json!(ps.profile.total_practice_sessions));
            map.insert("total_practice_chants".into(), json!(ps.profile.total_practice_chants));
            map.insert("ai_summary".into(), json!(toast));
            return (Value::Object(map), toast.clone(), Some(toast), false);
        }
    }

    let targets = vec![target_code.clone()];
    let bytes = sutra_by_title(&sutra)
        .map(|s| s.content.len() as u64)
        .unwrap_or(1024);
    let tr = state.db.pool().with_conn(|conn| {
        storage::record_transfer(conn, uid, &sutra, "sutra", &targets, bytes, 1)
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
    });
    match tr {
        Ok(res) => {
            sync_summary_and_maybe_notify(state, uid, Some(bot), None);
            let toast = format!(
                "🌍 已向 {} 节点发送《{}》法布施（累计 {}）",
                format_targets_label(&targets),
                sutra,
                format_leaderboard_bytes(res.total_transfer_bytes)
            );
            map.insert("total_transfer_bytes".into(), json!(res.total_transfer_bytes));
            map.insert("total_transfer_count".into(), json!(res.total_transfer_count));
            map.insert("ai_summary".into(), json!(toast));
            (Value::Object(map), toast.clone(), Some(toast), false)
        }
        Err(e) => (
            Value::Object(map),
            format!("法布施记录保存失败: {e}"),
            None,
            false,
        ),
    }
}
