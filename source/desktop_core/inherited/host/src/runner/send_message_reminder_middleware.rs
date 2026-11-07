use serde_json::Value;

pub const SAND_SEND_MESSAGE_TOOL_NAME: &str = "SendMessage";
pub const DEFAULT_SEND_MESSAGE_REMINDER_THRESHOLD: usize = 6;
pub const DEFAULT_EARLY_RESULT_REMINDER_THRESHOLD: usize = 0;
pub const SEND_MESSAGE_REMINDER_MESSAGE: &str = "<system_reminder>\nYou have made several tool calls without a SendMessage, so the user is currently watching silence. Actually invoke the SendMessage tool now. Send a brief, specific update on what you are doing or what you just found before continuing.\n</system_reminder>";
pub const EARLY_RESULT_REMINDER_MESSAGE: &str = "<system_reminder>\nRemember: the user cannot see tool output or your thinking — only SendMessage reaches them. If you have produced a result or finished what they asked, send it now with SendMessage tool call before continuing or ending the turn. If you are still mid-task, keep working and send the result once you have it.\n</system_reminder>";
pub const DISK_PRESSURE_REMINDER_MESSAGE: &str = "<system_reminder>\nThe box is near disk capacity. Avoid disk-heavy work and do not fill the remaining capacity.\n</system_reminder>";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MessagePart {
    pub r#type: Option<String>,
    pub text: Option<String>,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<String>,
    pub args: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<MessagePart>),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CursorProviderOptions {
    pub sand_send_message_reminder: bool,
    pub sand_early_result_reminder: bool,
    pub sand_start_of_turn_ack_reminder: bool,
    pub sand_disk_pressure_reminder: bool,
    pub sand_disk_pressure_reminder_episode_id: Option<String>,
    pub high_level_tool_call_result: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderOptions {
    pub cursor: Option<CursorProviderOptions>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageLike {
    pub role: String,
    pub content: MessageContent,
    pub provider_options: Option<ProviderOptions>,
}

impl MessageLike {
    pub fn text(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: MessageContent::Text(content.into()),
            provider_options: None,
        }
    }

    pub fn parts(role: impl Into<String>, parts: Vec<MessagePart>) -> Self {
        Self {
            role: role.into(),
            content: MessageContent::Parts(parts),
            provider_options: None,
        }
    }

    fn cursor(&self) -> Option<&CursorProviderOptions> {
        self.provider_options.as_ref()?.cursor.as_ref()
    }
}

pub fn get_user_message_text(message: &MessageLike) -> Option<String> {
    if message.role != "user" {
        return None;
    }
    Some(match &message.content {
        MessageContent::Text(text) => text.clone(),
        MessageContent::Parts(parts) => parts
            .iter()
            .filter(|part| part.r#type.as_deref() == Some("text"))
            .filter_map(|part| part.text.as_deref())
            .collect::<String>(),
    })
}

pub fn is_send_message_reminder_message(message: &MessageLike) -> bool {
    get_user_message_text(message)
        .is_some_and(|text| text.contains(SEND_MESSAGE_REMINDER_MESSAGE))
}

pub fn is_injected_reminder_message(message: &MessageLike) -> bool {
    let cursor = message.cursor();
    cursor.is_some_and(|cursor| {
        cursor.sand_send_message_reminder
            || cursor.sand_early_result_reminder
            || cursor.sand_start_of_turn_ack_reminder
            || cursor.sand_disk_pressure_reminder
    }) || is_send_message_reminder_message(message)
        || get_user_message_text(message)
            .is_some_and(|text| text.contains(EARLY_RESULT_REMINDER_MESSAGE))
}

pub fn has_send_message_call(message: &MessageLike) -> bool {
    message.role == "assistant"
        && matches!(
            &message.content,
            MessageContent::Parts(parts)
                if parts.iter().any(|part| {
                    part.r#type.as_deref() == Some("tool-call")
                        && part.tool_name.as_deref() == Some(SAND_SEND_MESSAGE_TOOL_NAME)
                })
        )
}

pub fn count_non_send_message_tool_calls(message: &MessageLike) -> usize {
    if message.role != "assistant" {
        return 0;
    }
    match &message.content {
        MessageContent::Text(_) => 0,
        MessageContent::Parts(parts) => parts
            .iter()
            .filter(|part| {
                part.r#type.as_deref() == Some("tool-call")
                    && part.tool_name.as_deref() != Some(SAND_SEND_MESSAGE_TOOL_NAME)
            })
            .count(),
    }
}

pub fn count_tool_calls_since_last_send_message(messages: &[MessageLike]) -> usize {
    let mut count = 0;
    for message in messages.iter().rev() {
        if matches!(message.role.as_str(), "user" | "system") || has_send_message_call(message) {
            break;
        }
        count += count_non_send_message_tool_calls(message);
    }
    count
}

pub fn has_send_message_since_real_turn_start(messages: &[MessageLike]) -> bool {
    for message in messages.iter().rev() {
        if is_injected_reminder_message(message) {
            continue;
        }
        if matches!(message.role.as_str(), "user" | "system") {
            return false;
        }
        if has_send_message_call(message) {
            return true;
        }
    }
    false
}

pub fn has_reminder_fired_this_silent_streak(messages: &[MessageLike]) -> bool {
    for message in messages.iter().rev() {
        if is_injected_reminder_message(message) {
            return true;
        }
        if matches!(message.role.as_str(), "user" | "system") || has_send_message_call(message) {
            return false;
        }
    }
    false
}

pub fn create_send_message_reminder_message() -> MessageLike {
    MessageLike {
        role: "user".into(),
        content: MessageContent::Text(SEND_MESSAGE_REMINDER_MESSAGE.into()),
        provider_options: Some(ProviderOptions {
            cursor: Some(CursorProviderOptions {
                sand_send_message_reminder: true,
                ..CursorProviderOptions::default()
            }),
        }),
    }
}

pub fn create_early_result_reminder_message() -> MessageLike {
    MessageLike {
        role: "user".into(),
        content: MessageContent::Text(EARLY_RESULT_REMINDER_MESSAGE.into()),
        provider_options: Some(ProviderOptions {
            cursor: Some(CursorProviderOptions {
                sand_early_result_reminder: true,
                ..CursorProviderOptions::default()
            }),
        }),
    }
}

pub trait PromptExecutor {
    type StreamArgs;
    type StreamOutput;

    fn get_messages(&self) -> &[MessageLike];
    fn get_state(&self) -> &[MessageLike];
    fn clear_messages(&mut self);
    fn append_messages(&mut self, messages: Vec<MessageLike>);
    fn stream(&mut self, args: Self::StreamArgs) -> Self::StreamOutput;
}

pub struct DiskPressureReminderMiddleware<E> {
    pub inner_executor: E,
    pub episode_id: String,
}

impl<E: PromptExecutor> PromptExecutor for DiskPressureReminderMiddleware<E> {
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
        let already_injected = self.inner_executor.get_messages().iter().any(|message| {
            message
                .cursor()
                .and_then(|cursor| cursor.sand_disk_pressure_reminder_episode_id.as_deref())
                == Some(self.episode_id.as_str())
        });
        if !already_injected {
            self.inner_executor.append_messages(vec![MessageLike {
                role: "user".into(),
                content: MessageContent::Text(DISK_PRESSURE_REMINDER_MESSAGE.into()),
                provider_options: Some(ProviderOptions {
                    cursor: Some(CursorProviderOptions {
                        sand_disk_pressure_reminder: true,
                        sand_disk_pressure_reminder_episode_id: Some(self.episode_id.clone()),
                        ..CursorProviderOptions::default()
                    }),
                }),
            }]);
        }
        self.inner_executor.stream(args)
    }
}

pub fn create_disk_pressure_reminder_middleware<E: PromptExecutor>(
    executor: E,
    episode_id: impl Into<String>,
) -> DiskPressureReminderMiddleware<E> {
    DiskPressureReminderMiddleware {
        inner_executor: executor,
        episode_id: episode_id.into(),
    }
}

pub struct SendMessageReminderMiddleware<E> {
    pub inner_executor: E,
    pub threshold: usize,
    pub early_result_threshold: usize,
}

impl<E: PromptExecutor> PromptExecutor for SendMessageReminderMiddleware<E> {
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
        let last_is_main_reminder = messages
            .last()
            .is_some_and(is_send_message_reminder_message);
        if !last_is_main_reminder {
            let count = count_tool_calls_since_last_send_message(messages);
            let reminder = if count > self.threshold {
                Some(create_send_message_reminder_message())
            } else if count > self.early_result_threshold
                && has_send_message_since_real_turn_start(messages)
                && !has_reminder_fired_this_silent_streak(messages)
            {
                Some(create_early_result_reminder_message())
            } else {
                None
            };
            if let Some(reminder) = reminder {
                self.inner_executor.append_messages(vec![reminder]);
            }
        }
        self.inner_executor.stream(args)
    }
}

pub fn create_send_message_reminder_middleware<E: PromptExecutor>(
    executor: E,
    threshold: Option<usize>,
    early_result_threshold: Option<usize>,
) -> SendMessageReminderMiddleware<E> {
    SendMessageReminderMiddleware {
        inner_executor: executor,
        threshold: threshold.unwrap_or(DEFAULT_SEND_MESSAGE_REMINDER_THRESHOLD),
        early_result_threshold: early_result_threshold
            .unwrap_or(DEFAULT_EARLY_RESULT_REMINDER_THRESHOLD),
    }
}
