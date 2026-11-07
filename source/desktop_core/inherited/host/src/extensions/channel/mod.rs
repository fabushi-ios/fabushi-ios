//! Unified Multi-Channel Ingress, Outbound Delivery, and Runtime Pipeline (OC-S3).
//!
//! Provides the end-to-end multi-channel orchestration pipeline:
//! 1. Inbound webhook signature & auth verification (Slack, LINE, Feishu, BlueBubbles, GoogleChat, MSTeams).
//! 2. Sender identification & DM / group policy gating (`evaluate_dm_policy`, `AllowlistMatch`).
//! 3. Built-in slash command dispatch (`dispatch_builtin_command`).
//! 4. Typing indicator lifecycle management (`TypingSignaler`).
//! 5. Agent turn handoff and outbound chunked delivery (`OutboundDeliveryPayload`, `chunk_discord_text`, `build_telegram_thread_params`, `normalize_whatsapp_jid`).

pub mod command_dispatch;
pub mod discord;
pub mod enterprise;
pub mod federated;
pub mod feishu;
pub mod line_adapter;
pub mod outbound_delivery;
pub mod policy_gate;
pub mod slack;
pub mod social;
pub mod telegram;
pub mod types;
pub mod typing_signaler;
pub mod webhook_ingress;
pub mod whatsapp;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

// Re-export core types
pub use types::{
    ChannelAdapter, ChannelDispatchResult, ChannelKind, DeliveryReceipt, DeliveryStatus,
    DispatchAction, InboundAttachmentMeta, InboundChannelMessage, MessageFormat,
    OutboundDeliveryPayload, SharedChannelAdapter,
};

// Re-export required webhook verification symbols
pub use webhook_ingress::{
    hmac_sha256, subtle_slices_equal, to_hex_string, verify_bluebubbles_webhook_auth,
    verify_feishu_webhook_token, verify_googlechat_bearer, verify_line_signature,
    verify_msteams_bearer_jwt_shape, verify_slack_signature,
};

// Re-export required policy gating symbols
pub use policy_gate::{
    evaluate_dm_policy, evaluate_group_policy, normalize_sender_id, AllowlistMatch,
    ChannelDmPolicy, ChannelGroupPolicy, ChannelPolicyGate,
};

// Re-export required command dispatch symbols
pub use command_dispatch::{
    dispatch_builtin_command, ChannelCommandRegistry, CommandContext, CommandExecutionResult,
};

// Re-export required typing indicator symbols
pub use typing_signaler::{TypingCallback, TypingScopeGuard, TypingSignaler};

// Re-export required outbound delivery symbols
pub use outbound_delivery::{
    build_telegram_thread_params, chunk_discord_text, chunk_text_preserving_code_blocks,
    normalize_whatsapp_jid, prepare_outbound_chunks, TelegramSendParams,
    DISCORD_DEFAULT_MAX_CHARS, SLACK_DEFAULT_MAX_CHARS, TELEGRAM_DEFAULT_MAX_CHARS,
    WHATSAPP_DEFAULT_MAX_CHARS,
};

// Re-export adapters
pub use discord::{DiscordAdapter, DiscordAdapterConfig};
pub use enterprise::{
    BlueBubblesAdapter, BlueBubblesAdapterConfig, GoogleChatAdapter, GoogleChatAdapterConfig,
    MSTeamsAdapter, MSTeamsAdapterConfig,
};
pub use federated::{
    IMessageAdapter, MatrixAdapter, MatrixAdapterConfig, MattermostAdapter, MattermostAdapterConfig,
    SignalAdapter, SignalAdapterConfig,
};
pub use feishu::{FeishuAdapter, FeishuAdapterConfig, FeishuClient, FeishuCredentials};
pub use line_adapter::{LineAdapter, LineAdapterConfig};
pub use slack::{SlackAdapter, SlackAdapterConfig};
pub use social::{
    NextcloudAdapter, NextcloudAdapterConfig, NostrAdapter, NostrAdapterConfig, TlonAdapter,
    TlonAdapterConfig, TwitchAdapter, TwitchAdapterConfig, ZaloAdapter, ZaloAdapterConfig,
};
pub use telegram::{TelegramAdapter, TelegramAdapterConfig};
pub use whatsapp::{WhatsAppAdapter, WhatsAppAdapterConfig};

/// Handler callback invoked when an inbound message is cleared for agent turn execution.
pub type AgentTurnHandler = Arc<
    dyn Fn(InboundChannelMessage) -> Result<String, String> + Send + Sync + 'static
>;

/// Unified runtime supervisor coordinating all external messaging channel adapters.
pub struct ChannelRuntime {
    adapters: Arc<RwLock<HashMap<ChannelKind, SharedChannelAdapter>>>,
    policy_gates: Arc<RwLock<HashMap<ChannelKind, ChannelPolicyGate>>>,
    command_registry: Arc<RwLock<ChannelCommandRegistry>>,
    typing_signaler: TypingSignaler,
    seen_messages: Arc<RwLock<HashSet<(String, String)>>>,
    agent_handler: Option<AgentTurnHandler>,
}

impl Default for ChannelRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelRuntime {
    /// Creates a new `ChannelRuntime` with built-in adapters pre-registered.
    pub fn new() -> Self {
        let runtime = Self {
            adapters: Arc::new(RwLock::new(HashMap::new())),
            policy_gates: Arc::new(RwLock::new(HashMap::new())),
            command_registry: Arc::new(RwLock::new(ChannelCommandRegistry::new())),
            typing_signaler: TypingSignaler::default(),
            seen_messages: Arc::new(RwLock::new(HashSet::new())),
            agent_handler: None,
        };

        runtime.register_default_adapters();
        runtime
    }

    /// Registers default instances of all supported external channel adapters.
    fn register_default_adapters(&self) {
        self.register_adapter(Arc::new(DiscordAdapter::new(DiscordAdapterConfig::default())));
        self.register_adapter(Arc::new(SlackAdapter::new(SlackAdapterConfig::default())));
        self.register_adapter(Arc::new(TelegramAdapter::new(TelegramAdapterConfig::default())));
        self.register_adapter(Arc::new(FeishuAdapter::new(FeishuAdapterConfig::default())));
        self.register_adapter(Arc::new(LineAdapter::new(LineAdapterConfig::default())));
        self.register_adapter(Arc::new(WhatsAppAdapter::new(WhatsAppAdapterConfig::default())));
        self.register_adapter(Arc::new(SignalAdapter::new(SignalAdapterConfig::default())));
        self.register_adapter(Arc::new(IMessageAdapter));
        self.register_adapter(Arc::new(MatrixAdapter::new(MatrixAdapterConfig::default())));
        self.register_adapter(Arc::new(MattermostAdapter::new(MattermostAdapterConfig::default())));
        self.register_adapter(Arc::new(NostrAdapter::new(NostrAdapterConfig::default())));
        self.register_adapter(Arc::new(TwitchAdapter::new(TwitchAdapterConfig::default())));
        self.register_adapter(Arc::new(ZaloAdapter::new(ZaloAdapterConfig::default())));
        self.register_adapter(Arc::new(NextcloudAdapter::new(NextcloudAdapterConfig::default())));
        self.register_adapter(Arc::new(TlonAdapter::new(TlonAdapterConfig::default())));
        self.register_adapter(Arc::new(BlueBubblesAdapter::new(BlueBubblesAdapterConfig::default())));
        self.register_adapter(Arc::new(GoogleChatAdapter::new(GoogleChatAdapterConfig::default())));
        self.register_adapter(Arc::new(MSTeamsAdapter::new(MSTeamsAdapterConfig::default())));
    }

    /// Registers or replaces an adapter for a specific channel.
    pub fn register_adapter(&self, adapter: SharedChannelAdapter) {
        let mut lock = self.adapters.write().unwrap();
        lock.insert(adapter.channel_kind(), adapter);
    }

    /// Retrieves an adapter by its channel kind.
    pub fn get_adapter(&self, kind: ChannelKind) -> Option<SharedChannelAdapter> {
        let lock = self.adapters.read().unwrap();
        lock.get(&kind).cloned()
    }

    /// Sets the policy gate for a channel.
    pub fn set_policy_gate(&self, kind: ChannelKind, gate: ChannelPolicyGate) {
        let mut lock = self.policy_gates.write().unwrap();
        lock.insert(kind, gate);
    }

    /// Attaches an agent turn handler callback for messages that pass all filters.
    pub fn with_agent_handler(mut self, handler: AgentTurnHandler) -> Self {
        self.agent_handler = Some(handler);
        self
    }

    /// Access the typing signaler.
    pub fn typing_signaler(&self) -> &TypingSignaler {
        &self.typing_signaler
    }

    /// Core inbound webhook pipeline:
    /// 1. Verifies webhook authenticity.
    /// 2. Parses event into normalized message.
    /// 3. Checks deduplication idempotency.
    /// 4. Checks DM / Group policy gating.
    /// 5. Intercepts built-in slash commands.
    /// 6. Signals typing and forwards to agent turn handler.
    pub fn handle_inbound_webhook(
        &self,
        channel: ChannelKind,
        headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<ChannelDispatchResult, String> {
        let adapter = self.get_adapter(channel)
            .ok_or_else(|| format!("No adapter registered for channel: {channel}"))?;

        // 1. Inbound Webhook Verification
        if !adapter.verify_webhook(headers, body)? {
            return Ok(ChannelDispatchResult {
                accepted: false,
                action_taken: DispatchAction::RejectedAuth,
                reason: Some("Webhook signature or authentication token verification failed".to_string()),
                response_payload: None,
            });
        }

        // 2. Parse Event
        let Some(msg) = adapter.parse_webhook_event(headers, body)? else {
            return Ok(ChannelDispatchResult {
                accepted: true,
                action_taken: DispatchAction::IgnoredEcho,
                reason: Some("Webhook acknowledged but contained no user message".to_string()),
                response_payload: None,
            });
        };

        // 3. Deduplication Check
        let dedup_key = (channel.to_string(), msg.message_id.clone());
        {
            let mut seen = self.seen_messages.write().unwrap();
            if !seen.insert(dedup_key) {
                return Ok(ChannelDispatchResult {
                    accepted: true,
                    action_taken: DispatchAction::SuppressedDuplicate,
                    reason: Some(format!("Message {} already processed", msg.message_id)),
                    response_payload: None,
                });
            }
        }

        // 4. Policy Gate Check
        {
            let gates = self.policy_gates.read().unwrap();
            if let Some(gate) = gates.get(&channel) {
                let match_result = gate.check_admission(&msg.sender_id, msg.is_dm, false);
                if !match_result.is_allowed() {
                    return Ok(ChannelDispatchResult {
                        accepted: false,
                        action_taken: DispatchAction::RejectedPolicy,
                        reason: Some(format!("Sender {} rejected by {:?} policy", msg.sender_id, match_result)),
                        response_payload: None,
                    });
                }
            }
        }

        // 5. Built-in Command Interception
        if msg.text.trim().starts_with('/') {
            let cmd_context = CommandContext::new(
                channel.to_string(),
                &msg.sender_id,
                &msg.conversation_id,
                false,
            );

            let registry = self.command_registry.read().unwrap();
            if let Some(res) = registry.execute(&msg.text, &cmd_context) {
                if let Some(reply_text) = res.immediate_reply {
                    let outbound = OutboundDeliveryPayload::new(channel, &msg.conversation_id, reply_text)
                        .with_reply_to(&msg.message_id);
                    let _ = adapter.deliver(&outbound);
                }

                return Ok(ChannelDispatchResult {
                    accepted: true,
                    action_taken: DispatchAction::HandledByBuiltinCommand,
                    reason: res.system_notice,
                    response_payload: None,
                });
            }
        }

        // 6. Typing Signaler & Agent Dispatch
        let session_key = format!("{}:{}", channel, msg.conversation_id);
        let adapter_clone = adapter.clone();
        let conv_id = msg.conversation_id.clone();
        let target_for_typing = conv_id.clone();
        let thread_id_clone = msg.thread_id.clone();

        let typing_callback: TypingCallback = Arc::new(move |_target, _thread| {
            adapter_clone.send_typing_indicator(&target_for_typing, thread_id_clone.as_deref())
        });

        self.typing_signaler.start(
            &session_key,
            &msg.conversation_id,
            msg.thread_id.as_deref(),
            typing_callback,
        );

        // Handoff to agent if handler attached
        if let Some(ref handler) = self.agent_handler {
            match handler(msg) {
                Ok(reply_text) => {
                    self.typing_signaler.stop(&session_key);
                    let outbound = OutboundDeliveryPayload::new(channel, conv_id, reply_text);
                    let _ = adapter.deliver(&outbound);
                }
                Err(err) => {
                    self.typing_signaler.stop(&session_key);
                    return Err(format!("Agent turn execution failure: {err}"));
                }
            }
        }

        Ok(ChannelDispatchResult {
            accepted: true,
            action_taken: DispatchAction::ForwardedToAgent,
            reason: None,
            response_payload: None,
        })
    }

    /// Dispatches an outbound payload using chunking and the appropriate channel adapter.
    pub fn dispatch_outbound(
        &self,
        payload: &OutboundDeliveryPayload,
    ) -> Result<Vec<DeliveryReceipt>, String> {
        let adapter = self.get_adapter(payload.channel)
            .ok_or_else(|| format!("No adapter registered for channel: {}", payload.channel))?;

        let chunks = prepare_outbound_chunks(payload);
        let mut receipts = Vec::with_capacity(chunks.len());

        for chunk in chunks {
            let receipt = adapter.deliver(&chunk)?;
            receipts.push(receipt);
        }

        Ok(receipts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_runtime_default_adapters() {
        let runtime = ChannelRuntime::new();
        assert!(runtime.get_adapter(ChannelKind::Discord).is_some());
        assert!(runtime.get_adapter(ChannelKind::Slack).is_some());
        assert!(runtime.get_adapter(ChannelKind::Telegram).is_some());
        assert!(runtime.get_adapter(ChannelKind::Feishu).is_some());
        assert!(runtime.get_adapter(ChannelKind::Line).is_some());
        assert!(runtime.get_adapter(ChannelKind::WhatsApp).is_some());
        assert!(runtime.get_adapter(ChannelKind::Signal).is_some());
        assert!(runtime.get_adapter(ChannelKind::IMessage).is_some());
        assert!(runtime.get_adapter(ChannelKind::Matrix).is_some());
        assert!(runtime.get_adapter(ChannelKind::Mattermost).is_some());
        assert!(runtime.get_adapter(ChannelKind::Nostr).is_some());
        assert!(runtime.get_adapter(ChannelKind::Twitch).is_some());
        assert!(runtime.get_adapter(ChannelKind::Zalo).is_some());
        assert!(runtime.get_adapter(ChannelKind::Nextcloud).is_some());
        assert!(runtime.get_adapter(ChannelKind::Tlon).is_some());
        assert!(runtime.get_adapter(ChannelKind::BlueBubbles).is_some());
        assert!(runtime.get_adapter(ChannelKind::GoogleChat).is_some());
        assert!(runtime.get_adapter(ChannelKind::MSTeams).is_some());
    }

    #[test]
    fn test_channel_runtime_command_dispatch() {
        let runtime = ChannelRuntime::new();
        let body = serde_json::json!({
            "id": "msg_cmd_1",
            "channel_id": "discord_room",
            "content": "/ping",
            "author": {
                "id": "user_alice",
                "username": "alice",
                "bot": false
            }
        });

        let body_bytes = serde_json::to_vec(&body).unwrap();
        let res = runtime
            .handle_inbound_webhook(ChannelKind::Discord, &HashMap::new(), &body_bytes)
            .expect("dispatch");

        assert!(res.accepted);
        assert_eq!(res.action_taken, DispatchAction::HandledByBuiltinCommand);
    }

    #[test]
    fn test_channel_runtime_outbound_dispatch() {
        let runtime = ChannelRuntime::new();
        let payload = OutboundDeliveryPayload::new(ChannelKind::Telegram, "12345678", "Outbound test message");
        let receipts = runtime.dispatch_outbound(&payload).expect("outbound");
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].status, DeliveryStatus::Delivered);
    }
}
