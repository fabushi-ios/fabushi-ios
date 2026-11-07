use serde_json::Value;

use super::send_message_reminder_middleware::{
    CursorProviderOptions, MessageContent, MessageLike, ProviderOptions, PromptExecutor,
    SAND_SEND_MESSAGE_TOOL_NAME, count_tool_calls_since_last_send_message,
    is_injected_reminder_message,
};

pub const DEFAULT_START_OF_TURN_ACK_THRESHOLD: usize = 1;
pub const START_OF_TURN_ACK_REMINDER_MESSAGE: &str = "<system_reminder>\nYou opened this turn by calling tools without first acknowledging the user, so they are watching silence and may think the app froze. Acknowledge them RIGHT NOW by actually invoking the SendMessage tool — make a real tool/function call, not text you write. Plain assistant text is NEVER shown to the user; only a real SendMessage tool invocation reaches them, so if you don't call the tool they just keep seeing silence. Make that first SendMessage a one-line text acknowledgement, before any further tool call, then continue the work. A widget, attachment, or cursor-agent card does not count as this acknowledgement.\n</system_reminder>";

pub fn build_reminder_message(content: impl Into<String>) -> MessageLike {
    MessageLike {
        role: "user".into(),
        content: MessageContent::Text(content.into()),
        provider_options: Some(ProviderOptions {
            cursor: Some(CursorProviderOptions {
                sand_start_of_turn_ack_reminder: true,
                ..CursorProviderOptions::default()
            }),
        }),
    }
}

pub fn is_start_of_turn_ack_reminder_message(message: &MessageLike) -> bool {
    message
        .provider_options
        .as_ref()
        .and_then(|options| options.cursor.as_ref())
        .is_some_and(|cursor| cursor.sand_start_of_turn_ack_reminder)
}

pub fn is_text_send_message_args(args: Option<&Value>) -> bool {
    args.and_then(Value::as_object)
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)
        == Some("text")
}

pub fn has_text_send_message_call(message: &MessageLike) -> bool {
    if message.role != "assistant" {
        return false;
    }
    let MessageContent::Parts(parts) = &message.content else {
        return false;
    };
    parts.iter().any(|part| {
        part.r#type.as_deref() == Some("tool-call")
            && part.tool_name.as_deref() == Some(SAND_SEND_MESSAGE_TOOL_NAME)
            && is_text_send_message_args(part.args.as_ref())
    })
}

pub fn has_text_send_message_since_turn_start(messages: &[MessageLike]) -> bool {
    for message in messages.iter().rev() {
        if is_injected_reminder_message(message) {
            continue;
        }
        if matches!(message.role.as_str(), "user" | "system") {
            return false;
        }
        if has_text_send_message_call(message) {
            return true;
        }
    }
    false
}

pub struct StartOfTurnAckReminderMiddleware<E> {
    pub inner_executor: E,
    pub threshold: usize,
    pub message: MessageLike,
}

impl<E: PromptExecutor> PromptExecutor for StartOfTurnAckReminderMiddleware<E> {
    type StreamArgs = E::StreamArgs;
    type StreamOutput = E::StreamOutput;

    fn get_messages(&self) -> &[MessageLike] {
        self.inner_executor.get_messages()
    }

    fn get_state(&self) -> &[MessageLike] {
        self.inner_executor.get_state()
    }

    fn clear_messages(&mut self) {
        self.inner_executor.clear_messages();
    }

    fn append_messages(&mut self, messages: Vec<MessageLike>) {
        self.inner_executor.append_messages(messages);
    }

    fn stream(&mut self, args: Self::StreamArgs) -> Self::StreamOutput {
        let messages = self.inner_executor.get_messages();
        let last_is_ack_reminder = messages
            .last()
            .is_some_and(is_start_of_turn_ack_reminder_message);
        if !last_is_ack_reminder
            && !has_text_send_message_since_turn_start(messages)
            && count_tool_calls_since_last_send_message(messages) > self.threshold
        {
            self.inner_executor
                .append_messages(vec![self.message.clone()]);
        }
        self.inner_executor.stream(args)
    }
}

pub fn create_start_of_turn_ack_reminder_middleware<E: PromptExecutor>(
    executor: E,
    threshold: Option<usize>,
    message: Option<&str>,
) -> StartOfTurnAckReminderMiddleware<E> {
    StartOfTurnAckReminderMiddleware {
        inner_executor: executor,
        threshold: threshold.unwrap_or(DEFAULT_START_OF_TURN_ACK_THRESHOLD),
        message: build_reminder_message(message.unwrap_or(START_OF_TURN_ACK_REMINDER_MESSAGE)),
    }
}
