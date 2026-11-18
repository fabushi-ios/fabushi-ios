use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
    NewMessage {
        chat_id: i64,
        message_id: i64,
        sender_id: i64,
        text: String,
        pts: u64,
    },
    MessageEdited {
        chat_id: i64,
        message_id: i64,
        new_text: String,
    },
    MessageDeleted {
        chat_id: i64,
        message_ids: Vec<i64>,
    },
    SessionRevoked {
        user_id: u64,
        session_id: String,
        reason: String,
    },
    UserStatusUpdate {
        user_id: u64,
        online: bool,
        last_seen_ms: i64,
    },
    Typing {
        chat_id: i64,
        user_id: u64,
    },
    RawJson {
        event_type: String,
        data: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerEvent {
    pub event_id: String,
    pub channel: String,
    pub timestamp_ms: i64,
    pub origin_node_id: String,
    pub payload: EventPayload,
}

impl ServerEvent {
    pub fn new(channel: impl Into<String>, origin_node_id: impl Into<String>, payload: EventPayload) -> Self {
        Self {
            event_id: uuid::Uuid::new_v4().to_string(),
            channel: channel.into(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            origin_node_id: origin_node_id.into(),
            payload,
        }
    }
}
