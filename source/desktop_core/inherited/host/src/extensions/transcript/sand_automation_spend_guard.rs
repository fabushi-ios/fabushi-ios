use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::automations::automation::AutomationRecord;

pub const SPEND_GUARD_IDLE_TTL_MS: f64 = 3.0 * 24.0 * 60.0 * 60_000.0;
pub const SPEND_GUARD_MIN_UNREAD_COUNT: usize = 15;
pub const SPEND_GUARD_MIN_FIRES_SINCE_VIEWED: usize = 20;
pub const SPEND_GUARD_PAUSE_DELAY_MS: f64 = 3.0 * 24.0 * 60.0 * 60_000.0;
pub const SPEND_GUARD_SNOOZE_MS: f64 = 30.0 * 24.0 * 60.0 * 60_000.0;
pub const SPEND_GUARD_VALUE_PREFIX: &str = "spend-guard:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpendGuardAnswer {
    Keep,
    Pause,
    OptOut,
    Resume,
    StayPaused,
}

impl SpendGuardAnswer {
    pub const fn value(self) -> &'static str {
        match self {
            Self::Keep => "spend-guard:keep",
            Self::Pause => "spend-guard:pause",
            Self::OptOut => "spend-guard:never-ask",
            Self::Resume => "spend-guard:resume",
            Self::StayPaused => "spend-guard:stay-paused",
        }
    }
}

pub fn interpret_spend_guard_answer(value: &str) -> Option<SpendGuardAnswer> {
    [
        SpendGuardAnswer::Keep,
        SpendGuardAnswer::Pause,
        SpendGuardAnswer::OptOut,
        SpendGuardAnswer::Resume,
        SpendGuardAnswer::StayPaused,
    ]
    .into_iter()
    .find(|answer| answer.value() == value)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpendGuardEvaluation {
    pub now_ms: f64,
    pub last_viewed_at_ms: f64,
    pub unread_count: usize,
    pub fires_since_viewed_count: usize,
    pub nudged_at_ms: Option<f64>,
    pub snoozed_until_ms: Option<f64>,
    pub opted_out: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpendGuardDecision {
    OptedOut,
    UserActive,
    Snoozed,
    Pause,
    AwaitingAck,
    Nudge,
    BelowThresholds,
}

pub fn evaluate_automation_spend_guard(input: SpendGuardEvaluation) -> SpendGuardDecision {
    if input.opted_out {
        return SpendGuardDecision::OptedOut;
    }
    if input.now_ms - input.last_viewed_at_ms < SPEND_GUARD_IDLE_TTL_MS {
        return SpendGuardDecision::UserActive;
    }
    if input
        .snoozed_until_ms
        .is_some_and(|until| input.now_ms < until)
    {
        return SpendGuardDecision::Snoozed;
    }
    if let Some(nudged_at_ms) = input
        .nudged_at_ms
        .filter(|nudged| *nudged > input.last_viewed_at_ms)
    {
        return if input.now_ms - nudged_at_ms >= SPEND_GUARD_PAUSE_DELAY_MS {
            SpendGuardDecision::Pause
        } else {
            SpendGuardDecision::AwaitingAck
        };
    }
    if input.unread_count >= SPEND_GUARD_MIN_UNREAD_COUNT
        || input.fires_since_viewed_count >= SPEND_GUARD_MIN_FIRES_SINCE_VIEWED
    {
        return SpendGuardDecision::Nudge;
    }
    SpendGuardDecision::BelowThresholds
}

pub fn count_automation_runs_since(automations: &[AutomationRecord], since_ms: f64) -> usize {
    automations
        .iter()
        .flat_map(|automation| automation.runs.iter())
        .filter(|run| run.started_at > since_ms)
        .count()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendGuardWidgetOption {
    pub label: String,
    pub value: String,
    pub style: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendGuardWidget {
    pub prompt: String,
    pub options: Vec<SpendGuardWidgetOption>,
}

pub fn build_spend_guard_nudge_widget() -> SpendGuardWidget {
    SpendGuardWidget {
        prompt: "You've been away for a bit — keep my routines running?".into(),
        options: vec![
            SpendGuardWidgetOption {
                label: "Keep them running".into(),
                value: SpendGuardAnswer::Keep.value().into(),
                style: Some("primary".into()),
            },
            SpendGuardWidgetOption {
                label: "Pause them all".into(),
                value: SpendGuardAnswer::Pause.value().into(),
                style: None,
            },
            SpendGuardWidgetOption {
                label: "Keep running, don't ask again".into(),
                value: SpendGuardAnswer::OptOut.value().into(),
                style: None,
            },
        ],
    }
}

pub fn build_spend_guard_paused_widget() -> SpendGuardWidget {
    SpendGuardWidget {
        prompt: "I paused all your routines while you were away to avoid wasted spend. Want me to start them back up?".into(),
        options: vec![
            SpendGuardWidgetOption {
                label: "Resume routines".into(),
                value: SpendGuardAnswer::Resume.value().into(),
                style: Some("primary".into()),
            },
            SpendGuardWidgetOption {
                label: "Keep them paused".into(),
                value: SpendGuardAnswer::StayPaused.value().into(),
                style: None,
            },
        ],
    }
}

pub fn render_spend_guard_answer_ack(answer: SpendGuardAnswer) -> String {
    let choice = match answer {
        SpendGuardAnswer::Keep => {
            "keep your routines running, and not to be asked again for a month"
        }
        SpendGuardAnswer::Resume => "start the paused routines back up",
        SpendGuardAnswer::OptOut => {
            "keep your routines running and never be asked about this again"
        }
        SpendGuardAnswer::Pause => "pause every one of your routines",
        SpendGuardAnswer::StayPaused => "leave your routines paused",
    };
    [
        "<system_reminder>".to_string(),
        format!(
            "The app asked the user about the money your routines spend while they are away. They chose to {choice}, and the app has ALREADY applied that itself."
        ),
        "Acknowledge their choice in one short line. Do NOT edit any automation.json and do NOT ask again.".into(),
        "</system_reminder>".into(),
    ]
    .join("\n")
}

pub fn is_spend_guard_card(widget: &SpendGuardWidget, value: &str) -> bool {
    let known_prompt = widget.prompt == build_spend_guard_nudge_widget().prompt
        || widget.prompt == build_spend_guard_paused_widget().prompt;
    known_prompt && widget.options.iter().any(|option| option.value == value)
}

pub fn render_spend_guard_nudge_reminder(
    input: SpendGuardEvaluation,
    time_zone: Option<&str>,
) -> String {
    let away = if input.last_viewed_at_ms > 0.0 {
        format!(
            "hasn't opened this chat since {}",
            format_timestamp(input.last_viewed_at_ms, time_zone)
        )
    } else {
        "has never opened this chat".into()
    };
    let deadline = format_timestamp(input.now_ms + SPEND_GUARD_PAUSE_DELAY_MS, time_zone);
    [
        "<system_reminder>".to_string(),
        format!(
            "The user {away} — {} of your messages are unread and your routines have run {} times since then. They may be spending money on work nobody is reading.",
            input.unread_count, input.fires_since_viewed_count
        ),
        "The app has already asked them directly whether to keep your routines running, and applies their answer itself. Do NOT ask again yourself and do NOT edit any automation.json; just acknowledge their choice if it comes back as their reply.".into(),
        format!(
            "If they neither answer nor return by {deadline}, the app will pause ALL of this agent's routines and tell them so."
        ),
        "</system_reminder>".into(),
    ]
    .join("\n")
}

fn format_timestamp(ms: f64, time_zone: Option<&str>) -> String {
    if !ms.is_finite() {
        return "never".into();
    }
    let millis = ms.trunc();
    if millis < i64::MIN as f64 || millis > i64::MAX as f64 {
        return "never".into();
    }
    let Some(datetime) = DateTime::<Utc>::from_timestamp_millis(millis as i64) else {
        return "never".into();
    };
    if let Some(zone) = time_zone.and_then(|value| value.parse::<Tz>().ok()) {
        return datetime
            .with_timezone(&zone)
            .format("%Y-%m-%d %H:%M:%S %Z")
            .to_string();
    }
    datetime.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}
