use crate::auth::session_manager::{generate_verification_code, hash_verification_code};
use crate::auth::{hash_password, verify_password, AuthBearer};
use crate::db::auth_store::PhoneChallengeRecord;
use crate::db::schema::UserRecord;
use crate::routes::{err_resp, issue_persistent_session, map_telegram_err};
use crate::server::AppState;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use chrono::Utc;
use gpui_telegram_core::types::UserId;
use gpui_telegram_proto::contracts::{
    PhoneSendCodeRequest, PhoneSendCodeResponse, PhoneVerifyRequest, TwoFactorSetRequest,
    TwoFactorVerifyRequest,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

fn normalize_phone(raw: &str) -> String {
    raw.trim().chars().filter(|c| c.is_ascii_digit() || *c == '+').collect()
}

pub async fn phone_send_code(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PhoneSendCodeRequest>,
) -> Response {
    let phone = normalize_phone(&req.phone);
    if phone.len() < 7 || phone.len() > 20 {
        return err_resp(StatusCode::BAD_REQUEST, "invalid_phone", "Valid phone number is required");
    }
    let now_ms = Utc::now().timestamp_millis();
    if let Ok(Some(prev)) = state.db.get_latest_phone_challenge(&phone) {
        if now_ms < prev.cooldown_until_ms {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({ "error": "rate_limited", "resend_after_ms": prev.cooldown_until_ms })),
            )
                .into_response();
        }
    }
    let code = generate_verification_code();
    let code_hash = hash_verification_code(&code);
    let phone_code_hash = Uuid::new_v4().simple().to_string();
    let cooldown_until_ms = now_ms + 60_000;
    let expires_at_ms = now_ms + 300_000;
    let rec = PhoneChallengeRecord {
        challenge_id: Uuid::new_v4().to_string(),
        phone: phone.clone(),
        phone_code_hash: phone_code_hash.clone(),
        code_hash,
        attempts_left: 5,
        cooldown_until_ms,
        expires_at_ms,
        created_at_ms: now_ms,
    };
    if let Err(e) = state.db.save_phone_challenge(&rec) {
        return map_telegram_err(e);
    }
    let debug_code = if std::env::var("GPUI_HIDE_DEBUG_CODE").is_ok() { None } else { Some(code) };
    (
        StatusCode::OK,
        Json(PhoneSendCodeResponse {
            ok: true,
            sent: true,
            phone_code_hash,
            expires_at_ms,
            resend_after_ms: cooldown_until_ms,
            debug_code,
        }),
    )
        .into_response()
}

pub async fn phone_verify(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PhoneVerifyRequest>,
) -> Response {
    let phone = normalize_phone(&req.phone);
    if phone.is_empty() || req.code.trim().is_empty() {
        return err_resp(StatusCode::BAD_REQUEST, "invalid_request", "Phone and verification code required");
    }
    let challenge = match state.db.get_latest_phone_challenge(&phone) {
        Ok(Some(c)) => c,
        Ok(None) => return err_resp(StatusCode::BAD_REQUEST, "challenge_not_found", "Request a phone code first"),
        Err(e) => return map_telegram_err(e),
    };
    let now_ms = Utc::now().timestamp_millis();
    if now_ms > challenge.expires_at_ms {
        let _ = state.db.delete_phone_challenge(&challenge.challenge_id);
        return err_resp(StatusCode::BAD_REQUEST, "code_expired", "Verification code expired");
    }
    if let Some(ref provided_hash) = req.phone_code_hash {
        if provided_hash != &challenge.phone_code_hash {
            return err_resp(StatusCode::BAD_REQUEST, "invalid_phone_code_hash", "phone_code_hash mismatch");
        }
    }
    if challenge.attempts_left <= 0 {
        return err_resp(StatusCode::TOO_MANY_REQUESTS, "too_many_attempts", "Maximum verification attempts exceeded");
    }
    let supplied_hash = hash_verification_code(req.code.trim());
    if supplied_hash != challenge.code_hash {
        let left = state.db.decrement_phone_challenge_attempts(&challenge.challenge_id).unwrap_or(0);
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid_code", "attempts_left": left })),
        )
            .into_response();
    }
    let _ = state.db.delete_phone_challenge(&challenge.challenge_id);

    let existing = state.db.get_user_by_phone(&phone).ok().flatten();
    let user = match existing {
        Some(u) => u,
        None => {
            let uid = UserId(state.session_manager.allocate_user_id().raw());
            let digits: String = phone.chars().filter(|c| c.is_ascii_digit()).collect();
            let uname = req.username.clone().filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| format!("tg_{}", digits));
            let first = req.first_name.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| uname.clone());
            let pass_hash = req.password.as_deref().filter(|p| !p.trim().is_empty())
                .and_then(|p| hash_password(p).ok());
            let new_u = UserRecord {
                user_id: uid, username: Some(uname), phone: Some(phone.clone()), first_name: first,
                last_name: req.last_name.clone(), bio: None, password_hash: pass_hash,
                is_bot: false, is_verified: false, is_support: false, is_scam: false, is_fake: false,
                profile_photo_file_id: None, account_generation: 1, created_at_ms: now_ms, updated_at_ms: now_ms,
            };
            if let Err(e) = state.db.create_user(&new_u) {
                return map_telegram_err(e);
            }
            new_u
        }
    };
    if let Ok(status) = state.db.get_two_factor_status(user.user_id) {
        if status.has_password {
            let challenge_token = format!("2fa_{}", Uuid::new_v4().simple());
            let _ = state.db.create_two_factor_challenge(&challenge_token, user.user_id, now_ms + 300_000);
            return (
                StatusCode::OK,
                Json(json!({ "requires_2fa": true, "challenge_token": challenge_token, "hint": status.hint })),
            )
                .into_response();
        }
    }
    let dev = req.device_name.as_deref().unwrap_or("GPUI Desktop");
    (StatusCode::OK, Json(issue_persistent_session(&state, &user, dev))).into_response()
}

pub async fn two_factor_status(auth: AuthBearer, State(state): State<Arc<AppState>>) -> Response {
    match state.db.get_two_factor_status(auth.user_id) {
        Ok(s) => (StatusCode::OK, Json(s)).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn two_factor_set(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<TwoFactorSetRequest>,
) -> Response {
    if req.new_password.trim().is_empty() {
        return err_resp(StatusCode::BAD_REQUEST, "invalid_password", "Cloud password cannot be empty");
    }
    if let Ok(Some(existing_hash)) = state.db.get_two_factor_hash(auth.user_id) {
        let cur = req.current_password.as_deref().unwrap_or("");
        if !verify_password(cur, &existing_hash) {
            return err_resp(StatusCode::UNAUTHORIZED, "invalid_current_password", "Current cloud password incorrect");
        }
    }
    let hash = match hash_password(&req.new_password) {
        Ok(h) => h,
        Err(e) => return err_resp(StatusCode::BAD_REQUEST, "hash_error", &e.to_string()),
    };
    if let Err(e) = state.db.set_two_factor_auth(auth.user_id, &hash, req.hint.as_deref(), req.recovery_email.as_deref()) {
        return map_telegram_err(e);
    }
    match state.db.get_two_factor_status(auth.user_id) {
        Ok(s) => (StatusCode::OK, Json(s)).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

#[derive(Debug, Deserialize)]
pub struct TwoFactorDisableReq {
    pub current_password: String,
}

pub async fn two_factor_disable(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<TwoFactorDisableReq>,
) -> Response {
    if let Ok(Some(existing_hash)) = state.db.get_two_factor_hash(auth.user_id) {
        if !verify_password(&req.current_password, &existing_hash) {
            return err_resp(StatusCode::UNAUTHORIZED, "invalid_current_password", "Current cloud password incorrect");
        }
    }
    if let Err(e) = state.db.delete_two_factor_auth(auth.user_id) {
        return map_telegram_err(e);
    }
    (StatusCode::OK, Json(json!({ "has_password": false }))).into_response()
}

pub async fn two_factor_verify(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TwoFactorVerifyRequest>,
) -> Response {
    let (user_id, attempts_left, expires_at_ms) = match state.db.verify_two_factor_challenge(&req.challenge_token) {
        Ok(Some(row)) => row,
        Ok(None) => return err_resp(StatusCode::UNAUTHORIZED, "invalid_challenge", "2FA challenge not found"),
        Err(e) => return map_telegram_err(e),
    };
    if Utc::now().timestamp_millis() > expires_at_ms || attempts_left <= 0 {
        let _ = state.db.decrement_or_delete_two_factor_challenge(&req.challenge_token, true);
        return err_resp(StatusCode::UNAUTHORIZED, "challenge_expired", "2FA challenge expired or exhausted");
    }
    let Some(hash) = state.db.get_two_factor_hash(user_id).ok().flatten() else {
        return err_resp(StatusCode::BAD_REQUEST, "no_2fa", "2FA not configured for account");
    };
    if !verify_password(&req.cloud_password, &hash) {
        let _ = state.db.decrement_or_delete_two_factor_challenge(&req.challenge_token, false);
        return err_resp(StatusCode::UNAUTHORIZED, "invalid_cloud_password", "Incorrect 2FA cloud password");
    }
    let _ = state.db.decrement_or_delete_two_factor_challenge(&req.challenge_token, true);
    let Some(user) = state.db.get_user(user_id).ok().flatten() else {
        return err_resp(StatusCode::NOT_FOUND, "user_not_found", "User account missing");
    };
    let dev = req.device_name.as_deref().unwrap_or("GPUI Desktop");
    (StatusCode::OK, Json(issue_persistent_session(&state, &user, dev))).into_response()
}
