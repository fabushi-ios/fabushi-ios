use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WhatsAppAckReactionMode {
    Always,
    Mentions,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AckReactionScope {
    Direct,
    GroupMentions,
    GroupAll,
}

/// Evaluates whether an ACK reaction should be posted for WhatsApp channels.
pub fn should_ack_reaction_for_whatsapp(
    emoji: &str,
    is_direct: bool,
    is_group: bool,
    direct_enabled: bool,
    group_mode: WhatsAppAckReactionMode,
    was_mentioned: bool,
    group_activated: bool,
) -> bool {
    if emoji.is_empty() {
        return false;
    }
    if is_direct {
        return direct_enabled;
    }
    if !is_group || group_mode == WhatsAppAckReactionMode::Never {
        return false;
    }
    if group_mode == WhatsAppAckReactionMode::Always {
        return true;
    }

    // Mentions mode
    was_mentioned || group_activated
}

pub const BUILD_TOOL_TOKENS: &[&str] = &["build", "compile", "cargo", "make", "gradle", "mvn"];
pub const CODING_TOOL_TOKENS: &[&str] = &["edit", "write", "patch", "git", "code", "file"];
pub const CONCIERGE_TOOL_TOKENS: &[&str] = &["search", "lookup", "find", "ask", "query"];
pub const DEPLOY_TOOL_TOKENS: &[&str] = &["deploy", "publish", "release", "push", "upload"];
pub const WEB_TOOL_TOKENS: &[&str] = &["browse", "fetch", "http", "curl", "web", "url"];

/// Resolves a contextual emoji representation for tool execution activity.
pub fn resolve_tool_emoji(tool_name: &str) -> &'static str {
    let lower = tool_name.to_ascii_lowercase();
    if CODING_TOOL_TOKENS.iter().any(|t| lower.contains(t)) {
        "💻"
    } else if BUILD_TOOL_TOKENS.iter().any(|t| lower.contains(t)) {
        "🔨"
    } else if DEPLOY_TOOL_TOKENS.iter().any(|t| lower.contains(t)) {
        "🚀"
    } else if WEB_TOOL_TOKENS.iter().any(|t| lower.contains(t)) {
        "🌐"
    } else if CONCIERGE_TOOL_TOKENS.iter().any(|t| lower.contains(t)) {
        "🔍"
    } else {
        "⚡"
    }
}
