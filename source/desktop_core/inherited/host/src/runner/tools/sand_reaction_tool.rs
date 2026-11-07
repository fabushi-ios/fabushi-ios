use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use serde_json::{Value, json};

use crate::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use crate::runner::routed_provider_runtime::RoutedToolBridge;

pub const SAND_REACT_TO_MESSAGE_TOOL_NAME: &str = "ReactToMessage";

#[derive(Clone, Default)]
pub struct ReactionDeliveryCounter {
    reacted_count: Arc<AtomicU64>,
}

impl ReactionDeliveryCounter {
    pub fn count(&self) -> u64 {
        self.reacted_count.load(Ordering::SeqCst)
    }

    fn record_success(&self) {
        self.reacted_count.fetch_add(1, Ordering::SeqCst);
    }
}

pub struct CountingReactionSink {
    delegate: Arc<dyn ReactionSink>,
    counter: ReactionDeliveryCounter,
}

impl CountingReactionSink {
    pub fn new(delegate: Arc<dyn ReactionSink>, counter: ReactionDeliveryCounter) -> Self {
        Self { delegate, counter }
    }
}

pub trait ReactionSink: Send + Sync {
    fn react(
        &self,
        message_address: &str,
        emoji: &str,
    ) -> Result<(), ProviderSessionError>;
}

impl ReactionSink for CountingReactionSink {
    fn react(
        &self,
        message_address: &str,
        emoji: &str,
    ) -> Result<(), ProviderSessionError> {
        self.delegate.react(message_address, emoji)?;
        self.counter.record_success();
        Ok(())
    }
}

pub struct ReactionToolBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    sink: Arc<dyn ReactionSink>,
}

impl ReactionToolBridge {
    pub fn new(delegate: Arc<dyn RoutedToolBridge>, sink: Arc<dyn ReactionSink>) -> Self {
        Self { delegate, sink }
    }
}

pub fn reaction_tool_definition() -> RoutedToolDefinition {
    RoutedToolDefinition {
        name: SAND_REACT_TO_MESSAGE_TOOL_NAME.to_string(),
        provider_identifier: "fabushi-runner".to_string(),
        tool_name: SAND_REACT_TO_MESSAGE_TOOL_NAME.to_string(),
        description: Some(
            "React to one of the USER's messages with a single emoji tapback. Use the t3u-style message address shown on the user's message; reacting with the same emoji again toggles it off."
                .to_string(),
        ),
        input_schema: json!({
            "type": "object",
            "required": ["message_address", "emoji"],
            "additionalProperties": false,
            "properties": {
                "message_address": {
                    "type": "string",
                    "minLength": 1,
                    "description": "The t3u-style address of the user's message."
                },
                "emoji": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 16,
                    "description": "A single common emoji reaction."
                }
            }
        }),
    }
}

pub fn is_message_address(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.first().copied() != Some(b't') || bytes.len() < 3 {
        return false;
    }

    fn consume_digits(bytes: &[u8], index: &mut usize) -> bool {
        let start = *index;
        while *index < bytes.len() && bytes[*index].is_ascii_digit() {
            *index += 1;
        }
        *index > start
    }

    let mut index = 1usize;
    if bytes[index] == b'b' {
        index += 1;
        if index >= bytes.len() || !matches!(bytes[index], b'a' | b's') {
            return false;
        }
        index += 1;
        return consume_digits(bytes, &mut index) && index == bytes.len();
    }

    if !consume_digits(bytes, &mut index) || index >= bytes.len() {
        return false;
    }
    match bytes[index] {
        b'u' => {
            index += 1;
            if index == bytes.len() {
                return true;
            }
            if bytes[index] != b'a' {
                return false;
            }
            index += 1;
            consume_digits(bytes, &mut index) && index == bytes.len()
        }
        b'a' | b's' => {
            index += 1;
            consume_digits(bytes, &mut index) && index == bytes.len()
        }
        _ => false,
    }
}

impl RoutedToolBridge for ReactionToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        let mut tools = self.delegate.list_tools()?;
        tools.retain(|tool| {
            tool.name != SAND_REACT_TO_MESSAGE_TOOL_NAME
                && tool.tool_name != SAND_REACT_TO_MESSAGE_TOOL_NAME
        });
        tools.insert(0, reaction_tool_definition());
        Ok(tools)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        if tool.name != SAND_REACT_TO_MESSAGE_TOOL_NAME
            && tool.tool_name != SAND_REACT_TO_MESSAGE_TOOL_NAME
        {
            return self.delegate.call_tool(tool, args, tool_call_id);
        }

        let object = args.as_object().ok_or_else(|| {
            ProviderSessionError::Tool("ReactToMessage arguments must be an object".into())
        })?;
        let address = object
            .get("message_address")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ProviderSessionError::Tool(
                    "message_address is required for ReactToMessage".into(),
                )
            })?;
        let emoji = object
            .get("emoji")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ProviderSessionError::Tool("emoji is required for ReactToMessage".into())
            })?;
        if emoji.encode_utf16().count() > 16 {
            return Err(ProviderSessionError::Tool(
                "emoji must be at most 16 UTF-16 code units".into(),
            ));
        }
        if !is_message_address(address) {
            return Ok(Value::String(format!(
                "\"{address}\" isn't a valid message address. React with the [t3u]-style tag shown on the user's message."
            )));
        }

        self.sink.react(address, emoji)?;
        Ok(Value::String(format!(
            "Reacted {emoji} on {address}. (Reactions toggle: react the same emoji again to take it back.)"
        )))
    }
}
