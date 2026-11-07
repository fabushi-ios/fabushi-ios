//! Built-in slash commands dispatcher for external channel interactions (OC-S3).

use std::collections::HashMap;
use std::sync::Arc;
use serde::{Deserialize, Serialize};

/// Execution context for a parsed built-in command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandContext {
    pub channel_id: String,
    pub sender_id: String,
    pub conversation_id: String,
    pub is_admin: bool,
}

impl CommandContext {
    pub fn new(
        channel_id: impl Into<String>,
        sender_id: impl Into<String>,
        conversation_id: impl Into<String>,
        is_admin: bool,
    ) -> Self {
        Self {
            channel_id: channel_id.into(),
            sender_id: sender_id.into(),
            conversation_id: conversation_id.into(),
            is_admin,
        }
    }
}

/// Output outcome from executing a built-in channel command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandExecutionResult {
    pub handled: bool,
    pub immediate_reply: Option<String>,
    pub should_halt: bool,
    pub system_notice: Option<String>,
}

impl CommandExecutionResult {
    pub fn reply(text: impl Into<String>) -> Self {
        Self {
            handled: true,
            immediate_reply: Some(text.into()),
            should_halt: true,
            system_notice: None,
        }
    }

    pub fn halt(notice: Option<String>) -> Self {
        Self {
            handled: true,
            immediate_reply: None,
            should_halt: true,
            system_notice: notice,
        }
    }

    pub fn pass_through() -> Self {
        Self {
            handled: false,
            immediate_reply: None,
            should_halt: false,
            system_notice: None,
        }
    }
}

/// Dispatches standard built-in slash commands.
///
/// Returns `Some(CommandExecutionResult)` if the command was recognized and handled;
/// returns `None` if the text was not a recognized built-in command.
pub fn dispatch_builtin_command(
    command: &str,
    args: &[&str],
    context: &CommandContext,
) -> Option<CommandExecutionResult> {
    let cmd = command.trim_start_matches('/').to_ascii_lowercase();

    match cmd.as_str() {
        "ping" => Some(CommandExecutionResult::reply("pong")),

        "help" => Some(CommandExecutionResult::reply(
            "Available built-in commands:\n\
             - /help: Display this assistance summary\n\
             - /status: Inspect bot operational and connection state\n\
             - /ping: Check channel responsiveness\n\
             - /id: Show your sender ID and conversation identifier\n\
             - /reset or /clear: Reset ongoing conversation context\n\
             - /stop: Halt active agent turn execution",
        )),

        "status" => Some(CommandExecutionResult::reply(format!(
            "🟢 Bot Status: Online & operational\n\
             Channel: {}\n\
             Conversation: {}\n\
             Runtime: native-fusion (OC-S3)",
            context.channel_id, context.conversation_id
        ))),

        "id" => Some(CommandExecutionResult::reply(format!(
            "Sender ID: {}\nConversation ID: {}\nChannel: {}\nAdmin Privileges: {}",
            context.sender_id, context.conversation_id, context.channel_id, context.is_admin
        ))),

        "reset" | "clear" => Some(CommandExecutionResult {
            handled: true,
            immediate_reply: Some("Conversation context has been cleared. Starting a fresh turn.".to_string()),
            should_halt: true,
            system_notice: Some("context_reset_requested".to_string()),
        }),

        "stop" => Some(CommandExecutionResult {
            handled: true,
            immediate_reply: Some("Active generation aborted.".to_string()),
            should_halt: true,
            system_notice: Some("turn_stop_requested".to_string()),
        }),

        "mode" => {
            let mode_name = args.first().copied().unwrap_or("standard");
            Some(CommandExecutionResult::reply(format!("Operating mode set to: {mode_name}")))
        }

        "model" => {
            let model_name = args.first().copied().unwrap_or("default");
            Some(CommandExecutionResult::reply(format!("Model preference updated to: {model_name}")))
        }

        _ => None,
    }
}

/// Custom command handler function signature.
pub type CustomCommandHandler = Arc<dyn Fn(&[&str], &CommandContext) -> CommandExecutionResult + Send + Sync>;

/// Extensible command registry supporting user-defined channel commands.
#[derive(Clone, Default)]
pub struct ChannelCommandRegistry {
    handlers: HashMap<String, CustomCommandHandler>,
}

impl ChannelCommandRegistry {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    pub fn register<F>(&mut self, name: &str, handler: F)
    where
        F: Fn(&[&str], &CommandContext) -> CommandExecutionResult + Send + Sync + 'static,
    {
        self.handlers.insert(name.trim_start_matches('/').to_ascii_lowercase(), Arc::new(handler));
    }

    pub fn execute(&self, raw_text: &str, context: &CommandContext) -> Option<CommandExecutionResult> {
        let trimmed = raw_text.trim();
        if !trimmed.starts_with('/') {
            return None;
        }

        let mut parts = trimmed.split_whitespace();
        let cmd = parts.next()?;
        let args: Vec<&str> = parts.collect();

        // Check custom handlers first
        let clean_cmd = cmd.trim_start_matches('/').to_ascii_lowercase();
        if let Some(handler) = self.handlers.get(&clean_cmd) {
            return Some(handler(&args, context));
        }

        // Fallback to built-in commands
        dispatch_builtin_command(cmd, &args, context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dispatch_builtin_ping_and_help() {
        let ctx = CommandContext::new("slack", "U12345", "C67890", false);

        let ping_res = dispatch_builtin_command("/ping", &[], &ctx).expect("ping handled");
        assert_eq!(ping_res.immediate_reply.as_deref(), Some("pong"));
        assert!(ping_res.should_halt);

        let help_res = dispatch_builtin_command("/help", &[], &ctx).expect("help handled");
        assert!(help_res.immediate_reply.unwrap().contains("Available built-in commands"));
    }

    #[test]
    fn test_dispatch_id_and_reset() {
        let ctx = CommandContext::new("discord", "user_99", "channel_101", true);

        let id_res = dispatch_builtin_command("/id", &[], &ctx).expect("id handled");
        assert!(id_res.immediate_reply.unwrap().contains("user_99"));

        let reset_res = dispatch_builtin_command("/reset", &[], &ctx).expect("reset handled");
        assert_eq!(reset_res.system_notice.as_deref(), Some("context_reset_requested"));
    }

    #[test]
    fn test_custom_command_registration() {
        let mut reg = ChannelCommandRegistry::new();
        reg.register("echo", |args, _ctx| {
            CommandExecutionResult::reply(args.join(" "))
        });

        let ctx = CommandContext::new("tg", "100", "200", false);
        let out = reg.execute("/echo hello world", &ctx).expect("custom echo handled");
        assert_eq!(out.immediate_reply.as_deref(), Some("hello world"));

        // Non-command text returns None
        assert!(reg.execute("just a normal message", &ctx).is_none());
    }
}
