use serde_json::Value;

use super::send_message_reminder_middleware::{
    DEFAULT_EARLY_RESULT_REMINDER_THRESHOLD, DEFAULT_SEND_MESSAGE_REMINDER_THRESHOLD,
    EARLY_RESULT_REMINDER_MESSAGE, SAND_SEND_MESSAGE_TOOL_NAME, SEND_MESSAGE_REMINDER_MESSAGE,
};
use super::start_of_turn_ack_reminder_middleware::{
    DEFAULT_START_OF_TURN_ACK_THRESHOLD, START_OF_TURN_ACK_REMINDER_MESSAGE,
    is_text_send_message_args,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnToolSessionReminderState {
    tool_calls_since_boundary: usize,
    saw_send_message_since_turn_start: bool,
    saw_text_send_message_since_turn_start: bool,
    reminder_fired_this_silent_streak: bool,
}

impl TurnToolSessionReminderState {
    pub fn observe_tool_step(&mut self, calls: &[(String, Value)]) -> Option<String> {
        if calls.is_empty() { return None; }
        let sends = calls.iter().filter(|(name, _)| name == SAND_SEND_MESSAGE_TOOL_NAME).collect::<Vec<_>>();
        if !sends.is_empty() {
            self.saw_send_message_since_turn_start = true;
            if sends.iter().any(|(_, args)| is_text_send_message_args(Some(args))) {
                self.saw_text_send_message_since_turn_start = true;
            }
            self.tool_calls_since_boundary = 0;
            self.reminder_fired_this_silent_streak = false;
            return None;
        }
        self.tool_calls_since_boundary = self.tool_calls_since_boundary.saturating_add(calls.len());
        if !self.saw_text_send_message_since_turn_start
            && self.tool_calls_since_boundary > DEFAULT_START_OF_TURN_ACK_THRESHOLD
        {
            self.tool_calls_since_boundary = 0;
            self.reminder_fired_this_silent_streak = true;
            return Some(START_OF_TURN_ACK_REMINDER_MESSAGE.to_string());
        }
        if self.tool_calls_since_boundary > DEFAULT_SEND_MESSAGE_REMINDER_THRESHOLD {
            self.tool_calls_since_boundary = 0;
            self.reminder_fired_this_silent_streak = true;
            return Some(SEND_MESSAGE_REMINDER_MESSAGE.to_string());
        }
        if self.tool_calls_since_boundary > DEFAULT_EARLY_RESULT_REMINDER_THRESHOLD
            && self.saw_send_message_since_turn_start
            && !self.reminder_fired_this_silent_streak
        {
            self.tool_calls_since_boundary = 0;
            self.reminder_fired_this_silent_streak = true;
            return Some(EARLY_RESULT_REMINDER_MESSAGE.to_string());
        }
        None
    }
}
