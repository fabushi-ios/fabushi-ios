use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InboundEnvelope {
    pub from: String,
    pub channel: String,
    pub body: String,
    pub timestamp_ms: u64,
}

/// Formats an inbound message into standard agent envelope text.
pub fn format_inbound_envelope(from: &str, channel: &str, body: &str, timestamp_ms: u64) -> String {
    format!("[{channel}] {from} ({timestamp_ms}): {body}")
}

/// Resolves standard location text from coordinates.
pub fn format_location_text(latitude: f64, longitude: f64, name: Option<&str>) -> String {
    if let Some(n) = name {
        format!("📍 {n} ({latitude:.6}, {longitude:.6})")
    } else {
        format!("📍 Location: {latitude:.6}, {longitude:.6}")
    }
}

/// Simple debounce state tracker for inbound text messages.
pub struct InboundDebouncer {
    debounce_duration: Duration,
    last_seen: HashMap<String, Instant>,
}

impl InboundDebouncer {
    pub fn new(debounce_ms: u64) -> Self {
        Self {
            debounce_duration: Duration::from_millis(debounce_ms),
            last_seen: HashMap::new(),
        }
    }

    /// Returns true if the message from sender should be debounced (dropped/coalesced).
    pub fn should_debounce(&mut self, sender_key: &str) -> bool {
        let now = Instant::now();
        if let Some(last) = self.last_seen.get(sender_key) {
            if now.duration_since(*last) < self.debounce_duration {
                return true;
            }
        }
        self.last_seen.insert(sender_key.to_string(), now);
        false
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelJoinedRoomContext {
    pub channel: String,
    pub room_id: String,
    pub inviter_id: Option<String>,
}

/// Reports a bot joining a channel room/chat, returning a greeting status outcome.
pub fn report_channel_room_join(context: &ChannelJoinedRoomContext) -> String {
    format!(
        "Joined room {} on channel {}{}",
        context.room_id,
        context.channel,
        context
            .inviter_id
            .as_ref()
            .map(|inv| format!(" (invited by {inv})"))
            .unwrap_or_default()
    )
}
