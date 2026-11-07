//! Outbound message formatting, code-fence-preserving chunking, and thread parameters (OC-S3).

use serde::{Deserialize, Serialize};

use super::types::{ChannelKind, OutboundDeliveryPayload};

pub const DISCORD_DEFAULT_MAX_CHARS: usize = 2_000;
pub const TELEGRAM_DEFAULT_MAX_CHARS: usize = 4_096;
pub const SLACK_DEFAULT_MAX_CHARS: usize = 4_000;
pub const WHATSAPP_DEFAULT_MAX_CHARS: usize = 4_096;

/// Splits Markdown text into chunks of at most `max_len` characters while preserving
/// balanced triple-backtick code fences across every emitted piece.
pub fn chunk_text_preserving_code_blocks(text: &str, max_len: usize) -> Vec<String> {
    let limit = max_len.max(10);
    if text.chars().count() <= limit {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut in_code_fence = false;
    let mut fence_lang = String::new();

    for line in text.lines() {
        let mut remaining_line = line;
        while !remaining_line.is_empty() {
            let separator = usize::from(!current.is_empty());
            let rem_chars = remaining_line.chars().count();
            if current.chars().count() + separator + rem_chars <= limit {
                if !current.is_empty() {
                    current.push('\n');
                }
                current.push_str(remaining_line);
                break;
            }

            if !current.is_empty() {
                if in_code_fence {
                    current.push_str("\n```");
                    chunks.push(current);
                    current = if fence_lang.is_empty() {
                        "```".to_string()
                    } else {
                        format!("```{fence_lang}")
                    };
                } else {
                    chunks.push(current);
                    current = String::new();
                }
            }

            let available = limit.saturating_sub(current.chars().count() + 1).max(1);
            if rem_chars <= available {
                if !current.is_empty() {
                    current.push('\n');
                }
                current.push_str(remaining_line);
                break;
            }

            let take_len = available.min(rem_chars);
            let split_idx = remaining_line
                .char_indices()
                .take(take_len)
                .last()
                .map(|(idx, ch)| idx + ch.len_utf8())
                .unwrap_or(remaining_line.len());

            let (head, tail) = remaining_line.split_at(split_idx);
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(head);
            remaining_line = tail;
        }

        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            if in_code_fence {
                in_code_fence = false;
                fence_lang.clear();
            } else {
                in_code_fence = true;
                fence_lang = trimmed
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

/// Splits outbound text specifically for Discord's character limit (2,000 characters by default),
/// guaranteeing that code blocks remain syntax-highlighted and uncorrupted.
pub fn chunk_discord_text(text: &str, max_len: usize) -> Vec<String> {
    let cap = if max_len == 0 { DISCORD_DEFAULT_MAX_CHARS } else { max_len };
    chunk_text_preserving_code_blocks(text, cap)
}

/// Telegram API message sending thread and reply parameters.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TelegramSendParams {
    pub message_thread_id: Option<i64>,
    pub reply_to_message_id: Option<i64>,
    pub is_forum_topic: bool,
}

/// Builds Telegram delivery parameters distinguishing forum topic messages from reply threads.
pub fn build_telegram_thread_params(
    thread_id: Option<&str>,
    is_topic: bool,
) -> TelegramSendParams {
    let Some(raw_id) = thread_id else {
        return TelegramSendParams::default();
    };

    let parsed_id = raw_id.parse::<i64>().ok();

    if is_topic {
        TelegramSendParams {
            message_thread_id: parsed_id,
            reply_to_message_id: None,
            is_forum_topic: true,
        }
    } else {
        TelegramSendParams {
            message_thread_id: None,
            reply_to_message_id: parsed_id,
            is_forum_topic: false,
        }
    }
}

/// Normalizes a WhatsApp recipient identifier (raw phone number, JID, or group JID) into standard JID.
///
/// Supported formats:
/// - `+1 (555) 123-4567` -> `15551234567@s.whatsapp.net`
/// - `15551234567` -> `15551234567@s.whatsapp.net`
/// - `123456789-987654@g.us` -> `123456789-987654@g.us` (group preserved)
/// - `123456789@lid` -> `123456789@lid` (LID preserved)
pub fn normalize_whatsapp_jid(recipient: &str) -> Result<String, String> {
    let trimmed = recipient.trim();
    if trimmed.is_empty() {
        return Err("WhatsApp recipient identifier cannot be empty".to_string());
    }

    // Already formatted JID
    if trimmed.ends_with("@s.whatsapp.net") || trimmed.ends_with("@g.us") || trimmed.ends_with("@lid") {
        return Ok(trimmed.to_string());
    }

    // If it contains @ but unrecognized domain
    if trimmed.contains('@') {
        return Err(format!("Unrecognized WhatsApp JID domain: {trimmed}"));
    }

    // Clean phone number: remove +, space, -, (, ), .
    let digits: String = trimmed.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() < 7 || digits.len() > 16 {
        return Err(format!("Invalid phone number length for WhatsApp JID: {recipient}"));
    }

    Ok(format!("{digits}@s.whatsapp.net"))
}

/// Helper that transforms an outbound delivery payload into channel-sized chunks.
pub fn prepare_outbound_chunks(
    payload: &OutboundDeliveryPayload,
) -> Vec<OutboundDeliveryPayload> {
    let limit = match payload.channel {
        ChannelKind::Discord => DISCORD_DEFAULT_MAX_CHARS,
        ChannelKind::Telegram => TELEGRAM_DEFAULT_MAX_CHARS,
        ChannelKind::Slack => SLACK_DEFAULT_MAX_CHARS,
        ChannelKind::WhatsApp => WHATSAPP_DEFAULT_MAX_CHARS,
        _ => 4_000,
    };

    let text_chunks = chunk_text_preserving_code_blocks(&payload.text, limit);
    if text_chunks.len() <= 1 {
        return vec![payload.clone()];
    }

    let mut payloads = Vec::with_capacity(text_chunks.len());
    for (i, chunk) in text_chunks.into_iter().enumerate() {
        let mut p = payload.clone();
        p.text = chunk;
        if i > 0 {
            // Subsequent chunks should not re-attach files
            p.attachments.clear();
        }
        payloads.push(p);
    }

    payloads
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_discord_text_code_fences() {
        let input = "Hello world\n```python\nfor i in range(10):\n    print(i)\n```\nGoodbye!";
        let chunks = chunk_discord_text(input, 40);

        assert!(chunks.len() > 1);
        for chunk in &chunks {
            // Check code fence balance
            let count = chunk.matches("```").count();
            assert_eq!(count % 2, 0, "Code fences must be balanced in chunk: {chunk}");
        }
    }

    #[test]
    fn test_build_telegram_thread_params() {
        let topic_params = build_telegram_thread_params(Some("789"), true);
        assert_eq!(topic_params.message_thread_id, Some(789));
        assert_eq!(topic_params.reply_to_message_id, None);
        assert!(topic_params.is_forum_topic);

        let reply_params = build_telegram_thread_params(Some("456"), false);
        assert_eq!(reply_params.message_thread_id, None);
        assert_eq!(reply_params.reply_to_message_id, Some(456));
        assert!(!reply_params.is_forum_topic);

        let none_params = build_telegram_thread_params(None, false);
        assert_eq!(none_params.message_thread_id, None);
        assert_eq!(none_params.reply_to_message_id, None);
    }

    #[test]
    fn test_normalize_whatsapp_jid() {
        assert_eq!(
            normalize_whatsapp_jid("+1 (555) 123-4567").unwrap(),
            "15551234567@s.whatsapp.net"
        );
        assert_eq!(
            normalize_whatsapp_jid("447123456789@s.whatsapp.net").unwrap(),
            "447123456789@s.whatsapp.net"
        );
        assert_eq!(
            normalize_whatsapp_jid("12345-67890@g.us").unwrap(),
            "12345-67890@g.us"
        );
        assert!(normalize_whatsapp_jid("bad@unknown.org").is_err());
        assert!(normalize_whatsapp_jid("123").is_err());
    }

    #[test]
    fn test_prepare_outbound_chunks() {
        let long_text = "line 1\n".repeat(400); // ~2800 chars
        let payload = OutboundDeliveryPayload::new(ChannelKind::Discord, "ch-1", long_text);
        let chunks = prepare_outbound_chunks(&payload);
        assert!(chunks.len() >= 2);
        for c in chunks {
            assert!(c.text.chars().count() <= DISCORD_DEFAULT_MAX_CHARS);
        }
    }
}
