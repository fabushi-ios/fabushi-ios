use crate::db::schema::DevicePushTokenRecord;
use crate::push::PushPlatform;
use crate::server::{AppState, AuthBearer};
use axum::{
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct RegisterPushPayload {
    pub platform: String,
    pub token: String,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub p256dh: Option<String>,
    #[serde(default)]
    pub auth_secret: Option<String>,
    #[serde(default)]
    pub app_version: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UnregisterPushPayload {
    pub token: String,
}

pub async fn handle_register_push(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let payload = match serde_json::from_slice::<RegisterPushPayload>(&body) {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "success": false,
                    "error": "invalid_payload",
                    "message": format!("Malformed register_push payload: {e}")
                })),
            )
                .into_response();
        }
    };

    if payload.token.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": "empty_token",
                "message": "Push device token cannot be empty"
            })),
        )
            .into_response();
    }

    let platform = match PushPlatform::from_str_loose(&payload.platform) {
        Some(p) => p,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "success": false,
                    "error": "unsupported_platform",
                    "message": format!("Unsupported push platform: '{}'", payload.platform)
                })),
            )
                .into_response();
        }
    };

    let now_ms = chrono::Utc::now().timestamp_millis();
    let record = DevicePushTokenRecord {
        token: payload.token.clone(),
        user_id: auth.user_id,
        platform: platform.as_str().to_string(),
        device_id: payload.device_id,
        endpoint: payload.endpoint,
        p256dh: payload.p256dh,
        auth_secret: payload.auth_secret,
        app_version: payload.app_version,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    };

    match state.db.save_device_push_token(&record) {
        Ok(_) => {
            let token_preview = match payload.token.char_indices().nth(8) {
                Some((idx, _)) => format!("{}...", &payload.token[..idx]),
                None => payload.token.clone(),
            };

            (
                StatusCode::OK,
                Json(json!({
                    "success": true,
                    "registered": true,
                    "user_id": auth.user_id.0,
                    "platform": platform.as_str(),
                    "token_preview": token_preview,
                    "updated_at_ms": now_ms,
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": "database_error",
                "message": format!("Failed to register push token: {e}")
            })),
        )
            .into_response(),
    }
}

pub async fn handle_unregister_push(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let payload = match serde_json::from_slice::<UnregisterPushPayload>(&body) {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "success": false,
                    "error": "invalid_payload",
                    "message": format!("Malformed unregister payload: {e}")
                })),
            )
                .into_response();
        }
    };

    match state.db.delete_device_push_token(auth.user_id, &payload.token) {
        Ok(deleted) => (
            StatusCode::OK,
            Json(json!({
                "success": true,
                "deleted": deleted,
                "user_id": auth.user_id.0
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": "database_error",
                "message": format!("Failed to delete push token: {e}")
            })),
        )
            .into_response(),
    }
}
