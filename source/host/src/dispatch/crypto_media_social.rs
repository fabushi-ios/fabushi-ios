use crate::desktop_helpers::evaluate_voice_gesture;
use crate::error::{ok_envelope, BridgeError};
use crate::host::TelegramAppHost;
use base64::engine::general_purpose::STANDARD as B64_STD;
use base64::Engine as _;
use gpui_telegram_client::{
    AddContactHttpRequest, CreatePollHttpRequest, ReactionHttpRequest, VotePollHttpRequest,
};
use gpui_telegram_core::types::{ChatId, MessageId};
use gpui_telegram_media::waveform::{decode_waveform_to_floats, encode_waveform_from_floats};
use gpui_telegram_proto::e2ee::EncryptedEnvelope;
use reqwest::Method;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

pub fn handle(host: &TelegramAppHost, cmd: &str, params: &Value) -> Option<String> {
    let client = host.current_client();
    match cmd {
        // ====================================================================
        // Namespace 12: `e2ee.*` (X3DH + Double Ratchet + Sender Keys)
        // ====================================================================
        "e2ee.status" => {
            let has_identity = client.e2ee.identity().is_ok();
            let pub_b64 = client
                .e2ee
                .identity()
                .ok()
                .map(|id| B64_STD.encode(id.x25519_pub));
            Some(ok_envelope(json!({
                "initialized": has_identity,
                "x25519_public_key_b64": pub_b64
            })))
        }

        "e2ee.bootstrap" | "e2ee.upload_prekeys" => {
            let batch = params
                .get("otpk_batch_size")
                .and_then(Value::as_u64)
                .unwrap_or(32) as usize;
            let c = Arc::clone(&client);
            match host.block_on(async move { c.e2ee_init_and_publish_prekeys(batch).await }) {
                Ok(res) => Some(ok_envelope(serde_json::to_value(res).unwrap_or_default())),
                Err(e) => Some(BridgeError::crypto(e.to_string()).to_envelope_string()),
            }
        }

        "e2ee.encrypt" | "e2ee.establish_session" => {
            let Some(recipient_user_id) = params.get("recipient_user_id").and_then(Value::as_u64) else {
                return Some(
                    BridgeError::invalid_params("Missing recipient_user_id").to_envelope_string(),
                );
            };
            let plaintext = params
                .get("plaintext")
                .and_then(Value::as_str)
                .unwrap_or("")
                .as_bytes()
                .to_vec();
            let c = Arc::clone(&client);
            match host.block_on(async move { c.e2ee_encrypt_dm(recipient_user_id, &plaintext).await }) {
                Ok(env) => Some(ok_envelope(serde_json::to_value(env).unwrap_or_default())),
                Err(e) => Some(BridgeError::crypto(e.to_string()).to_envelope_string()),
            }
        }

        "e2ee.decrypt" | "e2ee.decrypt_group" => {
            let env_val = params.get("envelope").unwrap_or(params);
            let Ok(env) = serde_json::from_value::<EncryptedEnvelope>(env_val.clone()) else {
                return Some(
                    BridgeError::invalid_params("Invalid EncryptedEnvelope").to_envelope_string(),
                );
            };
            match client.e2ee_decrypt_envelope(&env) {
                Ok(bytes) => Some(ok_envelope(json!({
                    "plaintext": String::from_utf8_lossy(&bytes),
                    "plaintext_b64": B64_STD.encode(&bytes)
                }))),
                Err(e) => Some(BridgeError::crypto(e.to_string()).to_envelope_string()),
            }
        }

        "e2ee.encrypt_group" | "e2ee.rotate_sender_key" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let members: Vec<u64> = params
                .get("member_user_ids")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_u64).collect())
                .unwrap_or_default();
            let plaintext = params
                .get("plaintext")
                .and_then(Value::as_str)
                .unwrap_or("")
                .as_bytes()
                .to_vec();
            let c = Arc::clone(&client);
            match host.block_on(async move { c.e2ee_encrypt_group(chat_id, &members, &plaintext).await }) {
                Ok((dist, group_env)) => Some(ok_envelope(json!({
                    "sender_key_distribution": dist,
                    "envelope": group_env
                }))),
                Err(e) => Some(BridgeError::crypto(e.to_string()).to_envelope_string()),
            }
        }

        "e2ee.safety_number" => {
            let Some(remote_b64) = params.get("remote_identity_key_b64").and_then(Value::as_str) else {
                return Some(
                    BridgeError::invalid_params("Missing remote_identity_key_b64")
                        .to_envelope_string(),
                );
            };
            match client.e2ee_safety_fingerprint(remote_b64) {
                Ok(emojis) => Some(ok_envelope(json!({ "emojis": emojis }))),
                Err(e) => Some(BridgeError::crypto(e.to_string()).to_envelope_string()),
            }
        }

        // ====================================================================
        // Namespace 13: `media.*` (5-Bit Waveform, Chunked Upload, CAS, Voice Orb)
        // ====================================================================
        "media.encode_waveform" | "telegram.media.encodeWaveform" | "media.encodeWaveform" => {
            let samples: Vec<f32> = params
                .get("samples")
                .and_then(Value::as_array)
                .map(|arr| arr.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect())
                .unwrap_or_default();
            let encoded = encode_waveform_from_floats(&samples);
            Some(ok_envelope(json!({
                "waveform_bytes": encoded,
                "waveform_b64": B64_STD.encode(&encoded),
                "byte_count": encoded.len()
            })))
        }

        "media.decode_waveform" | "telegram.media.decodeWaveform" | "media.decodeWaveform" => {
            let bytes: Vec<u8> = if let Some(b64) = params.get("waveform_b64").and_then(Value::as_str) {
                B64_STD.decode(b64).unwrap_or_default()
            } else {
                params
                    .get("waveform_bytes")
                    .and_then(Value::as_array)
                    .map(|arr| arr.iter().filter_map(|x| x.as_u64().map(|b| b as u8)).collect())
                    .unwrap_or_default()
            };
            let bar_count = params
                .get("bar_count")
                .and_then(Value::as_u64)
                .unwrap_or(100) as usize;
            let floats = decode_waveform_to_floats(&bytes, bar_count);
            Some(ok_envelope(json!({
                "samples": floats,
                "sample_count": floats.len()
            })))
        }

        "media.upload" | "media.upload_complete" => {
            let filename = params
                .get("filename")
                .and_then(Value::as_str)
                .unwrap_or("upload.bin")
                .to_string();
            let mime_type = params
                .get("mime_type")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream")
                .to_string();

            if let Some(file_path) = params.get("file_path").and_then(Value::as_str) {
                let pb = PathBuf::from(file_path);
                let chat_id = params.get("chat_id").and_then(Value::as_i64);
                let transfer_id = params
                    .get("transfer_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                let c = Arc::clone(&client);
                return match host.block_on(async move {
                    c.upload_file_resumable(pb, &mime_type, chat_id, transfer_id.as_deref())
                        .await
                }) {
                    Ok(done) => Some(ok_envelope(serde_json::to_value(done).unwrap_or_default())),
                    Err(e) => Some(BridgeError::network(e.to_string()).to_envelope_string()),
                };
            }

            let data = if let Some(b64) = params.get("data_b64").and_then(Value::as_str) {
                match B64_STD.decode(b64) {
                    Ok(d) => d,
                    Err(e) => {
                        return Some(
                            BridgeError::invalid_params(format!("Invalid data_b64: {e}"))
                                .to_envelope_string(),
                        )
                    }
                }
            } else {
                return Some(
                    BridgeError::invalid_params("Provide file_path or data_b64")
                        .to_envelope_string(),
                );
            };
            let c = Arc::clone(&client);
            match host.block_on(async move { c.upload_media(&filename, &mime_type, data).await }) {
                Ok(done) => Some(ok_envelope(serde_json::to_value(done).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "media.upload_init" => {
            match host.api_json(Method::POST, "/api/v1/media/chunked/init", Some(params.clone()), true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "media.upload_status" => {
            let Some(upload_id) = params.get("upload_id").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing upload_id").to_envelope_string());
            };
            match host.api_json(Method::GET, &format!("/api/v1/media/chunked/{upload_id}/status"), None, true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "media.download" => {
            let Some(file_id) = params.get("file_id").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing file_id").to_envelope_string());
            };
            let fid = file_id.to_string();
            let c = Arc::clone(&client);
            match host.block_on(async move { c.get_media_bytes(&fid).await }) {
                Ok(bytes) => {
                    let local_path = client
                        .local_media_path(file_id)
                        .map(|p| p.to_string_lossy().into_owned());
                    Some(ok_envelope(json!({
                        "file_id": file_id,
                        "size_bytes": bytes.len(),
                        "local_path": local_path,
                        "data_b64": B64_STD.encode(&bytes)
                    })))
                }
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "media.download_range" => {
            let Some(file_id) = params.get("file_id").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing file_id").to_envelope_string());
            };
            let start = params.get("start").and_then(Value::as_u64).unwrap_or(0);
            let end_inc = params.get("end_inclusive").and_then(Value::as_u64);
            let fid = file_id.to_string();
            let c = Arc::clone(&client);
            match host.block_on(async move { c.download_media_range(&fid, start, end_inc).await }) {
                Ok((status, bytes, content_range)) => Some(ok_envelope(json!({
                    "http_status": status,
                    "content_range": content_range,
                    "size_bytes": bytes.len(),
                    "data_b64": B64_STD.encode(&bytes)
                }))),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "media.voice_gesture_update" => {
            let dx = params.get("dx_pt").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let dy = params.get("dy_pt").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let released = params.get("released").and_then(Value::as_bool).unwrap_or(false);
            let reset = params.get("reset").and_then(Value::as_bool).unwrap_or(false);
            let mut g = host.voice_gesture.lock();
            if reset {
                *g = crate::desktop_helpers::VoiceGestureState::Idle;
            } else {
                *g = evaluate_voice_gesture(*g, dx, dy, released);
            }
            Some(ok_envelope(json!({ "state": *g })))
        }

        // ====================================================================
        // Namespace 14: `stickers.*` (Packs, Emoji Search & GIFs)
        // ====================================================================
        "stickers.packs" => {
            let query = params.get("query").and_then(Value::as_str).map(ToOwned::to_owned);
            let c = Arc::clone(&client);
            match host.block_on(async move { c.list_sticker_sets(query.as_deref()).await }) {
                Ok(packs) => Some(ok_envelope(serde_json::to_value(packs).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "stickers.install" | "stickers.favorite_toggle" => {
            let Some(set_id) = params.get("set_id").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing set_id").to_envelope_string());
            };
            let installed = params.get("installed").and_then(Value::as_bool).unwrap_or(true);
            let sid = set_id.to_string();
            let c = Arc::clone(&client);
            match host.block_on(async move { c.install_sticker_set(&sid, installed).await }) {
                Ok(()) => Some(ok_envelope(json!({ "set_id": set_id, "installed": installed }))),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "stickers.search_gifs" => {
            let query = params.get("query").and_then(Value::as_str).unwrap_or("trending").to_string();
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
            let c = Arc::clone(&client);
            match host.block_on(async move { c.search_gifs(&query, limit).await }) {
                Ok(gifs) => Some(ok_envelope(serde_json::to_value(gifs).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        // ====================================================================
        // Namespace 15: `reactions.*`
        // ====================================================================
        "reactions.toggle" | "messages.react" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let Some(msg_id) = params.get("msg_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing msg_id").to_envelope_string());
            };
            let Some(emoji) = params.get("emoji").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing emoji").to_envelope_string());
            };
            let add = params.get("add").and_then(Value::as_bool).unwrap_or(true);
            client
                .local_store
                .apply_viewer_reaction_mutation(ChatId(chat_id), MessageId(msg_id), emoji, add);
            if let Some(tok) = client.token() {
                let http = client.http_client.clone();
                let req = ReactionHttpRequest {
                    emoji: emoji.to_string(),
                    add,
                };
                if let Ok(resp) = host.block_on(async move {
                    http.toggle_reaction(&tok, chat_id, msg_id, &req).await
                }) {
                    return Some(ok_envelope(serde_json::to_value(resp).unwrap_or_default()));
                }
            }
            let current = client
                .local_store
                .get_reactions(ChatId(chat_id), MessageId(msg_id))
                .unwrap_or_default();
            Some(ok_envelope(json!({
                "chat_id": chat_id,
                "msg_id": msg_id,
                "reactions": current
            })))
        }

        "reactions.list" | "messages.reactions_detail" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let Some(msg_id) = params.get("msg_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing msg_id").to_envelope_string());
            };
            let reactions = client
                .local_store
                .get_reactions(ChatId(chat_id), MessageId(msg_id))
                .unwrap_or_default();
            Some(ok_envelope(json!({
                "chat_id": chat_id,
                "msg_id": msg_id,
                "reactions": reactions
            })))
        }

        // ====================================================================
        // Namespace 16: `polls.*`
        // ====================================================================
        "polls.create" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let Ok(req) = serde_json::from_value::<CreatePollHttpRequest>(params.clone()) else {
                return Some(
                    BridgeError::invalid_params("Invalid CreatePollHttpRequest")
                        .to_envelope_string(),
                );
            };
            let Some(tok) = client.token() else {
                return Some(BridgeError::unauthorized("No active session").to_envelope_string());
            };
            let http = client.http_client.clone();
            match host.block_on(async move { http.create_poll(&tok, chat_id, &req).await }) {
                Ok(poll) => Some(ok_envelope(serde_json::to_value(poll).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "polls.vote" => {
            let Some(poll_id) = params.get("poll_id").and_then(Value::as_u64) else {
                return Some(BridgeError::invalid_params("Missing poll_id").to_envelope_string());
            };
            let option_indices: Vec<usize> = params
                .get("option_indices")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_u64().map(|n| n as usize)).collect())
                .unwrap_or_default();
            let Some(tok) = client.token() else {
                return Some(BridgeError::unauthorized("No active session").to_envelope_string());
            };
            let http = client.http_client.clone();
            let req = VotePollHttpRequest { option_indices };
            match host.block_on(async move { http.vote_poll(&tok, poll_id, &req).await }) {
                Ok(poll) => Some(ok_envelope(serde_json::to_value(poll).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "polls.close" => {
            let Some(poll_id) = params.get("poll_id").and_then(Value::as_u64) else {
                return Some(BridgeError::invalid_params("Missing poll_id").to_envelope_string());
            };
            let Some(tok) = client.token() else {
                return Some(BridgeError::unauthorized("No active session").to_envelope_string());
            };
            let http = client.http_client.clone();
            match host.block_on(async move { http.close_poll(&tok, poll_id).await }) {
                Ok(poll) => Some(ok_envelope(serde_json::to_value(poll).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        // ====================================================================
        // Namespace 17: `peers.*` / `groups.*` / `forums.*`
        // ====================================================================
        "peers.profile" => {
            if let Some(user_id) = params.get("user_id").and_then(Value::as_u64) {
                match host.api_json(Method::GET, &format!("/api/v1/users/{user_id}"), None, true) {
                    Ok(v) => return Some(ok_envelope(v)),
                    Err(e) => return Some(e.to_envelope_string()),
                }
            }
            if let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) {
                let c = Arc::clone(&client);
                return match host.block_on(async move { c.get_chat(chat_id).await }) {
                    Ok(d) => Some(ok_envelope(serde_json::to_value(d).unwrap_or_default())),
                    Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
                };
            }
            Some(BridgeError::invalid_params("Provide user_id or chat_id").to_envelope_string())
        }

        "peers.members" | "groups.members" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let c = Arc::clone(&client);
            match host.block_on(async move { c.get_chat_members(chat_id).await }) {
                Ok(members) => Some(ok_envelope(serde_json::to_value(members).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "peers.add_member" | "groups.add_member" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let user_ids: Vec<u64> = params
                .get("user_ids")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_u64).collect())
                .or_else(|| params.get("user_id").and_then(Value::as_u64).map(|u| vec![u]))
                .unwrap_or_default();
            let c = Arc::clone(&client);
            match host.block_on(async move { c.add_chat_members(chat_id, &user_ids).await }) {
                Ok(ok) => Some(ok_envelope(json!({ "added": ok, "chat_id": chat_id }))),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "peers.remove_member" | "groups.remove_member" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let Some(user_id) = params.get("user_id").and_then(Value::as_u64) else {
                return Some(BridgeError::invalid_params("Missing user_id").to_envelope_string());
            };
            let c = Arc::clone(&client);
            match host.block_on(async move { c.remove_chat_member(chat_id, user_id).await }) {
                Ok(ok) => Some(ok_envelope(json!({ "removed": ok }))),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "peers.update_role" | "groups.update_role" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let Some(user_id) = params.get("user_id").and_then(Value::as_u64) else {
                return Some(BridgeError::invalid_params("Missing user_id").to_envelope_string());
            };
            let role = params.get("role").and_then(Value::as_str).unwrap_or("admin").to_string();
            let title = params.get("admin_title").and_then(Value::as_str).map(ToOwned::to_owned);
            let rights = params.get("admin_rights").and_then(Value::as_u64);
            let c = Arc::clone(&client);
            match host.block_on(async move {
                c.update_chat_member_role(chat_id, user_id, &role, title.as_deref(), rights)
                    .await
            }) {
                Ok(ok) => Some(ok_envelope(json!({ "updated": ok }))),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "peers.invite_links" | "groups.invite_links" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let c = Arc::clone(&client);
            match host.block_on(async move { c.list_invites(chat_id).await }) {
                Ok(links) => Some(ok_envelope(serde_json::to_value(links).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "peers.create_invite_link" | "groups.create_invite_link" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let title = params.get("title").and_then(Value::as_str).map(ToOwned::to_owned);
            let usage_limit = params.get("usage_limit").and_then(Value::as_u64).map(|n| n as u32);
            let expire_ms = params.get("expire_date_ms").and_then(Value::as_i64);
            let request_needed = params
                .get("request_needed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let c = Arc::clone(&client);
            match host.block_on(async move {
                c.create_invite(chat_id, title.as_deref(), usage_limit, expire_ms, request_needed)
                    .await
            }) {
                Ok(link) => Some(ok_envelope(serde_json::to_value(link).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "peers.revoke_invite_link" | "groups.revoke_invite_link" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let Some(invite_id) = params.get("invite_id").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing invite_id").to_envelope_string());
            };
            let iid = invite_id.to_string();
            let c = Arc::clone(&client);
            match host.block_on(async move { c.revoke_invite(chat_id, &iid).await }) {
                Ok(()) => Some(ok_envelope(json!({ "revoked": true }))),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "peers.join_by_invite" | "groups.join_by_invite" => {
            let Some(code) = params.get("code").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing code").to_envelope_string());
            };
            let code_s = code.to_string();
            let c = Arc::clone(&client);
            match host.block_on(async move { c.join_invite(&code_s).await }) {
                Ok(chat) => Some(ok_envelope(serde_json::to_value(chat).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "peers.topics_list" | "forums.topics_list" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let c = Arc::clone(&client);
            match host.block_on(async move { c.list_forum_topics(chat_id).await }) {
                Ok(topics) => Some(ok_envelope(serde_json::to_value(topics).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "peers.topic_create" | "forums.topic_create" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            match host.api_json(
                Method::POST,
                &format!("/api/v1/chats/{chat_id}/topics"),
                Some(params.clone()),
                true,
            ) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "peers.stats" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let c = Arc::clone(&client);
            match host.block_on(async move { c.get_channel_statistics(chat_id).await }) {
                Ok(stats) => Some(ok_envelope(serde_json::to_value(stats).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "peers.export_chat" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let fmt = params.get("format").and_then(Value::as_str).map(ToOwned::to_owned);
            let c = Arc::clone(&client);
            match host.block_on(async move { c.export_chat_history(chat_id, fmt.as_deref()).await }) {
                Ok(exported) => Some(ok_envelope(json!({ "chat_id": chat_id, "export": exported }))),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        // ====================================================================
        // Namespace 18: `contacts.*`
        // ====================================================================
        "contacts.list" => match host.api_json(Method::GET, "/api/v1/contacts", None, true) {
            Ok(v) => Some(ok_envelope(v)),
            Err(e) => Some(e.to_envelope_string()),
        },

        "contacts.add" => {
            let Ok(req) = serde_json::from_value::<AddContactHttpRequest>(params.clone()) else {
                return Some(
                    BridgeError::invalid_params("Invalid AddContactHttpRequest")
                        .to_envelope_string(),
                );
            };
            let Some(tok) = client.token() else {
                return Some(BridgeError::unauthorized("No active session").to_envelope_string());
            };
            let http = client.http_client.clone();
            match host.block_on(async move { http.add_contact(&tok, &req).await }) {
                Ok(added) => Some(ok_envelope(serde_json::to_value(added).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "contacts.delete" => {
            let Some(user_id) = params.get("user_id").and_then(Value::as_u64) else {
                return Some(BridgeError::invalid_params("Missing user_id").to_envelope_string());
            };
            match host.api_json(Method::DELETE, &format!("/api/v1/contacts/{user_id}"), None, true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "contacts.block" | "contacts.unblock" => {
            let Some(user_id) = params.get("user_id").and_then(Value::as_u64) else {
                return Some(BridgeError::invalid_params("Missing user_id").to_envelope_string());
            };
            let action = if cmd == "contacts.block" { "block" } else { "unblock" };
            match host.api_json(
                Method::POST,
                &format!("/api/v1/users/{user_id}/{action}"),
                Some(json!({})),
                true,
            ) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "contacts.blocked_list" => {
            match host.api_json(Method::GET, "/api/v1/users/blocked", None, true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "contacts.search_users" => {
            let query = params.get("query").and_then(Value::as_str).unwrap_or("");
            match host.api_json(
                Method::GET,
                &format!("/api/v1/users/search?q={}", url_encode_simple(query)),
                None,
                true,
            ) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        _ => None,
    }
}

fn url_encode_simple(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
