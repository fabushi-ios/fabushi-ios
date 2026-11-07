use std::sync::atomic::{AtomicI64, Ordering};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};

use gpui_telegram_core::types::{ChatId, MessageId, PeerId, UserId};
use gpui_telegram_media::waveform::{decode_waveform_to_floats, encode_waveform_from_floats};
use gpui_telegram_proto::message::{MessageContent, MessageItem, ReplyHeader};

use crate::error::{make_error_json, make_success_json};
use crate::host::TelegramAppHost;

static NEXT_MESSAGE_ID: AtomicI64 = AtomicI64::new(1001);

#[derive(Debug, Deserialize)]
struct RequestWrapper {
    #[serde(alias = "method")]
    pub command: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    #[allow(dead_code)]
    pub id: Option<Value>,
}

struct LeaderGuard<'a> {
    host: &'a TelegramAppHost,
    sender_id: u64,
    nonce: u64,
    #[allow(dead_code)]
    slot: std::sync::Arc<crate::dedup::InFlightSlot>,
    completed: bool,
}

impl<'a> Drop for LeaderGuard<'a> {
    fn drop(&mut self) {
        if !self.completed {
            self.host.dedup_store.lock().abort(self.sender_id, self.nonce);
        }
    }
}

pub fn route_command(host: &TelegramAppHost, request_json: &str) -> String {
    let req: RequestWrapper = match serde_json::from_str(request_json) {
        Ok(r) => r,
        Err(e) => return make_error_json("INVALID_JSON", &format!("Parse error: {e}")),
    };

    let command = match req.command.as_deref() {
        Some(cmd) => cmd,
        None => return make_error_json("MISSING_COMMAND", "Request missing command or method"),
    };

    let params = req.params.unwrap_or(Value::Null);

    // Intentional debug panic trigger for panic isolation verification
    if command == "debug.trigger_panic" {
        panic!("Intentional test panic for catch_unwind verification");
    }

    match command {
        // --- Core Ping & Connectivity ---
        "telegram.ping" | "ping" => {
            let nonce = params.get("nonce").and_then(|v| v.as_u64()).unwrap_or(0);
            make_success_json(&json!({
                "pong": true,
                "nonce": nonce,
                "timestamp": Utc::now().timestamp_millis()
            }))
        }

        // --- Proto & Protocol Configuration ---
        "telegram.proto.getConfig" | "proto.getConfig" | "telegram.proto.maxPacketSize" => {
            make_success_json(&json!({
                "max_packet_size": 16 * 1024 * 1024,
                "current_layer": 188,
                "min_layer": 18
            }))
        }

        // --- Chat & Messaging ---
        "telegram.sendMessage" | "chat.sendMessage" => {
            let chat_id = params.get("chat_id").and_then(|v| v.as_i64()).unwrap_or(0);
            let text = params
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let sender_id = params.get("sender_id").and_then(|v| v.as_u64()).unwrap_or(1);
            let reply_to = params.get("reply_to").and_then(|v| v.as_i64());

            // Extract 64-bit client nonce
            let client_nonce = params.get("client_nonce").and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_i64().and_then(|n| if n >= 0 { Some(n as u64) } else { None }))
                    .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
            });

            let now = Utc::now();

            // 24-Hour Deduplication Check via Atomic In-Flight Reservation
            let mut leader_guard: Option<LeaderGuard> = None;
            if let Some(nonce) = client_nonce {
                loop {
                    let outcome = host.dedup_store.lock().reserve(sender_id, nonce, now);
                    match outcome {
                        crate::dedup::ReservationOutcome::AlreadyExists(cached) => {
                            return make_success_json(&json!({
                                "id": cached.message_id,
                                "chat_id": cached.chat_id,
                                "sender_id": cached.sender_id,
                                "text": cached.text,
                                "timestamp": cached.timestamp,
                                "reply_to": cached.reply_to,
                                "client_nonce": cached.client_nonce,
                                "status": "sent",
                                "duplicate": true
                            }));
                        }
                        crate::dedup::ReservationOutcome::Follower(slot) => {
                            match slot.wait() {
                                Ok(cached) => {
                                    return make_success_json(&json!({
                                        "id": cached.message_id,
                                        "chat_id": cached.chat_id,
                                        "sender_id": cached.sender_id,
                                        "text": cached.text,
                                        "timestamp": cached.timestamp,
                                        "reply_to": cached.reply_to,
                                        "client_nonce": cached.client_nonce,
                                        "status": "sent",
                                        "duplicate": true
                                    }));
                                }
                                Err(crate::dedup::DedupWaitError::Aborted) => {
                                    // Leader thread aborted/panicked; retry reservation
                                    continue;
                                }
                            }
                        }
                        crate::dedup::ReservationOutcome::Leader(slot) => {
                            leader_guard = Some(LeaderGuard {
                                host,
                                sender_id,
                                nonce,
                                slot,
                                completed: false,
                            });
                            break;
                        }
                    }
                }
            }

            let msg_id = NEXT_MESSAGE_ID.fetch_add(1, Ordering::SeqCst);
            let timestamp = now.timestamp_millis();

            let mut message_item = MessageItem::new(
                MessageId(msg_id),
                ChatId(chat_id),
                UserId(sender_id),
                PeerId::User(UserId(sender_id)),
                MessageContent::plain_text(text.clone()),
            );

            if let Some(r) = reply_to {
                message_item.reply_to = Some(ReplyHeader::new(MessageId(r)));
            }

            host.client.local_store.insert_message(message_item);

            // Record into deduplication cache and complete in-flight reservation
            if let Some(mut guard) = leader_guard {
                let cached = crate::dedup::CachedMessage {
                    message_id: msg_id,
                    chat_id,
                    sender_id,
                    text: text.clone(),
                    timestamp,
                    reply_to,
                    client_nonce: guard.nonce,
                    created_at: now,
                };
                host.dedup_store.lock().complete(sender_id, guard.nonce, cached);
                guard.completed = true;
            }

            // Emit push event
            host.emit_event(
                "newMessage",
                &json!({
                    "id": msg_id,
                    "chat_id": chat_id,
                    "sender_id": sender_id,
                    "text": text,
                    "timestamp": timestamp,
                    "reply_to": reply_to,
                    "client_nonce": client_nonce
                })
                .to_string(),
            );

            make_success_json(&json!({
                "id": msg_id,
                "chat_id": chat_id,
                "sender_id": sender_id,
                "text": text,
                "timestamp": timestamp,
                "reply_to": reply_to,
                "client_nonce": client_nonce,
                "status": "sent"
            }))
        }

        "telegram.getMessages" | "chat.getMessages" => {
            let chat_id = params.get("chat_id").and_then(|v| v.as_i64()).unwrap_or(0);
            match host.client.get_messages(ChatId(chat_id)) {
                Ok(messages) => make_success_json(&messages),
                Err(e) => make_error_json("STORE_ERROR", &e.to_string()),
            }
        }

        "telegram.getChats" | "chat.getChats" => {
            // Default sample dialogs for fresh state
            let sample_chats = json!([
                {
                    "id": 100,
                    "title": "Saved Messages",
                    "kind": "saved",
                    "unread_count": 0,
                    "last_message": "Welcome to Fabushi iOS Telegram Core",
                    "timestamp": Utc::now().timestamp_millis()
                },
                {
                    "id": 200,
                    "title": "Fabushi Community",
                    "kind": "group",
                    "unread_count": 2,
                    "last_message": "Native iOS 1:1 Telegram UI is live!",
                    "timestamp": Utc::now().timestamp_millis()
                }
            ]);
            make_success_json(&sample_chats)
        }

        // --- Sync & Difference ---
        "telegram.syncDifference" | "sync.getDifference" => {
            let pts = params.get("pts").and_then(|v| v.as_u64()).unwrap_or(0);
            let diff_resp = json!({
                "pts": pts + 1,
                "pts_count": 1,
                "has_more": false,
                "updates": []
            });

            host.emit_event(
                "syncDifference",
                &json!({
                    "pts": pts + 1,
                    "has_more": false
                })
                .to_string(),
            );

            make_success_json(&diff_resp)
        }

        // --- Media & Waveform ---
        "telegram.media.encodeWaveform" | "media.encodeWaveform" => {
            let samples: Vec<f32> = params
                .get("samples")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect())
                .unwrap_or_default();

            let encoded = encode_waveform_from_floats(&samples);
            make_success_json(&json!({
                "waveform_bytes": encoded,
                "byte_count": encoded.len()
            }))
        }

        "telegram.media.decodeWaveform" | "media.decodeWaveform" => {
            let bytes: Vec<u8> = params
                .get("waveform_bytes")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|x| x.as_u64().map(|b| b as u8)).collect())
                .unwrap_or_default();

            let floats = decode_waveform_to_floats(&bytes, 100);
            make_success_json(&json!({
                "samples": floats,
                "sample_count": floats.len()
            }))
        }

        // --- Agent, Tasks & CUA ---
        "telegram.agent.listTasks" | "agent.listTasks" => {
            let tasks = host.task_registry.lock();
            let values: Vec<_> = tasks.values().cloned().collect();
            make_success_json(&values)
        }

        "telegram.agent.createTask" | "agent.createTask" => {
            let task_id = params
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("task-default")
                .to_string();

            let mut tasks = host.task_registry.lock();
            let task = gpui_telegram_agent::tasks::TaskFsm::new(task_id.clone());
            tasks.insert(task_id.clone(), task.clone());

            host.emit_event(
                "taskStateUpdated",
                &json!({
                    "taskId": task_id,
                    "state": "pending"
                })
                .to_string(),
            );

            make_success_json(&task)
        }

        "telegram.agent.decideApproval" | "agent.decideApproval" => {
            let req_id = params
                .get("request_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let approved = params.get("approved").and_then(|v| v.as_bool()).unwrap_or(false);

            let mut dock = host.tool_dock.lock();
            let resolved = dock.resolve(req_id, approved).is_some();

            make_success_json(&json!({
                "request_id": req_id,
                "resolved": resolved,
                "approved": approved
            }))
        }

        // --- Account Generation Fence ---
        "telegram.generation.current" => {
            let fence = host.generation_fence.lock();
            make_success_json(&json!({ "generation": fence.current() }))
        }

        "telegram.generation.bump" => {
            let fence = host.generation_fence.lock();
            let new_gen = fence.bump();
            make_success_json(&json!({ "generation": new_gen }))
        }

        // --- WebRTC Call Session ---
        "telegram.webrtc.createSession" | "webrtc.createSession" => {
            let peer_id = params
                .get("peer_id")
                .and_then(|v| v.as_str())
                .unwrap_or("peer-unknown");
            let session_id = format!("call-{}", Utc::now().timestamp_micros());

            make_success_json(&json!({
                "sessionId": session_id,
                "peerId": peer_id,
                "state": "initiating",
                "createdAt": Utc::now().to_rfc3339()
            }))
        }

        // --- Fallback / Unknown Command ---
        other => {
            // Default generic success for unhandled test/mock RPCs
            make_success_json(&json!({
                "status": "acknowledged",
                "command": other,
                "receivedParams": params
            }))
        }
    }
}
