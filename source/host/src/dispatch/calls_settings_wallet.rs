use crate::error::{ok_envelope, BridgeError};
use crate::host::TelegramAppHost;
use fabushi_miniapp::state::membership_plans;
use gpui_telegram_client::{CallInviteRequest, CallSignalRequest, StarGiftRequest};
use gpui_telegram_core::types::UserId;
use gpui_telegram_webrtc::calls::calls_emoji_fingerprint::CallsEmojiFingerprint;
use gpui_telegram_webrtc::calls::sha256_digest;
use gpui_telegram_webrtc::media::AiMediaTap;
use gpui_telegram_webrtc::signaling::SignalingFrame;
use reqwest::Method;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn handle(host: &TelegramAppHost, cmd: &str, params: &Value) -> Option<String> {
    let client = host.current_client();
    match cmd {
        // ====================================================================
        // Namespace 19: `calls.*` (+ `telegram.webrtc.*`)
        // ====================================================================
        "calls.start" | "telegram.webrtc.createSession" | "webrtc.createSession" => {
            let callee_id = params
                .get("callee_id")
                .and_then(Value::as_u64)
                .or_else(|| {
                    params
                        .get("peer_id")
                        .and_then(Value::as_str)
                        .and_then(|s| s.parse::<u64>().ok())
                })
                .unwrap_or(2);
            let is_video = params.get("is_video").and_then(Value::as_bool).unwrap_or(false);
            let local_uid = client.current_user_id().unwrap_or(1);
            let call_num = chrono::Utc::now().timestamp_millis() % 1_000_000_000;

            let (local_sdp, outbound) = {
                let mut eng = host.call_engine.lock();
                eng.start_outgoing(call_num, UserId(local_uid), UserId(callee_id), is_video);
                (eng.local_sdp.clone(), eng.drain_outbound_frames())
            };
            {
                let mut tree = host.semantic_tree.lock();
                tree.is_call_active = true;
                tree.recompute_state_hash();
            }

            if client.token().is_some() && !*host.simulate_offline.lock() {
                let req = CallInviteRequest {
                    callee_id,
                    is_video,
                    chat_id: params.get("chat_id").and_then(Value::as_i64),
                };
                let c = Arc::clone(&client);
                if let Ok(rec) = host.block_on(async move { c.call_invite(&req).await }) {
                    return Some(ok_envelope(json!({
                        "call": rec,
                        "sessionId": rec.call_id,
                        "peerId": callee_id.to_string(),
                        "state": "initiating",
                        "local_sdp": local_sdp,
                        "outbound_frames": outbound
                    })));
                }
            }

            let session_id = format!("call-{call_num}");
            Some(ok_envelope(json!({
                "sessionId": session_id,
                "call_id": call_num.to_string(),
                "peerId": callee_id.to_string(),
                "state": "initiating",
                "is_video": is_video,
                "local_sdp": local_sdp,
                "outbound_frames": outbound,
                "createdAt": chrono::Utc::now().to_rfc3339()
            })))
        }

        "calls.accept" => {
            let call_id = params
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or("1")
                .to_string();
            let ans_frame = host.call_engine.lock().accept_incoming();
            if client.token().is_some() && !*host.simulate_offline.lock() {
                let cid = call_id.clone();
                let c = Arc::clone(&client);
                if let Ok(rec) = host.block_on(async move { c.call_accept(&cid).await }) {
                    return Some(ok_envelope(json!({ "call": rec, "answer": ans_frame })));
                }
            }
            Some(ok_envelope(json!({ "call_id": call_id, "accepted": true, "answer": ans_frame })))
        }

        "calls.decline" | "calls.hangup" => {
            let call_id = params
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or("1")
                .to_string();
            host.call_engine.lock().hangup_local(cmd);
            {
                let mut tree = host.semantic_tree.lock();
                tree.is_call_active = false;
                tree.recompute_state_hash();
            }
            if client.token().is_some() && !*host.simulate_offline.lock() {
                let cid = call_id.clone();
                let c = Arc::clone(&client);
                let _ = if cmd == "calls.decline" {
                    host.block_on(async move { c.call_decline(&cid).await })
                } else {
                    host.block_on(async move { c.call_hangup(&cid).await })
                };
            }
            Some(ok_envelope(json!({ "call_id": call_id, "ended": true })))
        }

        "calls.signal" => {
            if let Ok(sig_frame) = serde_json::from_value::<SignalingFrame>(params.clone()) {
                host.call_engine.lock().apply_signaling_frame(sig_frame);
            }
            if let Ok(req) = serde_json::from_value::<CallSignalRequest>(params.clone()) {
                if client.token().is_some() && !*host.simulate_offline.lock() {
                    let target = params.get("target_user_id").and_then(Value::as_u64);
                    let c = Arc::clone(&client);
                    let _ = host.block_on(async move { c.send_call_signal(req, target).await });
                }
            }
            let fp = host
                .call_engine
                .lock()
                .emoji_fingerprint()
                .map(|e| e.as_emoji_string());
            Some(ok_envelope(json!({ "applied": true, "emoji_fingerprint": fp })))
        }

        "calls.emoji_fingerprint" => {
            if let Some(seed) = params.get("key_material").and_then(Value::as_str) {
                let digest = sha256_digest(seed.as_bytes());
                let fp = CallsEmojiFingerprint::from_sha256(digest);
                return Some(ok_envelope(json!({
                    "emojis": fp.as_emoji_string(),
                    "sha256_hex": digest.iter().map(|b| format!("{b:02x}")).collect::<String>()
                })));
            }
            let fp = host
                .call_engine
                .lock()
                .emoji_fingerprint()
                .map(|e| e.as_emoji_string());
            Some(ok_envelope(json!({ "emojis": fp })))
        }

        "calls.turn_credentials" => {
            let c = Arc::clone(&client);
            match host.block_on(async move { c.get_turn_credentials().await }) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "calls.history" => {
            let c = Arc::clone(&client);
            match host.block_on(async move { c.list_calls().await }) {
                Ok(list) => Some(ok_envelope(serde_json::to_value(list).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "calls.push_pcm_level" => {
            let samples: Vec<i16> = params
                .get("pcm_i16")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_i64().map(|n| n as i16)).collect())
                .unwrap_or_default();
            let mut eng = host.call_engine.lock();
            let level = eng.push_audio_level_pcm(&samples);
            Some(ok_envelope(json!({
                "level": level,
                "is_speaking": eng.meter.is_speaking()
            })))
        }

        // ====================================================================
        // Namespace 20: `talk.*` (`<60ms` VAD Barge-in & Live Voice AI)
        // ====================================================================
        "talk.start" => {
            let session_id = format!("talk_{}", uuid::Uuid::new_v4().simple());
            let peer = params
                .get("peer_id")
                .or_else(|| params.get("bot_id"))
                .and_then(Value::as_str)
                .unwrap_or("hermes_assistant")
                .to_string();
            host.ai_media_tap.reset_barge_in();
            {
                let mut st = host.talk_state.lock();
                st.active = true;
                st.session_id = Some(session_id.clone());
                st.peer_or_bot_id = Some(peer.clone());
                st.tts_playing = params.get("tts_playing").and_then(Value::as_bool).unwrap_or(true);
            }
            if client.token().is_some() && !*host.simulate_offline.lock() {
                let p = params.clone();
                let c = Arc::clone(&client);
                let _ = host.block_on(async move { c.create_talk_session(&p).await });
            }
            Some(ok_envelope(json!({
                "active": true,
                "session_id": session_id,
                "peer_id": peer
            })))
        }

        "talk.feed_pcm" | "talk.push_pcm_vad" => {
            let samples: Vec<i16> = params
                .get("pcm_i16")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_i64().map(|n| n as i16)).collect())
                .unwrap_or_default();
            let sample_rate = params
                .get("sample_rate")
                .and_then(Value::as_u64)
                .unwrap_or(16_000) as u32;
            let channels = params
                .get("channels")
                .and_then(Value::as_u64)
                .unwrap_or(1) as u16;
            if params.get("reset").and_then(Value::as_bool).unwrap_or(false) {
                host.ai_media_tap.reset_barge_in();
            }
            host.ai_media_tap
                .on_incoming_pcm_frame(&samples, sample_rate, channels);
            let barged_in = host.ai_media_tap.trigger_vad_barge_in();
            let rms = host.ai_media_tap.last_incoming_rms();
            {
                let mut st = host.talk_state.lock();
                st.last_vad_rms = rms;
                if barged_in && st.tts_playing {
                    st.tts_playing = false;
                    st.barge_in_count += 1;
                    host.events.emit(
                        "vadBargeIn",
                        &json!({ "rms": rms, "barge_in_count": st.barge_in_count }),
                    );
                }
            }
            Some(ok_envelope(json!({
                "barge_in": barged_in,
                "rms": rms,
                "tts_playing": host.talk_state.lock().tts_playing
            })))
        }

        "talk.barge_in" => {
            let mut st = host.talk_state.lock();
            st.tts_playing = false;
            st.barge_in_count += 1;
            host.ai_media_tap.reset_barge_in();
            Some(ok_envelope(json!({
                "barged_in": true,
                "barge_in_count": st.barge_in_count
            })))
        }

        "talk.stop" => {
            host.ai_media_tap.reset_barge_in();
            let mut st = host.talk_state.lock();
            st.active = false;
            st.tts_playing = false;
            Some(ok_envelope(json!({ "active": false })))
        }

        "talk.state" => {
            let st = host.talk_state.lock();
            Some(ok_envelope(json!({
                "active": st.active,
                "session_id": st.session_id,
                "peer_id": st.peer_or_bot_id,
                "tts_playing": st.tts_playing,
                "barge_in_count": st.barge_in_count,
                "last_vad_rms": st.last_vad_rms
            })))
        }

        // ====================================================================
        // Namespace 21: `settings.*` (Profile, Privacy, Push, Storage, Passcode)
        // ====================================================================
        "settings.get" => {
            let cfg = host.host_config.lock().clone();
            Some(ok_envelope(json!({
                "preferences": cfg.preferences,
                "has_passcode": cfg.passcode_phc.is_some(),
                "has_apns_token": cfg.apns_token.is_some(),
                "has_voip_token": cfg.voip_token.is_some()
            })))
        }

        "settings.update" => {
            if client.token().is_some() && !*host.simulate_offline.lock() {
                if params.get("first_name").is_some() || params.get("bio").is_some() {
                    let _ = host.api_json(Method::PATCH, "/api/v1/users/me", Some(params.clone()), true);
                }
            }
            crate::dispatch::config_auth::handle(host, "config.preferences_set", params)
        }

        "settings.privacy_get" => {
            match host.api_json(Method::GET, "/api/v1/users/me/privacy", None, true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "settings.privacy_update" => {
            match host.api_json(Method::PUT, "/api/v1/users/me/privacy", Some(params.clone()), true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "settings.register_push_token" => {
            {
                let mut cfg = host.host_config.lock();
                if let Some(apns) = params.get("apns_token").and_then(Value::as_str) {
                    cfg.apns_token = Some(apns.to_string());
                }
                if let Some(voip) = params.get("voip_token").and_then(Value::as_str) {
                    cfg.voip_token = Some(voip.to_string());
                }
            }
            let _ = host.persist_host_config();
            Some(ok_envelope(json!({ "registered": true })))
        }

        "settings.storage_usage" => {
            let cache_dir = host.app_data_dir.join("cache");
            let mut total_bytes: u64 = 0;
            let mut file_count: u64 = 0;
            if let Ok(entries) = std::fs::read_dir(&cache_dir) {
                for entry in entries.flatten() {
                    if let Ok(meta) = entry.metadata() {
                        if meta.is_file() {
                            total_bytes += meta.len();
                            file_count += 1;
                        }
                    }
                }
            }
            Some(ok_envelope(json!({
                "cache_bytes": total_bytes,
                "file_count": file_count,
                "app_data_dir": host.app_data_dir.to_string_lossy()
            })))
        }

        "settings.storage_clear" => {
            client.local_store.clear_all();
            Some(ok_envelope(json!({ "cleared": true })))
        }

        "settings.passcode_set" => {
            let Some(pin) = params.get("passcode").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing passcode").to_envelope_string());
            };
            match host.set_passcode(pin) {
                Ok(()) => Some(ok_envelope(json!({ "passcode_enabled": true }))),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "settings.passcode_verify" => {
            let Some(pin) = params.get("passcode").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing passcode").to_envelope_string());
            };
            match host.verify_passcode(pin) {
                Ok(valid) => Some(ok_envelope(json!({ "valid": valid }))),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "settings.passcode_clear" => {
            host.host_config.lock().passcode_phc = None;
            let _ = host.persist_host_config();
            Some(ok_envelope(json!({ "passcode_enabled": false })))
        }

        "settings.check_updates" => {
            crate::dispatch::config_auth::handle(host, "config.check_release_update", params)
        }

        // ====================================================================
        // Namespace 22: `wallet.*` (Non-Custodial TON Wallet)
        // ====================================================================
        "wallet.status" | "wallet.receive_address" => {
            let acct = host.wallet_service.get_active_account();
            let balance_nanos = host.wallet_service.get_cached_balance_nanotons();
            Some(ok_envelope(json!({
                "has_wallet": acct.is_some(),
                "account": acct,
                "balance_nanotons": balance_nanos
            })))
        }

        "wallet.create" | "wallet.import_mnemonic" => {
            let label = params
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or("Fabushi TON Wallet")
                .to_string();
            let seed_material = params
                .get("mnemonic")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let digest = sha256_digest(seed_material.as_bytes());
            let hex_part: String = digest.iter().take(20).map(|b| format!("{b:02x}")).collect();
            let ton_addr = format!("UQ{hex_part}");
            let account_id = format!("ton_{}", &hex_part[..12]);
            let now_secs = chrono::Utc::now().timestamp().max(0) as u64;
            let svc = host.wallet_service.clone();
            match host.block_on(async move {
                svc.create_new_wallet(&account_id, &ton_addr, &label, now_secs)
                    .await
            }) {
                Ok(acct) => {
                    if let Some(init_nanos) = params.get("initial_nanotons").and_then(Value::as_u64) {
                        host.wallet_service.set_balance_nanotons(init_nanos);
                    }
                    Some(ok_envelope(json!({
                        "account": acct,
                        "balance_nanotons": host.wallet_service.get_cached_balance_nanotons()
                    })))
                }
                Err(e) => Some(BridgeError::crypto(e.to_string()).to_envelope_string()),
            }
        }

        "wallet.balance" => {
            if client.token().is_some() && !*host.simulate_offline.lock() {
                let c = Arc::clone(&client);
                if let Ok(bal) = host.block_on(async move { c.wallet_balance().await }) {
                    return Some(ok_envelope(serde_json::to_value(bal).unwrap_or_default()));
                }
            }
            Some(ok_envelope(json!({
                "balance_nanotons": host.wallet_service.get_cached_balance_nanotons(),
                "account": host.wallet_service.get_active_account()
            })))
        }

        "wallet.send" => {
            let Some(recipient) = params.get("recipient").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing recipient").to_envelope_string());
            };
            let amount_nanos = params
                .get("amount_nanotons")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let fee_nanos = params
                .get("fee_nanotons")
                .and_then(Value::as_u64)
                .unwrap_or(5_000_000);
            let rec_s = recipient.to_string();
            let svc = host.wallet_service.clone();
            match host.block_on(async move { svc.send_transfer(&rec_s, amount_nanos, fee_nanos).await }) {
                Ok(tx_hash) => Some(ok_envelope(json!({
                    "tx_hash": tx_hash,
                    "remaining_nanotons": host.wallet_service.get_cached_balance_nanotons()
                }))),
                Err(e) => Some(BridgeError::invalid_params(e.to_string()).to_envelope_string()),
            }
        }

        "wallet.transactions" => {
            let c = Arc::clone(&client);
            match host.block_on(async move { c.wallet_transactions().await }) {
                Ok(txs) => Some(ok_envelope(serde_json::to_value(txs).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        // ====================================================================
        // Namespace 23: `stars.*` (Telegram Stars Ledger & Gifts)
        // ====================================================================
        "stars.balance" => {
            let c = Arc::clone(&client);
            match host.block_on(async move { c.wallet_balance().await }) {
                Ok(bal) => Some(ok_envelope(serde_json::to_value(bal).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "stars.transactions" => {
            let c = Arc::clone(&client);
            match host.block_on(async move { c.wallet_transactions().await }) {
                Ok(txs) => Some(ok_envelope(serde_json::to_value(txs).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "stars.topup" => {
            let amount = params
                .get("amount_stars")
                .and_then(Value::as_i64)
                .unwrap_or(100);
            let c = Arc::clone(&client);
            match host.block_on(async move { c.wallet_topup(amount).await }) {
                Ok(bal) => Some(ok_envelope(serde_json::to_value(bal).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "stars.send_gift" | "stars.pay_invoice" => {
            let Ok(req) = serde_json::from_value::<StarGiftRequest>(params.clone()) else {
                return Some(
                    BridgeError::invalid_params("Invalid StarGiftRequest").to_envelope_string(),
                );
            };
            let c = Arc::clone(&client);
            match host.block_on(async move { c.wallet_gift_stars(&req).await }) {
                Ok(bal) => Some(ok_envelope(serde_json::to_value(bal).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        // ====================================================================
        // Namespace 24: `alipay.*` (Alipay VIP Plans, Orders & Redeem Codes)
        // ====================================================================
        "alipay.plans" => {
            let plans = membership_plans();
            Some(ok_envelope(json!({ "plans": plans })))
        }

        "alipay.create_order" => {
            match host.api_json(Method::POST, "/api/v1/alipay/orders", Some(params.clone()), true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "alipay.order_status" => {
            let Some(order_no) = params.get("order_no").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing order_no").to_envelope_string());
            };
            match host.api_json(
                Method::GET,
                &format!("/api/v1/alipay/orders/{order_no}"),
                None,
                true,
            ) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "alipay.redeem_code" => {
            match host.api_json(Method::POST, "/api/v1/alipay/redeem", Some(params.clone()), true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        "alipay.vip_status" => {
            let uid = client.current_user_id().unwrap_or(0) as i64;
            let conn = host.dharma_db.lock();
            let profile = fabushi_miniapp::storage::ensure_profile(
                &conn,
                uid,
                "practitioner",
                "修行行者",
                "",
                false,
            )
            .unwrap_or_default();
            Some(ok_envelope(json!({
                "is_vip": profile.is_vip,
                "vip_plan": profile.vip_plan,
                "vip_expires_at": profile.vip_expires_at
            })))
        }

        _ => None,
    }
}
