use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const OUTBOUND_ECHO_WINDOW: Duration = Duration::from_secs(30);
pub const OUTBOUND_ECHO_MAX_ENTRIES: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundMessageIdentity {
    pub channel: String,
    pub conversation_id: String,
    pub account_id: Option<String>,
    pub message_id: Option<String>,
    pub source_id: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct OutboundEchoEntry {
    expires_at: Instant,
    sequence: u64,
}

#[derive(Clone, Default)]
pub struct OutboundEchoTracker {
    recent: Arc<Mutex<HashMap<String, OutboundEchoEntry>>>,
    window: Duration,
    next_sequence: Arc<Mutex<u64>>,
}

impl OutboundEchoTracker {
    pub fn new() -> Self {
        Self::with_window(OUTBOUND_ECHO_WINDOW)
    }

    pub fn with_window(window: Duration) -> Self {
        Self {
            recent: Arc::new(Mutex::new(HashMap::new())),
            window,
            next_sequence: Arc::new(Mutex::new(0)),
        }
    }

    pub fn record(&self, identity: OutboundMessageIdentity) {
        let keys = identity.keys();
        if keys.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut recent = self.recent.lock().unwrap();
        recent.retain(|_, entry| entry.expires_at > now);
        let expires_at = now + self.window;
        for key in keys {
            recent.remove(&key);
            while recent.len() >= OUTBOUND_ECHO_MAX_ENTRIES {
                let Some(oldest_key) = recent
                    .iter()
                    .min_by_key(|(_, entry)| entry.sequence)
                    .map(|(key, _)| key.clone())
                else {
                    break;
                };
                recent.remove(&oldest_key);
            }
            let mut sequence = self.next_sequence.lock().unwrap();
            *sequence = sequence.wrapping_add(1);
            let current_sequence = *sequence;
            drop(sequence);
            recent.insert(
                key,
                OutboundEchoEntry {
                    expires_at,
                    sequence: current_sequence,
                },
            );
        }
    }

    pub fn is_recent(&self, identity: &OutboundMessageIdentity) -> bool {
        let keys = identity.keys();
        if keys.is_empty() {
            return false;
        }
        let now = Instant::now();
        let mut recent = self.recent.lock().unwrap();
        keys.into_iter().any(|key| {
            let Some(entry) = recent.get(&key).copied() else {
                return false;
            };
            if entry.expires_at <= now {
                recent.remove(&key);
                false
            } else {
                true
            }
        })
    }

    pub fn clear(&self) {
        self.recent.lock().unwrap().clear();
    }
}

impl OutboundMessageIdentity {
    fn keys(&self) -> Vec<String> {
        let channel = self.channel.trim().to_ascii_lowercase();
        let conversation_id = self.conversation_id.trim();
        if channel.is_empty() || conversation_id.is_empty() {
            return Vec::new();
        }
        let account_id = self
            .account_id
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let scope = [channel, account_id, conversation_id.to_string()];
        [
            ("message", self.message_id.as_deref()),
            ("source", self.source_id.as_deref()),
        ]
        .into_iter()
        .filter_map(|(kind, value)| {
            let value = value?.trim();
            (!value.is_empty()).then(|| {
                serde_json::to_string(&(scope.clone(), kind, value))
                    .expect("outbound echo identity key is serializable")
            })
        })
        .collect()
    }
}

/// Helper that controls draft streaming chunking boundaries.
#[derive(Debug, Clone)]
pub struct ChannelDraftStreamingChunking {
    pub min_chars: usize,
    pub max_chars: usize,
    pub break_preference: String,
}

#[derive(Debug, Clone, Default)]
pub struct ChannelDraftStreamingChunkingOverride {
    pub min_chars: Option<usize>,
    pub max_chars: Option<usize>,
    pub break_preference: Option<String>,
}

impl Default for ChannelDraftStreamingChunking {
    fn default() -> Self {
        Self {
            min_chars: 200,
            max_chars: 800,
            break_preference: "paragraph".to_string(),
        }
    }
}

impl ChannelDraftStreamingChunking {
    /// Resolves the locked OpenClaw draft-streaming contract without owning
    /// channel configuration or text-limit lookup. Account configuration is
    /// the complete override when present; otherwise channel configuration is
    /// used. The caller supplies the already-resolved text limit.
    pub fn resolve(
        text_limit: usize,
        channel: Option<&ChannelDraftStreamingChunkingOverride>,
        account: Option<&ChannelDraftStreamingChunkingOverride>,
    ) -> Self {
        let selected = account.or(channel);
        let max_requested = selected
            .and_then(|config| config.max_chars)
            .unwrap_or(800)
            .max(1);
        let max_chars = max_requested.min(text_limit.max(1));
        let min_chars = selected
            .and_then(|config| config.min_chars)
            .unwrap_or(200)
            .max(1)
            .min(max_chars);
        let break_preference = match selected.and_then(|config| config.break_preference.as_deref()) {
            Some("newline") => "newline",
            Some("sentence") => "sentence",
            _ => "paragraph",
        };
        Self {
            min_chars,
            max_chars,
            break_preference: break_preference.to_string(),
        }
    }

    pub fn should_split_chunk(&self, current_len: usize) -> bool {
        current_len >= self.max_chars
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReplyPayload {
    pub text: String,
    pub media_paths: Vec<String>,
}

/// Preserves the source reply media selections when merging or recovering payload text.
pub fn preserve_reply_payload_media_selection(
    source: &ReplyPayload,
    recovered: &mut ReplyPayload,
) {
    if !source.media_paths.is_empty() {
        recovered.media_paths = source.media_paths.clone();
    }
}
