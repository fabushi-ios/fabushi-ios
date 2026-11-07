use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::extensions::transcript::channel_delivery_unregistered_error::SandChannelDeliveryUnregisteredError;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub const CHANNEL_MAX_MESSAGE_CHARS: usize = 4_000;

/// Split outbound text at channel-sized boundaries while keeping every
/// emitted chunk's markdown code fences balanced. The channel provider remains
/// responsible for transport-specific limits; this is the shared conservative
/// limit used by the Host delivery boundary.
pub fn split_channel_message(text: &str, limit: usize) -> Vec<String> {
    let limit = limit.max(1);
    if text.chars().count() <= limit {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut in_code_fence = false;
    let mut fence_lang = String::new();

    for line in text.lines() {
        let separator = usize::from(!current.is_empty());
        let projected = current.chars().count() + separator + line.chars().count();
        if projected > limit && !current.is_empty() {
            if in_code_fence {
                current.push_str("\n```");
                chunks.push(current);
                current = if fence_lang.is_empty() {
                    format!("```\n{line}")
                } else {
                    format!("```{fence_lang}\n{line}")
                };
            } else {
                chunks.push(current);
                current = line.to_string();
            }
        } else {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }

        if line.trim_start().starts_with("```") {
            if in_code_fence {
                in_code_fence = false;
                fence_lang.clear();
            } else {
                in_code_fence = true;
                fence_lang = line
                    .trim_start()
                    .trim_start_matches("```")
                    .trim()
                    .to_string();
            }
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// The source-level provenance of an allowlist candidate. Keeping this with
/// the match result lets the caller produce a diagnostic without reimplementing
/// normalization or leaking channel-specific text from the connector owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelAllowlistMatchSource {
    Wildcard,
    Id,
    Name,
    Tag,
    Username,
    PrefixedId,
    PrefixedUser,
    PrefixedName,
    Slug,
    Localpart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelAllowlistCandidate {
    pub value: Option<String>,
    pub source: ChannelAllowlistMatchSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelAllowlistMatch {
    pub allowed: bool,
    pub match_key: Option<String>,
    pub match_source: Option<ChannelAllowlistMatchSource>,
}

/// Evaluates the channel sender policy at the connector boundary. An empty
/// allowlist is deny-all, `*` allows every non-denylisted candidate, and the
/// denylist always wins over either form of allowlist access.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChannelAclGate {
    allowlist: HashSet<String>,
    denylist: HashSet<String>,
}

impl ChannelAclGate {
    pub fn new<I, J>(allowlist: I, denylist: J) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
        J: IntoIterator,
        J::Item: AsRef<str>,
    {
        Self {
            allowlist: Self::normalize_entries(allowlist),
            denylist: Self::normalize_entries(denylist),
        }
    }

    pub fn is_authorized(&self, sender_id: &str) -> bool {
        self.resolve_candidates(&[ChannelAllowlistCandidate {
            value: Some(sender_id.to_string()),
            source: ChannelAllowlistMatchSource::Id,
        }])
        .allowed
    }

    pub fn resolve_candidates(
        &self,
        candidates: &[ChannelAllowlistCandidate],
    ) -> ChannelAllowlistMatch {
        let normalized_candidates: Vec<(String, ChannelAllowlistMatchSource)> = candidates
            .iter()
            .filter_map(|candidate| {
                let value = normalize_channel_acl_value(candidate.value.as_deref().unwrap_or(""));
                (!value.is_empty()).then_some((value, candidate.source))
            })
            .collect();

        if self.allowlist.is_empty() {
            return ChannelAllowlistMatch {
                allowed: false,
                match_key: None,
                match_source: None,
            };
        }

        if self.allowlist.contains("*") {
            let allowed_candidate = normalized_candidates.iter().find(|(value, _)| {
                !self.denylist.contains("*") && !self.denylist.contains(value)
            });
            if allowed_candidate.is_none() {
                return ChannelAllowlistMatch {
                    allowed: false,
                    match_key: None,
                    match_source: None,
                };
            }
            return ChannelAllowlistMatch {
                allowed: true,
                match_key: Some("*".to_string()),
                match_source: Some(ChannelAllowlistMatchSource::Wildcard),
            };
        }

        for (value, source) in normalized_candidates {
            if self.denylist.contains("*") || self.denylist.contains(&value) {
                continue;
            }
            if self.allowlist.contains(&value) {
                return ChannelAllowlistMatch {
                    allowed: true,
                    match_key: Some(value),
                    match_source: Some(source),
                };
            }
        }

        ChannelAllowlistMatch {
            allowed: false,
            match_key: None,
            match_source: None,
        }
    }

    fn normalize_entries<I>(entries: I) -> HashSet<String>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        entries
            .into_iter()
            .map(|entry| normalize_channel_acl_value(entry.as_ref()))
            .filter(|entry| !entry.is_empty())
            .collect()
    }
}

fn normalize_channel_acl_value(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

/// Verifies a webhook's standard HMAC-SHA256 signature. Invalid encoding and
/// mismatched lengths fail closed, and the final comparison is constant-time.
pub fn verify_webhook_hmac_sha256(
    payload: &[u8],
    secret: &str,
    expected_hex: &str,
) -> bool {
    let Some(expected) = decode_sha256_hex(expected_hex) else {
        return false;
    };

    const BLOCK_SIZE: usize = 64;
    let mut key = secret.as_bytes().to_vec();
    if key.len() > BLOCK_SIZE {
        key = Sha256::digest(&key).to_vec();
    }
    key.resize(BLOCK_SIZE, 0);

    let mut inner_pad = [0x36_u8; BLOCK_SIZE];
    let mut outer_pad = [0x5c_u8; BLOCK_SIZE];
    for (index, byte) in key.iter().enumerate() {
        inner_pad[index] ^= byte;
        outer_pad[index] ^= byte;
    }

    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(payload);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    let actual = outer.finalize();
    actual.as_slice().ct_eq(&expected).into()
}

fn decode_sha256_hex(value: &str) -> Option<[u8; 32]> {
    let bytes = value.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut decoded = [0_u8; 32];
    for (index, pair) in bytes.chunks_exact(2).enumerate() {
        decoded[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Some(decoded)
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[derive(Debug, Clone, Default)]
pub struct InboundChannelDeduplicator {
    seen_messages: HashSet<(String, String)>,
    last_cursor: HashMap<String, u64>,
}

impl InboundChannelDeduplicator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `Ok(true)` for a newly admitted message, `Ok(false)` for an
    /// idempotent duplicate, and rejects a new message behind the channel's
    /// durable cursor.
    pub fn process_message(
        &mut self,
        channel_id: &str,
        message_id: &str,
        cursor: u64,
    ) -> Result<bool, &'static str> {
        let channel = normalize_channel_acl_value(channel_id);
        let message = message_id.trim().to_string();
        if channel.is_empty() || message.is_empty() {
            return Err("inbound channel message identity is empty");
        }
        let key = (channel.clone(), message);
        if self.seen_messages.contains(&key) {
            return Ok(false);
        }
        let current_cursor = self.last_cursor.entry(channel).or_insert(0);
        if cursor <= *current_cursor {
            return Err("inbound cursor is stale or out of order");
        }
        *current_cursor = cursor;
        self.seen_messages.insert(key);
        Ok(true)
    }
}

const MAX_INBOUND_DEBOUNCE_DELAY: Duration = Duration::from_secs(60);
const MAX_INBOUND_DEBOUNCE_CHANNELS: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelInboundMessageKind {
    Text,
    ControlCommand(String),
    Media,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelInboundDebouncePolicy {
    default_delay: Duration,
    channel_delays: HashMap<String, Duration>,
}

impl ChannelInboundDebouncePolicy {
    pub fn new(default_delay: Duration) -> Result<Self, ChannelInboundDebounceError> {
        validate_inbound_debounce_delay(default_delay)?;
        Ok(Self {
            default_delay,
            channel_delays: HashMap::new(),
        })
    }

    pub fn set_channel_delay(
        &mut self,
        channel_id: &str,
        delay: Duration,
    ) -> Result<(), ChannelInboundDebounceError> {
        let channel = normalize_channel_acl_value(channel_id);
        if channel.is_empty() {
            return Err(ChannelInboundDebounceError::EmptyChannel);
        }
        validate_inbound_debounce_delay(delay)?;
        if !self.channel_delays.contains_key(&channel)
            && self.channel_delays.len() >= MAX_INBOUND_DEBOUNCE_CHANNELS
        {
            return Err(ChannelInboundDebounceError::ChannelLimitReached);
        }
        self.channel_delays.insert(channel, delay);
        Ok(())
    }

    fn delay_for(&self, channel_id: &str) -> Duration {
        self.channel_delays
            .get(channel_id)
            .copied()
            .unwrap_or(self.default_delay)
    }

    fn max_delay(&self) -> Duration {
        self.channel_delays
            .values()
            .copied()
            .max()
            .unwrap_or(self.default_delay)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelInboundDebounceDecision {
    Immediate,
    Debounced { retry_after: Duration },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelInboundDebounceError {
    EmptyChannel,
    DelayTooLong,
    ChannelLimitReached,
    InvalidControlCommand,
}

fn is_immediate_control_command(command: &str) -> bool {
    matches!(
        command
            .trim()
            .trim_start_matches('/')
            .to_ascii_lowercase()
            .as_str(),
        "abort" | "stop" | "status"
    )
}

#[derive(Debug, Clone, Default)]
pub struct InboundChannelDebounceGate {
    last_text_admitted: HashMap<String, Instant>,
}

impl InboundChannelDebounceGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies only the debounce policy. Transport receipt, deduplication,
    /// timers, and turn dispatch remain owned by their existing boundaries.
    pub fn admit(
        &mut self,
        policy: &ChannelInboundDebouncePolicy,
        channel_id: &str,
        kind: &ChannelInboundMessageKind,
        now: Instant,
    ) -> Result<ChannelInboundDebounceDecision, ChannelInboundDebounceError> {
        let channel = normalize_channel_acl_value(channel_id);
        if channel.is_empty() {
            return Err(ChannelInboundDebounceError::EmptyChannel);
        }

        self.last_text_admitted.retain(|_, admitted_at| {
            now.checked_duration_since(*admitted_at)
                .is_none_or(|elapsed| elapsed <= policy.max_delay())
        });

        match kind {
            ChannelInboundMessageKind::Media => {
                return Ok(ChannelInboundDebounceDecision::Immediate);
            }
            ChannelInboundMessageKind::ControlCommand(command)
                if is_immediate_control_command(command) =>
            {
                return Ok(ChannelInboundDebounceDecision::Immediate);
            }
            ChannelInboundMessageKind::ControlCommand(_) => {
                return Err(ChannelInboundDebounceError::InvalidControlCommand);
            }
            ChannelInboundMessageKind::Text => {}
        }

        let delay = policy.delay_for(&channel);
        if let Some(admitted_at) = self.last_text_admitted.get(&channel).copied() {
            if let Some(elapsed) = now.checked_duration_since(admitted_at) {
                if elapsed < delay {
                    return Ok(ChannelInboundDebounceDecision::Debounced {
                        retry_after: delay - elapsed,
                    });
                }
            }
        }

        if !self.last_text_admitted.contains_key(&channel)
            && self.last_text_admitted.len() >= MAX_INBOUND_DEBOUNCE_CHANNELS
        {
            let oldest = self
                .last_text_admitted
                .iter()
                .min_by_key(|(_, admitted_at)| **admitted_at)
                .map(|(channel, _)| channel.clone());
            if let Some(oldest) = oldest {
                self.last_text_admitted.remove(&oldest);
            }
        }
        self.last_text_admitted.insert(channel, now);
        Ok(ChannelInboundDebounceDecision::Immediate)
    }
}

fn validate_inbound_debounce_delay(
    delay: Duration,
) -> Result<(), ChannelInboundDebounceError> {
    if delay > MAX_INBOUND_DEBOUNCE_DELAY {
        Err(ChannelInboundDebounceError::DelayTooLong)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod inbound_debounce_tests {
    use super::*;

    #[test]
    fn control_and_media_bypass_without_starting_text_window() {
        let mut policy = ChannelInboundDebouncePolicy::new(Duration::from_secs(5)).unwrap();
        policy
            .set_channel_delay("Telegram-General", Duration::from_secs(2))
            .unwrap();
        let mut gate = InboundChannelDebounceGate::new();
        let now = Instant::now();

        assert_eq!(
            gate.admit(
                &policy,
                "telegram-general",
                &ChannelInboundMessageKind::ControlCommand(" /STOP ".into()),
                now,
            )
            .unwrap(),
            ChannelInboundDebounceDecision::Immediate
        );
        assert_eq!(
            gate.admit(
                &policy,
                "telegram-general",
                &ChannelInboundMessageKind::Media,
                now,
            )
            .unwrap(),
            ChannelInboundDebounceDecision::Immediate
        );
    }

    #[test]
    fn text_uses_channel_override_and_expires() {
        let mut policy = ChannelInboundDebouncePolicy::new(Duration::from_secs(5)).unwrap();
        policy
            .set_channel_delay("fast", Duration::from_secs(2))
            .unwrap();
        let mut gate = InboundChannelDebounceGate::new();
        let now = Instant::now();

        assert_eq!(
            gate.admit(&policy, "fast", &ChannelInboundMessageKind::Text, now)
                .unwrap(),
            ChannelInboundDebounceDecision::Immediate
        );
        assert_eq!(
            gate.admit(
                &policy,
                "fast",
                &ChannelInboundMessageKind::Text,
                now + Duration::from_millis(500),
            )
            .unwrap(),
            ChannelInboundDebounceDecision::Debounced {
                retry_after: Duration::from_millis(1_500)
            }
        );
        assert_eq!(
            gate.admit(
                &policy,
                "fast",
                &ChannelInboundMessageKind::Text,
                now + Duration::from_secs(2),
            )
            .unwrap(),
            ChannelInboundDebounceDecision::Immediate
        );
        assert_eq!(
            gate.admit(
                &policy,
                "slow",
                &ChannelInboundMessageKind::Text,
                now + Duration::from_millis(500),
            )
            .unwrap(),
            ChannelInboundDebounceDecision::Immediate
        );
    }

    #[test]
    fn invalid_policy_input_fails_closed() {
        assert_eq!(
            ChannelInboundDebouncePolicy::new(Duration::from_secs(61)),
            Err(ChannelInboundDebounceError::DelayTooLong)
        );
        let mut policy = ChannelInboundDebouncePolicy::new(Duration::ZERO).unwrap();
        assert_eq!(
            policy.set_channel_delay("  ", Duration::from_secs(1)),
            Err(ChannelInboundDebounceError::EmptyChannel)
        );
        let mut gate = InboundChannelDebounceGate::new();
        assert_eq!(
            gate.admit(
                &policy,
                "  ",
                &ChannelInboundMessageKind::Text,
                Instant::now(),
            ),
            Err(ChannelInboundDebounceError::EmptyChannel)
        );
        assert_eq!(
            gate.admit(
                &policy,
                "channel",
                &ChannelInboundMessageKind::ControlCommand("/unknown".into()),
                Instant::now(),
            ),
            Err(ChannelInboundDebounceError::InvalidControlCommand)
        );
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChannelThreadRouter {
    thread_to_conversation: HashMap<(String, String), String>,
}

impl ChannelThreadRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn resolve_or_create(&mut self, channel_id: &str, thread_id: Option<&str>) -> String {
        let channel = normalize_channel_acl_value(channel_id);
        let thread = thread_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("default_root")
            .to_string();
        let key = (channel.clone(), thread.clone());
        if let Some(scope_id) = self.thread_to_conversation.get(&key) {
            return scope_id.clone();
        }
        // Length-prefix both components so distinct `(channel, thread)` pairs
        // cannot alias when either provider-owned value contains `-`.
        let new_scope = format!(
            "conv-{}:{}{}:{}",
            channel.len(),
            channel,
            thread.len(),
            thread
        );
        self.thread_to_conversation.insert(key, new_scope.clone());
        new_scope
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelOutboundMessage {
    Text { text: String, operation_id: Option<String> },
    Attachment {
        url: String,
        caption: Option<String>,
        operation_id: Option<String>,
    },
}

impl ChannelOutboundMessage {
    pub fn operation_id(&self) -> Option<&str> {
        match self {
            Self::Text { operation_id, .. } | Self::Attachment { operation_id, .. } => {
                operation_id.as_deref()
            }
        }
    }
}

pub trait ChannelDeliveryPort: Send + Sync {
    fn deliver(
        &self,
        agent_id: &str,
        address_token: &str,
        message: &ChannelOutboundMessage,
    ) -> Result<(), String>;
}

/// Routes channel-prefixed addresses to provider-owned delivery ports.
///
/// This is a delivery boundary only: provider ports own transport/auth and the
/// canonical HostChannelDelivery owner owns lifecycle and registration. It does
/// not create channel state, sessions, or another agent/runtime loop.
#[derive(Clone, Default)]
pub struct ChannelDeliveryRouter {
    ports: Arc<RwLock<HashMap<String, Arc<dyn ChannelDeliveryPort>>>>,
}

impl ChannelDeliveryRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        channel_id: impl Into<String>,
        port: Arc<dyn ChannelDeliveryPort>,
    ) -> Result<(), String> {
        let channel_id = channel_id.into().trim().to_ascii_lowercase();
        if channel_id.is_empty() {
            return Err("channel delivery registration requires a channel id".into());
        }
        let mut ports = self
            .ports
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if ports.contains_key(&channel_id) {
            return Err(format!("channel delivery provider already registered: {channel_id}"));
        }
        ports.insert(channel_id, port);
        Ok(())
    }

    pub fn replace(
        &self,
        channel_id: impl Into<String>,
        port: Arc<dyn ChannelDeliveryPort>,
    ) -> Result<(), String> {
        let channel_id = channel_id.into().trim().to_ascii_lowercase();
        if channel_id.is_empty() {
            return Err("channel delivery replacement requires a channel id".into());
        }
        self.ports
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(channel_id, port);
        Ok(())
    }

    pub fn unregister(&self, channel_id: &str) -> bool {
        self.ports
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&channel_id.trim().to_ascii_lowercase())
            .is_some()
    }

    fn resolve(&self, address_token: &str) -> Option<Arc<dyn ChannelDeliveryPort>> {
        let channel_id = address_token.split_once(':')?.0.trim().to_ascii_lowercase();
        self.ports
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&channel_id)
            .cloned()
    }
}

impl ChannelDeliveryPort for ChannelDeliveryRouter {
    fn deliver(
        &self,
        agent_id: &str,
        address_token: &str,
        message: &ChannelOutboundMessage,
    ) -> Result<(), String> {
        let Some(port) = self.resolve(address_token) else {
            return Err(SandChannelDeliveryUnregisteredError.to_string());
        };
        match message {
            ChannelOutboundMessage::Text { text, operation_id } => {
                for (index, chunk) in split_channel_message(text, CHANNEL_MAX_MESSAGE_CHARS)
                    .into_iter()
                    .enumerate()
                {
                    let chunk_operation_id = operation_id.as_ref().map(|id| {
                        format!(
                            "{id}:chunk-{index}",
                            index = index + 1,
                        )
                    });
                    let chunk_message = ChannelOutboundMessage::Text {
                        text: chunk,
                        operation_id: chunk_operation_id,
                    };
                    port.deliver(agent_id, address_token, &chunk_message)?;
                }
                Ok(())
            }
            ChannelOutboundMessage::Attachment { .. } => {
                port.deliver(agent_id, address_token, message)
            }
        }
    }
}

#[derive(Default)]
pub struct UnregisteredChannelDeliveryPort;

impl ChannelDeliveryPort for UnregisteredChannelDeliveryPort {
    fn deliver(
        &self,
        _agent_id: &str,
        _address_token: &str,
        _message: &ChannelOutboundMessage,
    ) -> Result<(), String> {
        Err(SandChannelDeliveryUnregisteredError.to_string())
    }
}

#[derive(Clone)]
pub struct HostChannelDelivery {
    port: Arc<RwLock<Arc<dyn ChannelDeliveryPort>>>,
}

impl Default for HostChannelDelivery {
    fn default() -> Self {
        Self::new(Arc::new(UnregisteredChannelDeliveryPort))
    }
}

impl HostChannelDelivery {
    pub fn new(port: Arc<dyn ChannelDeliveryPort>) -> Self {
        Self {
            port: Arc::new(RwLock::new(port)),
        }
    }

    pub fn set_port(&self, port: Arc<dyn ChannelDeliveryPort>) {
        *self
            .port
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = port;
    }

    pub fn reset(&self) {
        self.set_port(Arc::new(UnregisteredChannelDeliveryPort));
    }

    pub fn deliver(
        &self,
        agent_id: &str,
        address_token: &str,
        message: &ChannelOutboundMessage,
    ) -> Result<(), String> {
        let port = self
            .port
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        port.deliver(agent_id, address_token, message)
    }
}
