use mahayana_host_runtime::runner::conversation_outline::{
    ConversationTurnInput, MAX_TOOL_ACTIVITY_ARGS_CHARS, OutlineItem, OutlineStep,
    OutlineToolCall, TaskToolCall, derive_outline_from_conversation_state,
    derive_outline_turns_from_conversation_state, get_outline_tool_call_name,
    get_outline_tool_call_status, get_task_summary, get_tool_call_activity_args,
    send_message_from_tool_call, strip_hidden_marker,
};
use serde_json::json;

#[test]
fn hidden_markers_and_turn_derivation_match_frozen_runner_contract() {
    assert_eq!(
        strip_hidden_marker(
            "[SAND_HIDDEN_PROMPT][SAND_TRUSTED_AUTOMATION_PROMPT]secret"
        ),
        "secret"
    );
    let turns = derive_outline_turns_from_conversation_state(&[
        ConversationTurnInput::Agent {
            raw_user_text:
                "[SAND_HIDDEN_PROMPT][SAND_TRUSTED_AUTOMATION_PROMPT]secret".into(),
            user_message_id: "message-1".into(),
            steps: vec![
                OutlineStep::AssistantMessage {
                    text: "assistant".into(),
                },
                OutlineStep::ThinkingMessage {
                    text: "thinking".into(),
                    duration_ms: 17,
                },
            ],
        },
        ConversationTurnInput::Shell {
            command: "pwd".into(),
        },
    ]);
    assert_eq!(turns.len(), 2);
    assert!(matches!(
        &turns[0].items[0],
        OutlineItem::User { id, text, hidden }
            if id == "outline-user-0" && text == "secret" && *hidden
    ));
    assert!(matches!(
        &turns[0].items[1],
        OutlineItem::AssistantText { id, text }
            if id == "outline-0-0" && text == "assistant"
    ));
    assert!(matches!(
        &turns[0].items[2],
        OutlineItem::Thinking { duration_ms, .. } if *duration_ms == Some(17)
    ));
    assert!(matches!(
        &turns[1].items[0],
        OutlineItem::ToolCall { name, status, summary, .. }
            if name == "shellToolCall" && status == "done" && summary.as_deref() == Some("pwd")
    ));
    assert_eq!(
        derive_outline_from_conversation_state(&[
            ConversationTurnInput::Shell { command: String::new() }
        ]).len(),
        1
    );
}

#[test]
fn tool_names_summaries_status_and_send_message_match_frozen_semantics() {
    let task = OutlineToolCall {
        case: Some("taskToolCall".into()),
        task: Some(TaskToolCall {
            description: Some(" description ".into()),
            prompt: Some(" prompt ".into()),
            error: Some("failed".into()),
        }),
        ..OutlineToolCall::default()
    };
    assert_eq!(get_outline_tool_call_name(&task), "Task");
    assert_eq!(get_task_summary(task.task.as_ref().unwrap()).as_deref(), Some("failed"));
    assert_eq!(get_outline_tool_call_status("toolCallCompleted", &task), "failed");
    assert_eq!(get_outline_tool_call_status("toolCallStarted", &task), "pending");

    let screenshot = OutlineToolCall {
        case: Some("computerUseToolCall".into()),
        computer_action_cases: vec!["screenshot".into()],
        ..OutlineToolCall::default()
    };
    assert_eq!(get_outline_tool_call_name(&screenshot), "Screenshot");

    let send = OutlineToolCall {
        case: Some("sendMessageToolCall".into()),
        send_message: Some(json!({"type":"attachment","url":"file://a","alt":"A"})),
        ..OutlineToolCall::default()
    };
    assert_eq!(
        send_message_from_tool_call(&send),
        Some(json!({"type":"attachment","url":"file://a","alt":"A"}))
    );
}

#[test]
fn tool_activity_args_are_filtered_and_bounded() {
    assert!(get_tool_call_activity_args(&OutlineToolCall {
        activity_args: Some(json!({})),
        ..OutlineToolCall::default()
    })
    .is_none());

    let short = OutlineToolCall {
        activity_args: Some(json!({"query":"hello"})),
        ..OutlineToolCall::default()
    };
    assert_eq!(
        get_tool_call_activity_args(&short).as_deref(),
        Some(r#"{"query":"hello"}"#)
    );

    let long = OutlineToolCall {
        activity_args: Some(json!({"value":"x".repeat(MAX_TOOL_ACTIVITY_ARGS_CHARS + 50)})),
        ..OutlineToolCall::default()
    };
    let projected = get_tool_call_activity_args(&long).expect("truncated args");
    assert!(projected.ends_with("\n… (truncated)"));
}
