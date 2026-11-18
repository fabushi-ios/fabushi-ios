use gpui_telegram_client::ClientDomainEvent;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::ffi::{c_char, c_void, CString};

/// Nullable C-ABI function pointer matching C `TelegramBridgeEventCallback`.
pub type TelegramBridgeEventCallbackFn = unsafe extern "C" fn(
    event_type: *const c_char,
    payload_json: *const c_char,
    user_context: *mut c_void,
);

#[derive(Clone, Copy, Default)]
struct CallbackEntry {
    callback: Option<TelegramBridgeEventCallbackFn>,
    user_context: usize,
}

#[derive(Default)]
pub struct EventBridgeRegistry {
    entry: Mutex<CallbackEntry>,
}

impl EventBridgeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        callback: Option<TelegramBridgeEventCallbackFn>,
        user_context: *mut c_void,
    ) {
        let mut guard = self.entry.lock();
        match callback {
            Some(cb) => {
                *guard = CallbackEntry {
                    callback: Some(cb),
                    user_context: user_context as usize,
                };
            }
            None => {
                *guard = CallbackEntry::default();
            }
        }
    }

    pub fn clear(&self) {
        *self.entry.lock() = CallbackEntry::default();
    }

    pub fn is_registered(&self) -> bool {
        self.entry.lock().callback.is_some()
    }

    pub fn emit(&self, event_type: &str, payload: &Value) {
        let snapshot = *self.entry.lock();
        let Some(callback) = snapshot.callback else {
            return;
        };

        let Ok(c_type) = CString::new(event_type.replace('\0', "")) else {
            return;
        };
        let Ok(c_payload) = CString::new(payload.to_string().replace('\0', "")) else {
            return;
        };

        unsafe {
            callback(
                c_type.as_ptr(),
                c_payload.as_ptr(),
                snapshot.user_context as *mut c_void,
            );
        }
    }

    pub fn emit_domain_event(&self, ev: &ClientDomainEvent) {
        let (event_type, payload) = match ev {
            ClientDomainEvent::NewMessage(msg) => (
                "newMessage",
                serde_json::to_value(msg).unwrap_or_else(|_| json!({})),
            ),
            ClientDomainEvent::MessageEdited(msg) => (
                "messageEdited",
                serde_json::to_value(msg).unwrap_or_else(|_| json!({})),
            ),
            ClientDomainEvent::MessagesDeleted {
                chat_id,
                message_ids,
            } => (
                "messageDeleted",
                json!({ "chat_id": chat_id.0, "message_ids": message_ids.iter().map(|m| m.0).collect::<Vec<_>>() }),
            ),
            ClientDomainEvent::ReactionsUpdated {
                chat_id,
                msg_id,
                reactions,
            } => (
                "reactionUpdated",
                json!({
                    "chat_id": chat_id.0,
                    "msg_id": msg_id.0,
                    "reactions": reactions,
                }),
            ),
            ClientDomainEvent::Typing(typing) => (
                "typing",
                serde_json::to_value(typing).unwrap_or_else(|_| json!({})),
            ),
            ClientDomainEvent::UserStatus {
                user_id,
                online,
                last_seen_ms,
            } => (
                "presenceUpdated",
                json!({
                    "user_id": user_id.0,
                    "online": online,
                    "last_seen_ms": last_seen_ms,
                }),
            ),
            ClientDomainEvent::ReadHistoryInbox { chat_id, max_id } => (
                "readReceipt",
                json!({ "chat_id": chat_id.0, "max_id": max_id.0, "direction": "inbox" }),
            ),
            ClientDomainEvent::ReadHistoryOutbox { chat_id, max_id } => (
                "readReceipt",
                json!({ "chat_id": chat_id.0, "max_id": max_id.0, "direction": "outbox" }),
            ),
            ClientDomainEvent::PollUpdated {
                chat_id,
                msg_id,
                poll,
            } => (
                "pollUpdated",
                json!({ "chat_id": chat_id.0, "msg_id": msg_id.0, "poll": poll }),
            ),
            ClientDomainEvent::CallSignal {
                call_id,
                from_user_id,
                signal_type,
                sdp,
                candidate,
                sdp_mid,
                sdp_mline_index,
            } => (
                "callSignal",
                json!({
                    "call_id": call_id,
                    "from_user_id": from_user_id,
                    "signal_type": signal_type,
                    "sdp": sdp,
                    "candidate": candidate,
                    "sdp_mid": sdp_mid,
                    "sdp_mline_index": sdp_mline_index,
                }),
            ),
            ClientDomainEvent::AgentStreamChunk {
                run_id,
                chat_id,
                msg_id,
                delta,
                done,
            } => (
                "agentStreamDelta",
                json!({
                    "run_id": run_id,
                    "chat_id": chat_id.0,
                    "msg_id": msg_id.map(|m| m.0),
                    "delta": delta,
                    "done": done,
                }),
            ),
            other => (
                "domainEvent",
                json!({ "debug": format!("{other:?}") }),
            ),
        };
        self.emit(event_type, &payload);
    }
}
