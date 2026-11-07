use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginRegistrationMode {
    Full,
    CliMetadata,
    Capabilities,
    SetupOnly,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundledChannelEntryFeatures {
    pub account_inspect: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundledChannelSetupEntryFeatures {
    pub legacy_state_migrations: bool,
    pub legacy_session_surfaces: bool,
}

#[derive(Debug, Clone)]
pub struct ChannelOutboundSessionRoute {
    pub session_key: String,
    pub channel: String,
    pub target: String,
    pub thread_id: Option<String>,
}

/// Builds an outbound session routing key and canonical route tuple.
pub fn build_channel_outbound_session_route(
    channel: &str,
    target: &str,
    thread_id: Option<&str>,
) -> ChannelOutboundSessionRoute {
    let clean_target = strip_channel_target_prefix(target, channel);
    let session_key = if let Some(th) = thread_id {
        format!("{channel}:{clean_target}:thread:{th}")
    } else {
        format!("{channel}:{clean_target}")
    };

    ChannelOutboundSessionRoute {
        session_key,
        channel: channel.to_string(),
        target: clean_target,
        thread_id: thread_id.map(String::from),
    }
}

/// Strips standard channel prefix from a target string (e.g. "telegram:12345" -> "12345").
pub fn strip_channel_target_prefix(target: &str, channel: &str) -> String {
    let trimmed = target.trim();
    let prefix = format!("{channel}:");
    if trimmed.to_ascii_lowercase().starts_with(&prefix.to_ascii_lowercase()) {
        trimmed[prefix.len()..].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Strips generic target kind prefix (e.g. "user:42" -> "42", "channel:general" -> "general").
pub fn strip_target_kind_prefix(target: &str) -> String {
    let trimmed = target.trim();
    if let Some(pos) = trimmed.find(':') {
        trimmed[pos + 1..].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Recover thread ID from a compound session key (e.g. "...:thread:12345" -> "12345").
pub fn recover_current_thread_session_id(session_key: &str) -> Option<String> {
    let marker = ":thread:";
    session_key.find(marker).map(|idx| session_key[idx + marker.len()..].to_string())
}

/// Parse optional comma/semicolon/newline-delimited string entries into a clean vector.
pub fn parse_optional_delimited_entries(raw: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(s) = raw {
        for token in s.split(|c| c == ',' || c == ';' || c == '\n' || c == '\r') {
            let trimmed = token.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
        }
    }
    out
}

/// Clears specified account entry fields from a channel configuration map.
pub fn clear_account_entry_fields(
    entry: &mut serde_json::Map<String, Value>,
    fields_to_clear: &[&str],
) -> usize {
    let mut count = 0;
    for f in fields_to_clear {
        if entry.remove(*f).is_some() {
            count += 1;
        }
    }
    count
}

/// Pure contract representation of a bundled channel's entrypoint definition.
#[derive(Clone)]
pub struct BundledChannelEntryContract {
    pub id: String,
    pub name: String,
    pub description: String,
    pub features: BundledChannelEntryFeatures,
    pub registration_handler: Arc<dyn Fn(PluginRegistrationMode) -> Vec<String> + Send + Sync>,
}

impl BundledChannelEntryContract {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            features: BundledChannelEntryFeatures::default(),
            registration_handler: Arc::new(|_| Vec::new()),
        }
    }

    pub fn with_features(mut self, features: BundledChannelEntryFeatures) -> Self {
        self.features = features;
        self
    }

    pub fn with_handler(
        mut self,
        handler: impl Fn(PluginRegistrationMode) -> Vec<String> + Send + Sync + 'static,
    ) -> Self {
        self.registration_handler = Arc::new(handler);
        self
    }

    pub fn register(&self, mode: PluginRegistrationMode) -> Vec<String> {
        (self.registration_handler)(mode)
    }
}

/// In-memory wire recorder for delivery trace testkit scenarios.
#[derive(Debug, Clone, Default)]
pub struct WireRecorder {
    events: Vec<WireTraceEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireTraceEvent {
    pub direction: String, // "inbound" | "outbound"
    pub event_type: String,
    pub payload_summary: String,
}

impl WireRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, direction: &str, event_type: &str, summary: &str) {
        self.events.push(WireTraceEvent {
            direction: direction.to_string(),
            event_type: event_type.to_string(),
            payload_summary: summary.to_string(),
        });
    }

    pub fn events(&self) -> &[WireTraceEvent] {
        &self.events
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}
