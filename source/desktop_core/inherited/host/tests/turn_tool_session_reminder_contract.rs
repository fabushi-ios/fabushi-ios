use mahayana_host_runtime::runner::send_message_reminder_middleware::{
    EARLY_RESULT_REMINDER_MESSAGE, SAND_SEND_MESSAGE_TOOL_NAME, SEND_MESSAGE_REMINDER_MESSAGE,
};
use mahayana_host_runtime::runner::start_of_turn_ack_reminder_middleware::
    START_OF_TURN_ACK_REMINDER_MESSAGE;
use mahayana_host_runtime::runner::turn_tool_session_reminders::TurnToolSessionReminderState;
use serde_json::json;

fn call(name: &str, args: serde_json::Value) -> (String, serde_json::Value) {
    (name.to_string(), args)
}

#[test]
fn reminder_state_preserves_frozen_ack_send_and_early_result_order() {
    let mut state = TurnToolSessionReminderState::default();
    assert_eq!(state.observe_tool_step(&[call("Shell", json!({}))]), None);
    assert_eq!(state.observe_tool_step(&[call("Read", json!({}))]), Some(START_OF_TURN_ACK_REMINDER_MESSAGE.to_string()));
    assert_eq!(state.observe_tool_step(&[call(SAND_SEND_MESSAGE_TOOL_NAME, json!({"type":"text","text":"Checking now."}))]), None);
    assert_eq!(state.observe_tool_step(&[call("Read", json!({}))]), Some(EARLY_RESULT_REMINDER_MESSAGE.to_string()));
    for _ in 0..6 { assert_eq!(state.observe_tool_step(&[call("Read", json!({}))]), None); }
    assert_eq!(state.observe_tool_step(&[call("Shell", json!({}))]), Some(SEND_MESSAGE_REMINDER_MESSAGE.to_string()));
}

#[test]
fn any_send_message_call_resets_the_current_silent_streak() {
    let mut state = TurnToolSessionReminderState::default();
    assert_eq!(state.observe_tool_step(&[call("Read", json!({}))]), None);
    assert_eq!(state.observe_tool_step(&[call(SAND_SEND_MESSAGE_TOOL_NAME, json!({"type":"widget","widget":{"id":"demo"}}))]), None);
    assert_eq!(state.observe_tool_step(&[call("Read", json!({}))]), Some(EARLY_RESULT_REMINDER_MESSAGE.to_string()));
}
