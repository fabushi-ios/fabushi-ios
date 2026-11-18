use crate::db::social_store::CallSignalOutcome;
use crate::routes::{check_active_membership, err_resp, map_telegram_err, parse_json_object};
use crate::server::{AppState, AuthBearer};
use crate::webrtc::signaling_hub::generate_turn_token;
use axum::{body::Bytes, extract::{Path, Query, State}, http::StatusCode, response::{IntoResponse, Json, Response}};
use chrono::Utc;
use gpui_telegram_core::types::{ChatId, UserId};
use gpui_telegram_proto::contracts::{CallInviteRequest, CallRecordDto, CallSignalRequest};
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::WsServerFrame;
use gpui_telegram_webrtc::signaling::SignalingFrame;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

pub async fn stickers_sets(auth: AuthBearer, State(state): State<Arc<AppState>>) -> Response {
    let _ = state.db.seed_default_catalog_if_empty();
    match state.db.list_sticker_sets(auth.user_id) {
        Ok(sets) => (StatusCode::OK, Json(json!({ "sets": sets }))).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn stickers_install(auth: AuthBearer, State(state): State<Arc<AppState>>, Path(set_id): Path<String>, body: Bytes) -> Response {
    let install = if body.is_empty() { true } else { serde_json::from_slice::<Value>(&body).ok().and_then(|v| v.get("install").and_then(|b| b.as_bool())).unwrap_or(true) };
    match state.db.install_sticker_set(auth.user_id, &set_id, install) {
        Ok(ok) => (StatusCode::OK, Json(json!({ "set_id": set_id, "installed": install, "success": ok }))).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn gifs_search(_auth: AuthBearer, State(state): State<Arc<AppState>>, Query(q): Query<HashMap<String, String>>) -> Response {
    let _ = state.db.seed_default_catalog_if_empty();
    let query = q.get("q").map(String::as_str).unwrap_or("");
    let limit = q.get("limit").and_then(|s| s.parse().ok()).unwrap_or(24).clamp(1, 50);
    match state.db.search_gifs(query, limit) {
        Ok(gifs) => (StatusCode::OK, Json(json!({ "query": query, "gifs": gifs }))).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn stories_list(auth: AuthBearer, State(state): State<Arc<AppState>>) -> Response {
    match state.db.list_active_stories(auth.user_id) {
        Ok(stories) => (StatusCode::OK, Json(json!({ "stories": stories }))).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn stories_create(auth: AuthBearer, State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match parse_json_object(&body) { Ok(v) => v, Err(r) => return r };
    let media_id = payload.get("media_id").and_then(|v| v.as_str());
    let media_url = payload.get("media_url").and_then(|v| v.as_str());
    if media_id.is_none() && media_url.is_none() {
        return err_resp(StatusCode::BAD_REQUEST, "missing_media", "media_id or media_url is required for story");
    }
    let media_type = payload.get("media_type").and_then(|v| v.as_str()).unwrap_or("photo");
    let caption = payload.get("caption").and_then(|v| v.as_str());
    let privacy = payload.get("privacy").and_then(|v| v.as_str()).unwrap_or("contacts");
    let ttl_secs = payload.get("ttl_hours").and_then(|v| v.as_i64()).unwrap_or(24).clamp(1, 48) * 3600;
    match state.db.create_story(auth.user_id, media_id, media_url, media_type, caption, privacy, ttl_secs) {
        Ok(story) => {
            state.gateway_hub.broadcast_to_user(auth.user_id, WsServerFrame::Update(UpdateEvent::StoryUpdated { story: story.clone() }));
            (StatusCode::CREATED, Json(story)).into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

pub async fn stories_view(auth: AuthBearer, State(state): State<Arc<AppState>>, Path(story_id): Path<String>, body: Bytes) -> Response {
    let reaction = if body.is_empty() { None } else { serde_json::from_slice::<Value>(&body).ok().and_then(|v| v.get("reaction").and_then(|s| s.as_str()).map(String::from)) };
    match state.db.view_story(&story_id, auth.user_id, reaction.as_deref()) {
        Ok(views) => (StatusCode::OK, Json(json!({ "story_id": story_id, "views_count": views }))).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn stories_delete(auth: AuthBearer, State(state): State<Arc<AppState>>, Path(story_id): Path<String>) -> Response {
    match state.db.delete_story(&story_id, auth.user_id) {
        Ok(true) => (StatusCode::OK, Json(json!({ "deleted": true, "story_id": story_id }))).into_response(),
        Ok(false) => err_resp(StatusCode::NOT_FOUND, "not_found", "Story not found or not owned by user"),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn calls_list(auth: AuthBearer, State(state): State<Arc<AppState>>) -> Response {
    match state.db.list_calls_for_user(auth.user_id) {
        Ok(calls) => (StatusCode::OK, Json(json!({ "calls": calls }))).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

async fn resolve_call_peer_from_chat(
    state: &AppState,
    caller_id: UserId,
    chat_id: ChatId,
) -> Result<(crate::db::schema::UserRecord, crate::db::schema::ChatRecord), Response> {
    let (chat, _, _) = check_active_membership(state, chat_id, caller_id)?;
    let members = state.db.get_chat_members(chat_id).unwrap_or_default();
    let mut peer_candidates = Vec::new();
    for member_id in members {
        if member_id != caller_id {
            if let Ok(Some(u)) = state.db.get_user(member_id) {
                peer_candidates.push(u);
            }
        }
    }
    if peer_candidates.is_empty() && crate::server::is_welcome_bot_chat(chat_id) {
        if let Ok(Some(bot)) = state.db.get_user(UserId(777_000)) {
            peer_candidates.push(bot);
        }
    }
    let peer = peer_candidates
        .iter()
        .find(|u| u.is_bot)
        .cloned()
        .or_else(|| peer_candidates.into_iter().next())
        .ok_or_else(|| err_resp(StatusCode::NOT_FOUND, "user_not_found", "Callee user not found"))?;
    Ok((peer, chat))
}

pub async fn calls_invite(auth: AuthBearer, State(state): State<Arc<AppState>>, Json(req): Json<CallInviteRequest>) -> Response {
    if req.callee_id == auth.user_id.0 {
        return err_resp(StatusCode::BAD_REQUEST, "invalid_callee", "Cannot call yourself");
    }

    let mut resolved_chat = match req.chat_id {
        Some(cid) => state.db.get_chat(ChatId(cid)).ok().flatten(),
        None => None,
    };

    let target_user = match state.db.get_user(UserId(req.callee_id)) {
        Ok(Some(u)) => u,
        Ok(None) => {
            if let Some(cand_cid) = i64::try_from(req.callee_id).ok().filter(|_| req.callee_id > 0) {
                if let Ok(Some(_)) = state.db.get_chat(ChatId(cand_cid)) {
                    match resolve_call_peer_from_chat(&state, auth.user_id, ChatId(cand_cid)).await {
                        Ok((peer, chat)) => {
                            resolved_chat = Some(chat);
                            peer
                        }
                        Err(resp) => return resp,
                    }
                } else if let Some(explicit_cid) = req.chat_id {
                    match resolve_call_peer_from_chat(&state, auth.user_id, ChatId(explicit_cid)).await {
                        Ok((peer, chat)) => {
                            resolved_chat = Some(chat);
                            peer
                        }
                        Err(resp) => return resp,
                    }
                } else {
                    return err_resp(StatusCode::NOT_FOUND, "user_not_found", "Callee user not found");
                }
            } else if let Some(explicit_cid) = req.chat_id {
                match resolve_call_peer_from_chat(&state, auth.user_id, ChatId(explicit_cid)).await {
                    Ok((peer, chat)) => {
                        resolved_chat = Some(chat);
                        peer
                    }
                    Err(resp) => return resp,
                }
            } else {
                return err_resp(StatusCode::NOT_FOUND, "user_not_found", "Callee user not found");
            }
        }
        Err(e) => return map_telegram_err(e),
    };

    if target_user.user_id == auth.user_id {
        return err_resp(StatusCode::BAD_REQUEST, "invalid_callee", "Cannot call yourself");
    }

    let resolved_chat_id = resolved_chat.as_ref().map(|c| c.chat_id).or(req.chat_id.map(ChatId));
    let is_ai_bot = target_user.is_bot
        || resolved_chat
            .as_ref()
            .is_some_and(|c| c.chat_type == 4 || crate::server::is_welcome_bot_chat(c.chat_id));

    let call_id = format!("call_{}", Uuid::new_v4().simple());
    let now_ms = Utc::now().timestamp_millis();
    let initial_state = if is_ai_bot { "active" } else { "ringing" };
    let rec = CallRecordDto {
        call_id: call_id.clone(),
        caller_id: auth.user_id.0,
        callee_id: target_user.user_id.0,
        is_video: req.is_video,
        state: initial_state.into(),
        duration_secs: 0,
        rating: None,
        started_at_ms: now_ms,
        ended_at_ms: None,
    };
    if let Err(e) = state.db.save_call(&rec, resolved_chat_id) {
        return map_telegram_err(e);
    }

    if is_ai_bot {
        let (talk_session_id, talk_state, talk_generation, talk_signal_seq) = {
            let mut coord = state.talk_coordinator.write();
            coord.session_id = format!("talk_{}", Uuid::new_v4().simple());
            coord.generation = coord.generation.saturating_add(1);
            coord.signal_seq = coord.signal_seq.saturating_add(1);
            coord.state = gpui_telegram_agent::TalkSessionState::Listening;
            (
                coord.session_id.clone(),
                coord.state,
                coord.generation,
                coord.signal_seq,
            )
        };
        let transcript_seed = json!([
            {
                "role": "system",
                "mode": "ai_talk",
                "call_id": call_id,
                "chat_id": resolved_chat_id.map(|c| c.0),
                "bot_id": target_user.user_id.0,
                "bot_name": target_user.first_name,
                "is_video": req.is_video,
                "started_at_ms": now_ms
            }
        ]);
        let _ = state.db.upsert_talk_session(
            &talk_session_id,
            auth.user_id.0,
            "listening",
            &transcript_seed,
            now_ms,
        );
        let tap_chat = resolved_chat_id.unwrap_or(ChatId(target_user.user_id.0 as i64));
        let ai_tap = Arc::new(gpui_telegram_webrtc::media::DefaultAiMediaTap::new(48_000, 1, 0.05));
        state.signaling_hub.join_sfu_room(tap_chat, auth.user_id);
        state.signaling_hub.join_sfu_room(tap_chat, target_user.user_id);
        state.signaling_hub.attach_ai_tap(tap_chat, ai_tap);
        let ev = WsServerFrame::Update(UpdateEvent::CallUpdated {
            call_id: call_id.clone(),
            state: "active".into(),
            signal: None,
        });
        state.gateway_hub.broadcast_to_user(auth.user_id, ev.clone());
        state.gateway_hub.broadcast_to_user(target_user.user_id, ev);
        return (
            StatusCode::CREATED,
            Json(json!({
                "call_id": call_id,
                "caller_id": auth.user_id.0,
                "callee_id": target_user.user_id.0,
                "chat_id": resolved_chat_id.map(|c| c.0),
                "is_video": req.is_video,
                "state": "active",
                "status": "active",
                "duration_secs": 0,
                "rating": Value::Null,
                "started_at_ms": now_ms,
                "ended_at_ms": Value::Null,
                "mode": "ai_talk",
                "is_bot": true,
                "bot_name": target_user.first_name,
                "talk_session_id": talk_session_id,
                "talk_state": talk_state,
                "talk_generation": talk_generation,
                "talk_signal_seq": talk_signal_seq,
            })),
        )
            .into_response();
    }

    let frame = SignalingFrame::Invite {
        call_id: call_id.clone(),
        from_user: auth.user_id,
        to_user: target_user.user_id,
        is_video: req.is_video,
    };
    let _ = state.signaling_hub.dispatch_frame(auth.user_id, frame).await;
    let ev = WsServerFrame::Update(UpdateEvent::CallUpdated {
        call_id: call_id.clone(),
        state: "ringing".into(),
        signal: None,
    });
    state.gateway_hub.broadcast_to_user(target_user.user_id, ev.clone());
    state.gateway_hub.broadcast_to_user(auth.user_id, ev);
    (
        StatusCode::CREATED,
        Json(json!({
            "call_id": call_id,
            "caller_id": auth.user_id.0,
            "callee_id": target_user.user_id.0,
            "chat_id": resolved_chat_id.map(|c| c.0),
            "is_video": req.is_video,
            "state": "ringing",
            "status": "invited",
            "duration_secs": 0,
            "rating": Value::Null,
            "started_at_ms": now_ms,
            "ended_at_ms": Value::Null,
            "mode": "rtc",
        })),
    )
        .into_response()
}

pub async fn calls_signal(auth: AuthBearer, State(state): State<Arc<AppState>>, Json(req): Json<CallSignalRequest>) -> Response {
    let st = req.signal_type.to_ascii_lowercase();
    let frame = match st.as_str() {
        "ringing" => SignalingFrame::Ringing { call_id: req.call_id.clone() },
        "accept" => SignalingFrame::Accept { call_id: req.call_id.clone() },
        "offer" => {
            if req.sdp.as_deref().unwrap_or("").trim().is_empty() {
                return err_resp(StatusCode::BAD_REQUEST, "invalid_payload", "sdp is required for offer");
            }
            SignalingFrame::Offer { call_id: req.call_id.clone(), sdp: req.sdp.clone().unwrap_or_default() }
        }
        "answer" => {
            if req.sdp.as_deref().unwrap_or("").trim().is_empty() {
                return err_resp(StatusCode::BAD_REQUEST, "invalid_payload", "sdp is required for answer");
            }
            SignalingFrame::Answer { call_id: req.call_id.clone(), sdp: req.sdp.clone().unwrap_or_default() }
        }
        "ice_candidate" | "ice" => {
            if req.candidate.as_deref().unwrap_or("").trim().is_empty() {
                return err_resp(StatusCode::BAD_REQUEST, "invalid_payload", "candidate is required for ice_candidate");
            }
            let sdp_mline_index = match req.sdp_mline_index {
                Some(i) => match u16::try_from(i) {
                    Ok(idx) => Some(idx),
                    Err(_) => return err_resp(StatusCode::BAD_REQUEST, "invalid_payload", "sdp_mline_index must fit in u16"),
                },
                None => None,
            };
            SignalingFrame::IceCandidate {
                call_id: req.call_id.clone(), candidate: req.candidate.clone().unwrap_or_default(),
                sdp_mid: req.sdp_mid.clone(), sdp_mline_index,
            }
        }
        "hangup" | "end" | "decline" => SignalingFrame::Hangup { call_id: req.call_id.clone(), reason: req.reason.clone().unwrap_or_else(|| "hangup".into()) },
        _ => return err_resp(StatusCode::BAD_REQUEST, "invalid_signal", "Unsupported WebRTC signal_type"),
    };

    let updated = match state.db.apply_call_signal(&req.call_id, auth.user_id.0, &st, Utc::now().timestamp_millis()) {
        Ok(CallSignalOutcome::Applied(c)) => c,
        Ok(CallSignalOutcome::NotFound) => return err_resp(StatusCode::NOT_FOUND, "not_found", "Call not found"),
        Ok(CallSignalOutcome::Forbidden(msg)) => return err_resp(StatusCode::FORBIDDEN, "forbidden", msg),
        Ok(CallSignalOutcome::Conflict { code, message }) => return err_resp(StatusCode::CONFLICT, code, message),
        Err(e) => return map_telegram_err(e),
    };

    if let Err(err) = state.signaling_hub.dispatch_frame(auth.user_id, frame).await {
        match err {
            gpui_telegram_core::error::TelegramError::PermissionDenied(msg) => {
                return err_resp(StatusCode::FORBIDDEN, "forbidden", &msg);
            }
            other => {
                tracing::debug!(call_id = %req.call_id, signal_type = %st, error = %other, "SignalingHub dispatch_frame non-fatal fallback");
            }
        }
    }

    let peer = if updated.caller_id == auth.user_id.0 {
        UserId(updated.callee_id)
    } else {
        UserId(updated.caller_id)
    };
    state.gateway_hub.broadcast_to_user(peer, WsServerFrame::Update(UpdateEvent::CallUpdated {
        call_id: req.call_id.clone(),
        state: updated.state.clone(),
        signal: Some(req.clone()),
    }));
    (StatusCode::OK, Json(json!({ "call_id": req.call_id, "user_id": auth.user_id.0, "status": "relayed", "call": updated }))).into_response()
}

pub async fn calls_rate(auth: AuthBearer, State(state): State<Arc<AppState>>, Path(call_id): Path<String>, body: Bytes) -> Response {
    let payload = match parse_json_object(&body) { Ok(v) => v, Err(r) => return r };
    let rating = match payload.get("rating").and_then(|v| v.as_u64()).filter(|&r| (1..=5).contains(&r)) {
        Some(r) => r as u8,
        None => return err_resp(StatusCode::BAD_REQUEST, "invalid_rating", "rating must be between 1 and 5"),
    };
    let feedback = payload.get("feedback").and_then(|v| v.as_str());

    let call = match state.db.get_call(&call_id) {
        Ok(Some(c)) => c,
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, "not_found", "Call not found"),
        Err(e) => return map_telegram_err(e),
    };
    if call.caller_id != auth.user_id.0 && call.callee_id != auth.user_id.0 {
        return err_resp(StatusCode::FORBIDDEN, "forbidden", "Caller did not participate in this call");
    }

    match state.db.update_call_state_and_rating(&call_id, None, None, Some(rating), feedback) {
        Ok(Some(c)) => (StatusCode::OK, Json(c)).into_response(),
        Ok(None) => err_resp(StatusCode::NOT_FOUND, "not_found", "Call not found"),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn webrtc_turn_credentials_clean(auth: AuthBearer) -> Response {
    let secret = std::env::var("TURN_SECRET").unwrap_or_else(|_| "gpui_telegram_turn_ephemeral_secret".into());
    let uris = vec![
        std::env::var("STUN_URL").unwrap_or_else(|_| "stun:stun.l.google.com:19302".into()),
        std::env::var("TURN_URL").unwrap_or_else(|_| "turn:turn.gpui-telegram.local:3478?transport=udp".into()),
    ];
    let token = generate_turn_token(auth.user_id, &secret, uris, 12);
    (StatusCode::OK, Json(json!({
        "urls": token.uris, "uris": token.uris, "username": token.username,
        "credential": token.credential, "ttl_seconds": 43200, "expires_at": token.expires_at.to_rfc3339(),
    }))).into_response()
}
