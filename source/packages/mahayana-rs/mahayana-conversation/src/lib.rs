//! Conversation provider routing shared by CLI, native app, Electron, and Web surfaces.

use async_trait::async_trait;
use mahayana_core::ApprovalDecision;
use mahayana_core::ApprovalId;
use mahayana_core::Conversation;
use mahayana_core::ConversationId;
use mahayana_core::Message;
use mahayana_core::OperationId;
use mahayana_core::PluginCommandDescriptor;
use mahayana_core::RuntimeEvent;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

pub const MAHAYANA_AI_PROVIDER_KEY: &str = "mahayana-ai";
pub const MAHAYANA_AI_CONVERSATION_PREFIX: &str = "mahayana-ai:";

#[derive(Debug, Clone)]
pub struct SendMessageRequest {
    pub conversation_id: ConversationId,
    pub operation_id: OperationId,
    pub text: String,
    pub display_text: Option<String>,
    pub client_message_id: Option<String>,
    pub hidden: bool,
    pub show_assistant_output: bool,
    pub recovery_eligible: bool,
    pub reply_to_message_id: Option<String>,
    pub is_fork: bool,
    pub attachment_batch_id: Option<String>,
    pub selected_image_data_urls: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SuspendConversationOperationRequest {
    pub operation_id: OperationId,
    pub reason: Option<String>,
    pub cascade: bool,
}

#[derive(Debug, Clone)]
pub struct ResumeConversationOperationRequest {
    pub conversation_id: ConversationId,
    pub operation_id: OperationId,
    pub hidden: bool,
    pub show_assistant_output: bool,
    pub reply_to_message_id: Option<String>,
    pub is_fork: bool,
    pub attachment_batch_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolveApprovalRequest {
    pub approval_id: ApprovalId,
    pub decision: ApprovalDecision,
    pub payload: Value,
}

/// Event sink used by provider implementations. Implementations must preserve
/// ordering for events belonging to the same operation.
pub trait ConversationEventSink: Send + Sync {
    fn emit(&self, event: RuntimeEvent) -> Result<(), ConversationError>;
}

pub type SharedConversationEventSink = Arc<dyn ConversationEventSink>;

pub fn select_history_window(
    messages: &[Message],
    before_message_id: Option<&str>,
    after_message_id: Option<&str>,
    limit: Option<u32>,
) -> Result<Vec<Message>, ConversationError> {
    if before_message_id.is_some() && after_message_id.is_some() {
        return Err(ConversationError::Provider(
            "history window cannot specify both beforeMessageId and afterMessageId".into(),
        ));
    }

    let (start, end) = if let Some(after_message_id) = after_message_id {
        let Some(index) = messages
            .iter()
            .position(|message| message.id.as_str() == after_message_id)
        else {
            return Ok(Vec::new());
        };
        (index + 1, messages.len())
    } else if let Some(before_message_id) = before_message_id {
        let Some(index) = messages
            .iter()
            .position(|message| message.id.as_str() == before_message_id)
        else {
            return Ok(Vec::new());
        };
        (0, index)
    } else {
        (0, messages.len())
    };

    let mut selected = messages[start..end].to_vec();
    if let Some(limit) = limit {
        let limit = limit.max(1) as usize;
        if after_message_id.is_some() {
            selected.truncate(limit);
        } else if selected.len() > limit {
            selected = selected[selected.len() - limit..].to_vec();
        }
    }
    Ok(selected)
}

/// One source of conversations. Implementations own provider-specific network,
/// persistence, and approval behavior while exposing one product contract.
#[async_trait]
pub trait ConversationProvider: Send + Sync {
    fn key(&self) -> &'static str;

    async fn list_conversations(&self) -> Result<Vec<Conversation>, ConversationError>;

    async fn list_plugin_commands(
        &self,
        _plugin_id: Option<&str>,
    ) -> Result<Vec<PluginCommandDescriptor>, ConversationError> {
        Ok(Vec::new())
    }

    async fn history(
        &self,
        conversation_id: &ConversationId,
        limit: u32,
    ) -> Result<Vec<Message>, ConversationError>;

    async fn history_window(
        &self,
        conversation_id: &ConversationId,
        before_message_id: Option<&str>,
        after_message_id: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<Message>, ConversationError> {
        let scan_limit = limit.unwrap_or(500).max(500).min(10_000);
        let messages = self.history(conversation_id, scan_limit).await?;
        select_history_window(
            &messages,
            before_message_id,
            after_message_id,
            limit,
        )
    }

    /// Replaces one existing canonical message while preserving provider-owned
    /// persistence. Providers that do not support durable local mutation fail
    /// closed instead of accepting a parallel Host/UI transcript owner.
    async fn replace_message(
        &self,
        _conversation_id: &ConversationId,
        _message: Message,
    ) -> Result<bool, ConversationError> {
        Err(ConversationError::Provider(
            "conversation provider does not support canonical message replacement".into(),
        ))
    }

    /// Prepare provider-owned resources needed by the first user-visible turn
    /// without sending model input or mutating the conversation transcript.
    ///
    /// Providers that have no cold session/setup cost may keep the default
    /// no-op. Native agent providers should make this idempotent so product
    /// startup can establish real readiness before the composer is usable.
    async fn warmup(&self, _conversation_id: &ConversationId) -> Result<(), ConversationError> {
        Ok(())
    }

    async fn send_message(
        &self,
        request: SendMessageRequest,
        events: SharedConversationEventSink,
    ) -> Result<(), ConversationError>;

    async fn interrupt(&self, operation_id: &OperationId) -> Result<(), ConversationError>;

    /// Cooperatively park one already-running operation without converting it
    /// into a terminal interruption. Providers that cannot preserve an
    /// unfinished operation fail closed.
    async fn suspend_operation(
        &self,
        _request: SuspendConversationOperationRequest,
    ) -> Result<(), ConversationError> {
        Err(ConversationError::Provider(
            "conversation provider does not support operation suspension".into(),
        ))
    }

    /// Resume a previously suspended operation without enqueueing the original
    /// user input a second time.
    async fn resume_operation(
        &self,
        _request: ResumeConversationOperationRequest,
        _events: SharedConversationEventSink,
    ) -> Result<(), ConversationError> {
        Err(ConversationError::Provider(
            "conversation provider does not support operation resume".into(),
        ))
    }

    async fn resolve_approval(
        &self,
        request: ResolveApprovalRequest,
    ) -> Result<(), ConversationError>;

    /// Drops local transcript/session state when the authenticated product
    /// account changes. Remote providers may keep the default no-op.
    async fn reset_session(&self) -> Result<(), ConversationError> {
        Ok(())
    }

    /// Switch the local transcript file used by a long-lived provider. The
    /// default is a no-op for providers whose history is remote or owned by a
    /// separate account boundary.
    async fn set_history_path(&self, _path: Option<PathBuf>) -> Result<(), ConversationError> {
        Ok(())
    }
}

#[derive(Default)]
pub struct ProviderRegistry {
    providers: BTreeMap<String, Arc<dyn ConversationProvider>>,
}

impl ProviderRegistry {
    pub fn register(
        &mut self,
        provider: Arc<dyn ConversationProvider>,
    ) -> Result<(), ConversationError> {
        let key = provider.key().trim();
        if key.is_empty() {
            return Err(ConversationError::InvalidProviderKey);
        }
        if self.providers.insert(key.to_string(), provider).is_some() {
            return Err(ConversationError::DuplicateProvider(key.to_string()));
        }
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<Arc<dyn ConversationProvider>> {
        self.providers.get(key).cloned()
    }

    pub fn for_conversation(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<Arc<dyn ConversationProvider>, ConversationError> {
        let key = provider_key_for_conversation_id(conversation_id)?;
        self.get(key)
            .ok_or_else(|| ConversationError::ProviderUnavailable(key.to_string()))
    }

    pub fn keys(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    pub fn providers(&self) -> Vec<Arc<dyn ConversationProvider>> {
        self.providers.values().cloned().collect()
    }
}

pub fn provider_key_for_conversation_id(
    conversation_id: &ConversationId,
) -> Result<&'static str, ConversationError> {
    let value = conversation_id.as_str();
    if value.starts_with(MAHAYANA_AI_CONVERSATION_PREFIX) || value.starts_with("codex:") {
        // `codex:` remains a read-compatible migration prefix only. New
        // Mahayana surfaces emit `mahayana-ai:` identifiers and both route to
        // the sovereign AI provider boundary.
        Ok(MAHAYANA_AI_PROVIDER_KEY)
    } else if value.starts_with("telegram:") {
        Ok("telegram")
    } else if value.starts_with("mahayana:") {
        Ok("mahayana-social")
    } else if value.starts_with("miniapp:") {
        Ok("miniapp")
    } else {
        Err(ConversationError::UnsupportedConversation(
            value.to_string(),
        ))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConversationError {
    #[error("provider key must not be empty")]
    InvalidProviderKey,
    #[error("provider is already registered: {0}")]
    DuplicateProvider(String),
    #[error("provider is unavailable: {0}")]
    ProviderUnavailable(String),
    #[error("unsupported conversation id: {0}")]
    UnsupportedConversation(String),
    #[error("conversation was not found: {0}")]
    ConversationNotFound(ConversationId),
    #[error("operation was not found: {0}")]
    OperationNotFound(OperationId),
    #[error("operation interrupted: {0}")]
    Interrupted(String),
    #[error("operation suspended")]
    Suspended,
    #[error("approval was not found: {0}")]
    ApprovalNotFound(ApprovalId),
    #[error("model usage limit exceeded: {0}")]
    UsageLimitExceeded(String),
    #[error("provider failed: {0}")]
    Provider(String),
    #[error("event consumer is closed")]
    EventConsumerClosed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_window_preserves_before_after_boundaries_and_missing_anchor_fails_closed() {
        let conversation_id = ConversationId("mahayana-ai:agent:assistant".into());
        let messages = ["a", "b", "c", "d"]
            .into_iter()
            .map(|id| Message {
                id: mahayana_core::MessageId(id.into()),
                conversation_id: conversation_id.clone(),
                role: mahayana_core::MessageRole::Assistant,
                text: id.into(),
                created_at_ms: 0,
                metadata: Value::Null,
            })
            .collect::<Vec<_>>();

        let before = select_history_window(&messages, Some("d"), None, Some(2))
            .expect("before window");
        assert_eq!(
            before.into_iter().map(|message| message.id.0).collect::<Vec<_>>(),
            vec!["b", "c"]
        );

        let after = select_history_window(&messages, None, Some("b"), None)
            .expect("after window");
        assert_eq!(
            after.into_iter().map(|message| message.id.0).collect::<Vec<_>>(),
            vec!["c", "d"]
        );

        assert!(select_history_window(&messages, Some("missing"), None, Some(2))
            .expect("missing anchor")
            .is_empty());
        assert!(select_history_window(&messages, Some("b"), Some("c"), Some(2)).is_err());
    }

    #[test]
    fn routes_all_supported_peer_prefixes() {
        let cases = [
            ("mahayana-ai:agent:assistant", MAHAYANA_AI_PROVIDER_KEY),
            ("codex:agent:assistant", MAHAYANA_AI_PROVIDER_KEY),
            ("telegram:user:42", "telegram"),
            ("mahayana:contact:abc", "mahayana-social"),
            ("miniapp:official.flashcards", "miniapp"),
        ];
        for (id, expected) in cases {
            let actual = provider_key_for_conversation_id(&ConversationId(id.to_string()))
                .expect("known conversation prefix");
            assert_eq!(actual, expected);
        }
    }
}
