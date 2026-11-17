use crate::routes::{err_resp, map_telegram_err};
use crate::server::{AppState, AuthBearer};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use base64::{engine::general_purpose::STANDARD as BASE64_STD, Engine as _};
use chrono::Utc;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use gpui_telegram_core::types::UserId;
use gpui_telegram_proto::e2ee::{
    PreKeyCountResponse, PublishPreKeysRequest, PublishPreKeysResponse, UserPreKeyBundlesResponse,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

const SPK_SIGN_DOMAIN: &[u8] = b"gpui-e2ee-spk-v1";

fn decode_fixed_b64<const N: usize>(val: &str, field: &str) -> Result<[u8; N], Response> {
    let bytes = BASE64_STD
        .decode(val.trim())
        .map_err(|_| err_resp(StatusCode::BAD_REQUEST, "invalid_key_encoding", &format!("{field} must be valid base64")))?;
    if bytes.len() != N {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_key_length",
            &format!("{field} must decode to exactly {N} bytes"),
        ));
    }
    let mut out = [0u8; N];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn resolve_device_id(explicit: Option<&str>, headers: &HeaderMap, session_dev: &str) -> String {
    if let Some(s) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return s.to_string();
    }
    if let Some(hdr) = headers
        .get("x-device-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return hdr.to_string();
    }
    session_dev.trim().to_string()
}

pub async fn publish_prekeys(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<PublishPreKeysRequest>,
) -> Response {
    let device_id = resolve_device_id(Some(&req.device_id), &headers, &auth.session.device_name);
    if device_id.is_empty() || device_id.len() > 128 {
        return err_resp(StatusCode::BAD_REQUEST, "invalid_device_id", "Valid device_id is required");
    }

    let _ik_bytes = match decode_fixed_b64::<32>(&req.identity_key, "identity_key") {
        Ok(b) => b,
        Err(r) => return r,
    };
    let signing_bytes = match decode_fixed_b64::<32>(&req.identity_signing_key, "identity_signing_key") {
        Ok(b) => b,
        Err(r) => return r,
    };
    let spk_pub_bytes = match decode_fixed_b64::<32>(&req.signed_prekey.public_key, "signed_prekey.public_key") {
        Ok(b) => b,
        Err(r) => return r,
    };
    let spk_sig_bytes = match decode_fixed_b64::<64>(&req.signed_prekey.signature, "signed_prekey.signature") {
        Ok(b) => b,
        Err(r) => return r,
    };

    let Ok(verifying_key) = VerifyingKey::from_bytes(&signing_bytes) else {
        return err_resp(StatusCode::BAD_REQUEST, "invalid_signing_key", "Malformed Ed25519 identity_signing_key");
    };
    let signature = Signature::from_bytes(&spk_sig_bytes);
    let mut signed_msg = Vec::with_capacity(SPK_SIGN_DOMAIN.len() + 32);
    signed_msg.extend_from_slice(SPK_SIGN_DOMAIN);
    signed_msg.extend_from_slice(&spk_pub_bytes);
    if verifying_key.verify(&signed_msg, &signature).is_err() {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_spk_signature",
            "Signed prekey Ed25519 signature verification failed",
        );
    }

    for otpk in &req.one_time_prekeys {
        if let Err(r) = decode_fixed_b64::<32>(&otpk.public_key, "one_time_prekey.public_key") {
            return r;
        }
    }

    let now_ms = Utc::now().timestamp_millis();
    match state.db.upsert_device_prekeys(auth.user_id, &device_id, &req, now_ms) {
        Ok(remaining) => (
            StatusCode::OK,
            Json(PublishPreKeysResponse {
                success: true,
                user_id: auth.user_id.0,
                device_id,
                remaining_one_time_prekeys: remaining,
                updated_at_ms: now_ms,
            }),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn fetch_user_prekey_bundles(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(target_user_id): Path<u64>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    if target_user_id == 0 {
        return err_resp(StatusCode::BAD_REQUEST, "invalid_user_id", "user_id must be positive");
    }
    match state.db.get_user(UserId(target_user_id)) {
        Ok(Some(_)) => {}
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, "user_not_found", "Target user not found"),
        Err(e) => return map_telegram_err(e),
    }
    let device_filter = params
        .get("device_id")
        .map(String::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .consume_user_prekey_bundles(UserId(target_user_id), device_filter, now_ms)
    {
        Ok(devices) if devices.is_empty() => err_resp(
            StatusCode::NOT_FOUND,
            "prekeys_not_found",
            "No active E2EE prekey bundles found for user",
        ),
        Ok(devices) => (
            StatusCode::OK,
            Json(UserPreKeyBundlesResponse {
                user_id: target_user_id,
                devices,
            }),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn prekey_count(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let device_id = resolve_device_id(
        params.get("device_id").map(String::as_str),
        &headers,
        &auth.session.device_name,
    );
    match state.db.count_device_one_time_prekeys(auth.user_id, &device_id) {
        Ok(Some((remaining, has_spk))) => (
            StatusCode::OK,
            Json(PreKeyCountResponse {
                user_id: auth.user_id.0,
                device_id,
                remaining_one_time_prekeys: remaining,
                has_signed_prekey: has_spk,
            }),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::OK,
            Json(PreKeyCountResponse {
                user_id: auth.user_id.0,
                device_id,
                remaining_one_time_prekeys: 0,
                has_signed_prekey: false,
            }),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn revoke_device(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<String>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .deactivate_e2ee_device(auth.user_id, device_id.trim(), now_ms)
    {
        Ok(true) => (StatusCode::OK, Json(json!({ "revoked": true, "device_id": device_id }))).into_response(),
        Ok(false) => err_resp(StatusCode::NOT_FOUND, "device_not_found", "Active E2EE device not found"),
        Err(e) => map_telegram_err(e),
    }
}
