use crate::db::social_store::MediaMetadataDto;
use crate::media::metadata::extract_media_metadata;
use crate::routes::{check_active_membership, err_resp, map_telegram_err, parse_json_object};
use crate::server::{AppState, AuthBearer};
use axum::{body::Bytes, extract::{Path, Query, State}, http::{header, HeaderMap, StatusCode}, response::{IntoResponse, Json, Response}};
use base64::Engine;
use chrono::Utc;
use gpui_telegram_core::types::ChatId;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

fn extract_multipart_part(body: &[u8], content_type: &str) -> Option<(Vec<u8>, Option<String>, Option<String>)> {
    let b_pos = content_type.find("boundary=")?;
    let boundary = content_type[b_pos + 9..].split(';').next()?.trim().trim_matches('"');
    if boundary.is_empty() { return None; }
    let delim = format!("--{boundary}").into_bytes();
    let start = body.windows(delim.len()).position(|w| w == delim.as_slice())? + delim.len();
    let rest = &body[start..];
    let sep_pos = rest.windows(4).position(|w| w == b"\r\n\r\n")?;
    let headers_str = String::from_utf8_lossy(&rest[..sep_pos]);
    let payload_start = &rest[sep_pos + 4..];
    let end_pos = payload_start.windows(delim.len()).position(|w| w == delim.as_slice()).unwrap_or(payload_start.len());
    let data = payload_start[..end_pos].strip_suffix(b"\r\n").unwrap_or(&payload_start[..end_pos]).to_vec();
    let filename = headers_str.split("filename=\"").nth(1).and_then(|s| s.split('"').next()).map(String::from);
    let mime = headers_str.lines().find(|l| l.to_ascii_lowercase().starts_with("content-type:"))
        .and_then(|l| l.split_once(':')).map(|(_, v)| v.trim().to_string());
    Some((data, filename, mime))
}

fn persist_media_meta(state: &AppState, auth: &AuthBearer, sha256: &str, data: &[u8], filename: &str, mime_type: &str, is_voice: bool, is_video_note: bool, chat_id: Option<ChatId>) -> MediaMetadataDto {
    let meta = extract_media_metadata(data, mime_type, is_voice, is_video_note);
    let dto = MediaMetadataDto {
        media_id: sha256.to_string(), filename: filename.to_string(), mime_type: mime_type.to_string(),
        size_bytes: data.len() as u64, sha256: sha256.to_string(), width: meta.width, height: meta.height,
        duration_seconds: meta.duration_seconds, waveform: meta.waveform, thumbnail_data_url: meta.thumbnail_data_url,
        is_voice, is_video_note, created_at_ms: Utc::now().timestamp_millis(),
    };
    let _ = state.db.save_media_file(&dto, auth.user_id, chat_id);
    if let Some(cid) = chat_id {
        let mut searchable = format!("{} {}", filename, mime_type);
        if mime_type.starts_with("text/") || mime_type == "application/json" || mime_type == "application/markdown" {
            if let Ok(text_preview) = std::str::from_utf8(&data[..data.len().min(4096)]) {
                searchable.push(' ');
                searchable.push_str(text_preview);
            }
        }
        state.hybrid_memory.index_message_or_file(
            &format!("file:{sha256}"),
            cid.0,
            "file",
            &searchable,
            vec![filename.to_string()],
        );
    }
    dto
}

fn meta_json(dto: &MediaMetadataDto, upload_id: Option<&str>) -> Value {
    let waveform_b64 = dto.waveform.as_ref().map(|w| base64::engine::general_purpose::STANDARD.encode(w));
    json!({
        "upload_id": upload_id, "file_id": dto.media_id, "media_id": dto.media_id, "sha256": dto.sha256,
        "size_bytes": dto.size_bytes, "mime_type": dto.mime_type, "filename": dto.filename,
        "width": dto.width, "height": dto.height, "duration_seconds": dto.duration_seconds,
        "waveform": dto.waveform, "waveform_b64": waveform_b64, "thumbnail_data_url": dto.thumbnail_data_url,
        "is_voice": dto.is_voice, "is_video_note": dto.is_video_note, "status": "completed",
    })
}

pub async fn media_upload_direct(
    auth: AuthBearer, State(state): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>, headers: HeaderMap, body: Bytes,
) -> Response {
    if body.is_empty() { return err_resp(StatusCode::BAD_REQUEST, "empty_body", "Media body cannot be empty"); }
    let chat_id = match q.get("chat_id").and_then(|s| s.parse::<i64>().ok()) {
        Some(cid) => {
            if let Err(r) = check_active_membership(&state, ChatId(cid), auth.user_id) { return r; }
            Some(ChatId(cid))
        }
        None => None,
    };
    let ctype = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("application/octet-stream");
    let (data, mp_name, mp_mime) = if ctype.to_ascii_lowercase().starts_with("multipart/form-data") {
        match extract_multipart_part(&body, ctype) {
            Some(parts) => parts,
            None => return err_resp(StatusCode::BAD_REQUEST, "invalid_multipart", "Invalid multipart payload"),
        }
    } else if ctype.to_ascii_lowercase().starts_with("application/json") {
        let v: Value = match serde_json::from_slice(&body) { Ok(v) => v, Err(_) => return err_resp(StatusCode::BAD_REQUEST, "invalid_json", "Invalid JSON upload") };
        let b64 = v.get("data_base64").and_then(|s| s.as_str()).unwrap_or("");
        let decoded = match base64::engine::general_purpose::STANDARD.decode(b64) { Ok(d) => d, Err(_) => return err_resp(StatusCode::BAD_REQUEST, "invalid_base64", "Invalid data_base64") };
        (decoded, v.get("filename").and_then(|s| s.as_str()).map(String::from), v.get("mime_type").and_then(|s| s.as_str()).map(String::from))
    } else { (body.to_vec(), None, None) };
    if data.is_empty() { return err_resp(StatusCode::BAD_REQUEST, "empty_media", "Uploaded media file is empty"); }
    let filename = q.get("filename").cloned().or(mp_name)
        .or_else(|| headers.get("x-filename").and_then(|v| v.to_str().ok()).map(String::from))
        .unwrap_or_else(|| "upload.bin".to_string());
    let mime_type = q.get("mime_type").cloned().or(mp_mime)
        .unwrap_or_else(|| if ctype.starts_with("multipart/") || ctype == "application/json" { "application/octet-stream".into() } else { ctype.to_string() });
    let is_voice = q.get("is_voice").map(|s| s == "true" || s == "1").unwrap_or_else(|| mime_type.starts_with("audio/"));
    let is_video_note = q.get("is_video_note").map(|s| s == "true" || s == "1").unwrap_or(false);

    let sha256 = match state.cas.store_blob_sync(&data) {
        Ok(h) => h,
        Err(e) => return map_telegram_err(e),
    };
    let dto = persist_media_meta(&state, &auth, &sha256, &data, &filename, &mime_type, is_voice, is_video_note, chat_id);
    (StatusCode::CREATED, Json(meta_json(&dto, None))).into_response()
}

pub async fn media_upload_init_clean(auth: AuthBearer, State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match parse_json_object(&body) { Ok(v) => v, Err(r) => return r };
    let size_bytes = match payload.get("size_bytes") {
        None | Some(Value::Null) => 0,
        Some(v) => match v.as_u64() {
            Some(sz) if sz <= gpui_telegram_media::chunked_upload::MAX_UPLOAD_SIZE_BYTES => sz,
            Some(_) => {
                return err_resp(
                    StatusCode::BAD_REQUEST,
                    "invalid_upload_size",
                    &format!(
                        "size_bytes exceeds maximum allowed upload size of {} bytes",
                        gpui_telegram_media::chunked_upload::MAX_UPLOAD_SIZE_BYTES
                    ),
                );
            }
            None => {
                return err_resp(
                    StatusCode::BAD_REQUEST,
                    "invalid_upload_size",
                    "size_bytes must be a non-negative integer",
                );
            }
        },
    };
    let part_size: u32 = match payload.get("chunk_size") {
        None | Some(Value::Null) => gpui_telegram_media::chunked_upload::recommend_part_size(size_bytes),
        Some(v) => {
            let Some(raw) = v.as_u64() else {
                return err_resp(
                    StatusCode::BAD_REQUEST,
                    "invalid_chunk_size",
                    "chunk_size must be a positive integer",
                );
            };
            match u32::try_from(raw) {
                Ok(ps) if (1..=gpui_telegram_media::chunked_upload::MAX_PART_SIZE).contains(&ps) => ps,
                _ => {
                    return err_resp(
                        StatusCode::BAD_REQUEST,
                        "invalid_chunk_size",
                        &format!(
                            "chunk_size must be between 1 and {} bytes",
                            gpui_telegram_media::chunked_upload::MAX_PART_SIZE
                        ),
                    );
                }
            }
        }
    };
    let expected_sha256 = payload.get("expected_sha256").and_then(|v| v.as_str()).map(String::from);
    let filename = payload.get("filename").and_then(|v| v.as_str()).unwrap_or("file.bin");
    let upload_id = format!("upl_{}", Uuid::new_v4().simple());
    match state.cas.init_chunked_upload_for_user(&upload_id, auth.user_id.0, size_bytes, part_size, expected_sha256).await {
        Ok(session) => (StatusCode::CREATED, Json(json!({
            "upload_id": upload_id, "chunk_size": session.part_size, "total_parts": session.total_parts,
            "filename": filename, "size_bytes": session.total_size, "status": "initialized",
        }))).into_response(),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, "upload_init_failed", &e.to_string()),
    }
}

pub async fn media_upload_part_clean(auth: AuthBearer, State(state): State<Arc<AppState>>, Path((upload_id, part_index)): Path<(String, u32)>, body: Bytes) -> Response {
    match state.cas.write_chunk_for_user(&upload_id, auth.user_id.0, part_index, &body).await {
        Ok(is_complete) => (StatusCode::OK, Json(json!({
            "upload_id": upload_id, "part_index": part_index, "bytes_received": body.len(),
            "is_complete": is_complete, "status": "accepted",
        }))).into_response(),
        Err(gpui_telegram_core::error::TelegramError::PermissionDenied(m)) => err_resp(StatusCode::FORBIDDEN, "forbidden", &m),
        Err(gpui_telegram_core::error::TelegramError::NotFound(m)) => err_resp(StatusCode::NOT_FOUND, "upload_not_found", &m),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, "upload_part_failed", &e.to_string()),
    }
}

pub async fn media_upload_status(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(upload_id): Path<String>,
) -> Response {
    match state.cas.get_upload_session_for_user(&upload_id, auth.user_id.0).await {
        Ok(session) => {
            let missing_parts = session.missing_parts();
            let is_complete = session.is_complete();
            (
                StatusCode::OK,
                Json(json!({
                    "upload_id": upload_id,
                    "total_size": session.total_size,
                    "part_size": session.part_size,
                    "total_parts": session.total_parts,
                    "uploaded_parts": session.uploaded_parts,
                    "missing_parts": missing_parts,
                    "is_complete": is_complete,
                })),
            )
                .into_response()
        }
        Err(gpui_telegram_core::error::TelegramError::PermissionDenied(m)) => err_resp(StatusCode::FORBIDDEN, "forbidden", &m),
        Err(gpui_telegram_core::error::TelegramError::Validation(m)) => err_resp(StatusCode::BAD_REQUEST, "invalid_upload_id", &m),
        Err(_) => err_resp(
            StatusCode::NOT_FOUND,
            "upload_not_found",
            &format!("Upload session '{upload_id}' not found"),
        ),
    }
}

pub async fn media_upload_complete_clean(
    auth: AuthBearer, State(state): State<Arc<AppState>>,
    Path(upload_id): Path<String>, Query(q): Query<HashMap<String, String>>, body: Bytes,
) -> Response {
    let payload = if body.is_empty() { json!({}) } else { serde_json::from_slice(&body).unwrap_or_else(|_| json!({})) };
    let filename = payload.get("filename").and_then(|v| v.as_str()).or_else(|| q.get("filename").map(String::as_str)).unwrap_or("file.bin");
    let mime_type = payload.get("mime_type").and_then(|v| v.as_str()).or_else(|| q.get("mime_type").map(String::as_str)).unwrap_or("application/octet-stream");
    let chat_id = payload.get("chat_id").and_then(|v| v.as_i64()).or_else(|| q.get("chat_id").and_then(|s| s.parse().ok())).map(ChatId);
    if let Some(cid) = chat_id { if let Err(r) = check_active_membership(&state, cid, auth.user_id) { return r; } }
    let is_voice = payload.get("is_voice").and_then(|v| v.as_bool()).unwrap_or_else(|| mime_type.starts_with("audio/"));
    let is_video_note = payload.get("is_video_note").and_then(|v| v.as_bool()).unwrap_or(false);
    let sha256 = match state.cas.complete_chunked_upload_for_user(&upload_id, auth.user_id.0).await {
        Ok(h) => h,
        Err(gpui_telegram_core::error::TelegramError::PermissionDenied(m)) => return err_resp(StatusCode::FORBIDDEN, "forbidden", &m),
        Err(gpui_telegram_core::error::TelegramError::NotFound(m)) => return err_resp(StatusCode::NOT_FOUND, "upload_not_found", &m),
        Err(e) => return err_resp(StatusCode::BAD_REQUEST, "upload_complete_failed", &e.to_string()),
    };
    let bytes = state.cas.resolve_cas_path(&sha256).and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
    let dto = persist_media_meta(&state, &auth, &sha256, &bytes, filename, mime_type, is_voice, is_video_note, chat_id);
    (StatusCode::OK, Json(meta_json(&dto, Some(&upload_id)))).into_response()
}

pub async fn media_stream_clean(auth: AuthBearer, State(state): State<Arc<AppState>>, Path(file_id): Path<String>, headers: HeaderMap) -> Response {
    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let db_meta = state.db.get_media_file(&file_id).ok().flatten();
    if db_meta.is_some() && !state.db.user_can_access_media(auth.user_id, &file_id).unwrap_or(false) {
        return err_resp(StatusCode::FORBIDDEN, "forbidden", "Caller cannot access this media file");
    }
    let sha256 = db_meta.as_ref().map(|(m, _, _)| m.sha256.as_str()).unwrap_or(&file_id);
    let mime = db_meta.as_ref().map(|(m, _, _)| m.mime_type.as_str()).or_else(|| headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()));
    crate::media::streaming::stream_blob(&state.cas, sha256, range_header, mime).await
}

pub async fn media_metadata_get(auth: AuthBearer, State(state): State<Arc<AppState>>, Path(file_id): Path<String>) -> Response {
    match state.db.get_media_file(&file_id) {
        Ok(Some((m, _, _))) => {
            if !state.db.user_can_access_media(auth.user_id, &file_id).unwrap_or(false) {
                return err_resp(StatusCode::FORBIDDEN, "forbidden", "Caller cannot access this media file");
            }
            (StatusCode::OK, Json(meta_json(&m, None))).into_response()
        }
        Ok(None) => err_resp(StatusCode::NOT_FOUND, "not_found", "Media file not found"),
        Err(e) => map_telegram_err(e),
    }
}
