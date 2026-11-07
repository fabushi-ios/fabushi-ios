use crate::extensions::inference::provider_session::{ProviderMessage, ProviderSessionError};
use crate::extensions::telemetry::sand_error_tags::SandErrorValue;
use crate::extensions::telemetry::turn_empty_delivery_telemetry::TurnEmptyDeliveryReport;
use crate::runner::{StreamFailureKind, TransientStreamError, TurnRunOptions};
use crate::runner::would_recover_via_prepend;

use super::send_pipeline::{PersistedSendContext, RecoverySend};

pub const MAX_REPLY_NUDGES: usize = 3;
pub const REPLY_NUDGE_PROMPT: &str = "Your previous turn left the user without the result they're waiting on — you never called SendMessage that turn, or every SendMessage you tried failed to deliver. Either way they received nothing and are still waiting. Do not assume a send from an earlier turn covered it: an opening acknowledgement back then did not deliver this result (ack ≠ delivery). Deliver the result now by actually invoking the SendMessage tool — make a real tool/function call, not text you write. Plain assistant text is NEVER shown to the user; only a real SendMessage tool invocation reaches them, so if you don't call the tool they just keep seeing silence.";
pub const CLOSING_SEND_NUDGE_PROMPT: &str = "Your previous turn acknowledged the user and then ran tool calls, but ended without a follow-up SendMessage — the last thing the user saw is that opening acknowledgement, so whatever the tool calls produced after it never reached them. If that work produced the result or answer they are waiting on, deliver it now by actually invoking the SendMessage tool — make a real tool/function call, not text you write. Plain assistant text is NEVER shown to the user; only a real SendMessage tool invocation reaches them. If the work is genuinely unfinished, continue it and send the result once you have it.";

pub fn is_delivery_owed(sent_message_count: u64, reacted: bool) -> bool {
    sent_message_count == 0 && !reacted
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnTerminalKind {
    WaitingUser,
    Cancelled,
    Completed,
    Failed,
}

impl TurnTerminalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WaitingUser => "waiting_user",
            Self::Cancelled => "cancelled",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnTerminalProjection {
    pub kind: TurnTerminalKind,
    pub delivered: bool,
    pub reply_nudge_owed: bool,
}

pub fn project_turn_terminal(
    waiting_user: bool,
    cancelled: bool,
    succeeded: bool,
    sent_message_count: u64,
    reacted: bool,
) -> TurnTerminalProjection {
    let kind = if waiting_user {
        TurnTerminalKind::WaitingUser
    } else if cancelled {
        TurnTerminalKind::Cancelled
    } else if succeeded {
        TurnTerminalKind::Completed
    } else {
        TurnTerminalKind::Failed
    };
    let delivered = !is_delivery_owed(sent_message_count, reacted);
    TurnTerminalProjection {
        kind,
        delivered,
        reply_nudge_owed: kind == TurnTerminalKind::Completed && !delivered,
    }
}

pub fn build_turn_empty_delivery_report(
    conversation_id: &str,
    request_id: Option<&str>,
    request_source: Option<&str>,
    reply_nudge_attempts: usize,
    tool_call_count: u64,
    stream_output_produced: bool,
    duration_ms: u64,
    ack_outstanding: bool,
    sent_message_count: u64,
    reacted: bool,
    waiting_user: bool,
    cancelled: bool,
    succeeded: bool,
    turn_epoch: u64,
    current_epoch: u64,
) -> Option<TurnEmptyDeliveryReport> {
    if !succeeded
        || cancelled
        || waiting_user
        || turn_epoch != current_epoch
        || !is_delivery_owed(sent_message_count, reacted)
    {
        return None;
    }
    Some(TurnEmptyDeliveryReport {
        conversation_id: conversation_id.to_string(),
        request_id: request_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        source: "turn".into(),
        request_source: request_source
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        reply_nudge_attempts: Some(reply_nudge_attempts.min(i64::MAX as usize) as i64),
        redrive_attempts: None,
        tool_call_count: tool_call_count.min(i64::MAX as u64) as i64,
        stream_output_produced,
        duration_ms: duration_ms as f64,
        ack_outstanding,
    })
}

pub fn should_attempt_reply_nudge(
    sent_message_count: u64,
    reacted: bool,
    attempts: usize,
    turn_epoch: u64,
    current_epoch: u64,
    cancelled: bool,
    waiting_user: bool,
    succeeded: bool,
) -> bool {
    succeeded
        && !cancelled
        && !waiting_user
        && attempts < MAX_REPLY_NUDGES
        && turn_epoch == current_epoch
        && is_delivery_owed(sent_message_count, reacted)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyNudgeTurnInput {
    pub lifecycle_messages: Vec<ProviderMessage>,
    pub provider_messages: Vec<ProviderMessage>,
    pub options: TurnRunOptions,
}

fn shape_hidden_nudge_turn_input(
    lifecycle_messages: &[ProviderMessage],
    provider_messages: &[ProviderMessage],
    options: &TurnRunOptions,
    prompt: &str,
) -> ReplyNudgeTurnInput {
    let pending_user_request = provider_messages
        .iter()
        .rev()
        .find(|message| message.role == "user" && !message.content.trim().is_empty())
        .or_else(|| {
            lifecycle_messages
                .iter()
                .rev()
                .find(|message| message.role == "user" && !message.content.trim().is_empty())
        })
        .map(|message| message.content.trim());
    let nudge_content = pending_user_request.map_or_else(
        || prompt.to_string(),
        |request| {
            format!(
                "{prompt}\n\nThe user request still pending from this same turn is reproduced verbatim below. Answer this exact request when you invoke SendMessage; preserve every explicit marker, constraint, and requested output detail.\n\n--- BEGIN PENDING USER REQUEST ---\n{request}\n--- END PENDING USER REQUEST ---"
            )
        },
    );
    let nudge = ProviderMessage {
        role: "user".into(),
        content: nudge_content.clone(),
    };
    let mut shaped_lifecycle_messages = lifecycle_messages.to_vec();
    shaped_lifecycle_messages.push(nudge.clone());
    let mut shaped_provider_messages = provider_messages.to_vec();
    shaped_provider_messages.push(nudge);
    let mut shaped_options = options.clone();
    shaped_options.message_id = None;
    shaped_options.recent_message_text = Some(nudge_content);
    shaped_options.recent_message_rich_text = None;
    shaped_options.recent_user_messages.clear();
    shaped_options.is_fork = false;
    shaped_options.attachment_count = 0;
    shaped_options.image_count = 0;
    shaped_options.video_count = 0;
    shaped_options.has_reply_context = false;
    ReplyNudgeTurnInput {
        lifecycle_messages: shaped_lifecycle_messages,
        provider_messages: shaped_provider_messages,
        options: shaped_options,
    }
}

pub fn shape_reply_nudge_turn_input(
    lifecycle_messages: &[ProviderMessage],
    provider_messages: &[ProviderMessage],
    options: &TurnRunOptions,
) -> ReplyNudgeTurnInput {
    shape_hidden_nudge_turn_input(
        lifecycle_messages,
        provider_messages,
        options,
        REPLY_NUDGE_PROMPT,
    )
}

pub fn shape_closing_send_nudge_turn_input(
    lifecycle_messages: &[ProviderMessage],
    provider_messages: &[ProviderMessage],
    options: &TurnRunOptions,
) -> ReplyNudgeTurnInput {
    shape_hidden_nudge_turn_input(
        lifecycle_messages,
        provider_messages,
        options,
        CLOSING_SEND_NUDGE_PROMPT,
    )
}

pub fn classify_agent_error(error: &ProviderSessionError) -> SandErrorValue {
    let message = error.to_string();
    let lower = message.to_ascii_lowercase();

    if lower.contains("first-output watchdog")
        || lower.contains("first token stall")
        || lower.contains("first-token stall")
    {
        return SandErrorValue::new("SAND-E0402");
    }
    if lower.contains("context window overflow")
        || lower.contains("context-window overflow")
        || lower.contains("context length")
    {
        return SandErrorValue::new("SAND-E0404");
    }
    if lower.contains("conversation too large")
        || lower.contains("conversation-size hard cap")
        || lower.contains("conversation size hard cap")
    {
        return SandErrorValue::new("SAND-E0414");
    }

    let transient = match error {
        ProviderSessionError::Cancelled(_) => TransientStreamError {
            kind: StreamFailureKind::Cancelled,
            message,
            retry_after_ms: None,
        },
        ProviderSessionError::Authentication(_) => TransientStreamError {
            kind: StreamFailureKind::Authentication,
            message,
            retry_after_ms: None,
        },
        ProviderSessionError::Configuration(_) => TransientStreamError {
            kind: StreamFailureKind::InvalidRequest,
            message,
            retry_after_ms: None,
        },
        ProviderSessionError::Protocol(_) => TransientStreamError {
            kind: StreamFailureKind::Protocol,
            message,
            retry_after_ms: None,
        },
        ProviderSessionError::Tool(_) => TransientStreamError {
            kind: StreamFailureKind::Unknown,
            message,
            retry_after_ms: None,
        },
        ProviderSessionError::ProviderRefusal(_) => TransientStreamError {
            kind: StreamFailureKind::Protocol,
            message,
            retry_after_ms: None,
        },
        ProviderSessionError::Transport(_) => {
            TransientStreamError::classify(message, None, None)
        }
    };

    match transient.kind {
        StreamFailureKind::Capacity | StreamFailureKind::RateLimit => {
            SandErrorValue::new("SAND-E0401")
        }
        StreamFailureKind::Timeout
        | StreamFailureKind::Transport
        | StreamFailureKind::Server => SandErrorValue::new("SAND-E0406"),
        StreamFailureKind::Authentication
        | StreamFailureKind::InvalidRequest
        | StreamFailureKind::Protocol => SandErrorValue::new("SAND-E0405"),
        StreamFailureKind::Cancelled | StreamFailureKind::Unknown => {
            SandErrorValue::new("SAND-E0407")
        }
    }
}

pub struct QueuedTurnRecoveryCheck<'a> {
    pub epoch: u64,
    pub current_epoch: u64,
    pub recovery_break_epoch: u64,
    pub prompt: &'a str,
    pub context: &'a PersistedSendContext,
    pub latest_recovery: Option<&'a RecoverySend>,
    pub is_fork: bool,
    pub has_reply_context: bool,
    pub attachment_count: usize,
    pub image_count: usize,
    pub video_count: usize,
}

pub fn should_supersede_stale_turn(args: QueuedTurnRecoveryCheck<'_>) -> bool {
    if args.epoch == args.current_epoch {
        return false;
    }
    let Some(message_id) = args
        .context
        .user_message_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    if args.is_fork
        || args.has_reply_context
        || args.attachment_count > 0
        || args.image_count > 0
        || args.video_count > 0
    {
        return false;
    }
    let Some(raw_text) = args
        .context
        .recent_user_messages
        .iter()
        .find(|message| message.id == message_id)
        .map(|message| message.text.as_str())
    else {
        return false;
    };
    if raw_text != args.prompt.trim() || args.epoch <= args.recovery_break_epoch {
        return false;
    }
    let Some(latest) = args.latest_recovery else {
        return false;
    };
    latest.epoch == args.current_epoch
        && would_recover_via_prepend(
            &latest.recent_user_messages,
            &latest.message_id,
            message_id,
        )
}
