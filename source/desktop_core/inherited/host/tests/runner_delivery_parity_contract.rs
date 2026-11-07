use mahayana_host_runtime::runner::send_message_reminder_middleware::{
    CursorProviderOptions, EARLY_RESULT_REMINDER_MESSAGE, MessageContent, MessageLike, MessagePart,
    PromptExecutor, ProviderOptions, SEND_MESSAGE_REMINDER_MESSAGE,
    create_disk_pressure_reminder_middleware, create_send_message_reminder_middleware,
    count_tool_calls_since_last_send_message, has_reminder_fired_this_silent_streak,
    has_send_message_since_real_turn_start, is_injected_reminder_message,
};
use mahayana_host_runtime::runner::start_of_turn_ack_reminder_middleware::{
    START_OF_TURN_ACK_REMINDER_MESSAGE, create_start_of_turn_ack_reminder_middleware,
    has_text_send_message_since_turn_start,
};
use mahayana_host_runtime::extensions::inference::provider_session::{
    OpenRouterCheckpoint, RoutedProviderCheckpoint,
};
use mahayana_host_runtime::runner::turn_shape::{
    checkpoint_ended_on_silent_tool_calls, turn_ended_on_silent_tool_calls,
};
use mahayana_host_runtime::runner::tools::sand_computer_use_subagent::{
    COMPUTER_USE_SUBAGENT_TYPE, computer_use_subagent_description,
    create_sand_computer_use_subagent_config, is_computer_use_subagent_type,
};
use serde_json::json;

#[derive(Debug, Clone)]
struct FakeExecutor {
    messages: Vec<MessageLike>,
    state: Vec<MessageLike>,
    streams: usize,
}

impl FakeExecutor {
    fn new(messages: Vec<MessageLike>) -> Self {
        Self {
            state: messages.clone(),
            messages,
            streams: 0,
        }
    }
}

impl PromptExecutor for FakeExecutor {
    type StreamArgs = ();
    type StreamOutput = usize;

    fn get_messages(&self) -> &[MessageLike] {
        &self.messages
    }

    fn get_state(&self) -> &[MessageLike] {
        &self.state
    }

    fn clear_messages(&mut self) {
        self.messages.clear();
    }

    fn append_messages(&mut self, messages: Vec<MessageLike>) {
        self.messages.extend(messages);
    }

    fn stream(&mut self, _args: ()) -> usize {
        self.streams += 1;
        self.streams
    }
}

fn tool_call(name: &str, id: &str, args: serde_json::Value) -> MessageLike {
    MessageLike::parts(
        "assistant",
        vec![MessagePart {
            r#type: Some("tool-call".into()),
            tool_name: Some(name.into()),
            tool_call_id: Some(id.into()),
            args: Some(args),
            ..MessagePart::default()
        }],
    )
}

#[test]
fn send_message_reminders_match_silent_streak_rules() {
    let messages = vec![
        MessageLike::text("user", "do the task"),
        tool_call("Read", "t1", json!({})),
        tool_call("Write", "t2", json!({})),
    ];
    assert_eq!(count_tool_calls_since_last_send_message(&messages), 2);
    assert!(!has_send_message_since_real_turn_start(&messages));

    let executor = FakeExecutor::new(messages);
    let mut middleware = create_send_message_reminder_middleware(executor, Some(1), Some(0));
    assert_eq!(middleware.stream(()), 1);
    let injected = middleware.get_messages().last().expect("main reminder");
    assert!(is_injected_reminder_message(injected));
    assert!(matches!(
        &injected.content,
        MessageContent::Text(text) if text == SEND_MESSAGE_REMINDER_MESSAGE
    ));

    let acknowledged = vec![
        MessageLike::text("user", "do the task"),
        tool_call("SendMessage", "send-1", json!({"type":"text","text":"Working on it."})),
        tool_call("Read", "t1", json!({})),
    ];
    assert!(has_send_message_since_real_turn_start(&acknowledged));
    assert!(!has_reminder_fired_this_silent_streak(&acknowledged));
    let mut early = create_send_message_reminder_middleware(
        FakeExecutor::new(acknowledged),
        Some(6),
        Some(0),
    );
    early.stream(());
    assert!(matches!(
        &early.get_messages().last().expect("early reminder").content,
        MessageContent::Text(text) if text == EARLY_RESULT_REMINDER_MESSAGE
    ));
    assert!(has_reminder_fired_this_silent_streak(early.get_messages()));

    let mut disk = create_disk_pressure_reminder_middleware(
        FakeExecutor::new(vec![MessageLike::text("user", "continue")]),
        "pressure-episode-1",
    );
    disk.stream(());
    let once = disk.get_messages().len();
    disk.stream(());
    assert_eq!(disk.get_messages().len(), once);
    assert!(disk.get_messages().iter().any(|message| {
        message.provider_options.as_ref()
            .and_then(|options| options.cursor.as_ref())
            .is_some_and(|cursor| {
                cursor.sand_disk_pressure_reminder
                    && cursor.sand_disk_pressure_reminder_episode_id.as_deref()
                        == Some("pressure-episode-1")
            })
    }));
}

#[test]
fn start_of_turn_ack_requires_a_real_text_send_message() {
    let silent = vec![
        MessageLike::text("user", "please investigate"),
        tool_call("Read", "t1", json!({})),
        tool_call("Search", "t2", json!({})),
    ];
    let mut middleware = create_start_of_turn_ack_reminder_middleware(
        FakeExecutor::new(silent),
        Some(1),
        None,
    );
    middleware.stream(());
    let reminder = middleware.get_messages().last().expect("ack reminder");
    assert!(matches!(
        &reminder.content,
        MessageContent::Text(text) if text == START_OF_TURN_ACK_REMINDER_MESSAGE
    ));
    assert!(is_injected_reminder_message(reminder));

    let widget_only = vec![
        MessageLike::text("user", "please investigate"),
        tool_call("SendMessage", "send-widget", json!({"type":"widget"})),
    ];
    assert!(!has_text_send_message_since_turn_start(&widget_only));

    let text_ack = vec![
        MessageLike::text("user", "please investigate"),
        tool_call("SendMessage", "send-text", json!({"type":"text","text":"Checking now."})),
        tool_call("Read", "t1", json!({})),
        tool_call("Search", "t2", json!({})),
    ];
    assert!(has_text_send_message_since_turn_start(&text_ack));
    let mut no_injection = create_start_of_turn_ack_reminder_middleware(
        FakeExecutor::new(text_ack.clone()),
        Some(1),
        None,
    );
    no_injection.stream(());
    assert_eq!(no_injection.get_messages(), text_ack.as_slice());
}

#[test]
fn turn_shape_detects_only_silent_tool_call_endings_after_visible_ack() {
    let silent_tail = vec![
        json!({"role":"user","content":"do it"}),
        json!({"role":"assistant","content":[
            {"type":"tool-call","toolName":"SendMessage","toolCallId":"ack-1","args":{"type":"text"}}
        ]}),
        json!({"role":"assistant","content":[
            {"type":"tool-call","toolName":"Read","toolCallId":"read-1"}
        ]}),
        json!({"role":"tool","content":[
            {"type":"tool-result","toolCallId":"read-1"}
        ]}),
    ];
    assert!(turn_ended_on_silent_tool_calls(&silent_tail));

    let visible_tail = vec![
        json!({"role":"user","content":"do it"}),
        json!({"role":"assistant","content":[
            {"type":"tool-call","toolName":"SendMessage","toolCallId":"ack-1","args":{"type":"text"}}
        ]}),
        json!({"role":"assistant","content":[
            {"type":"tool-call","toolName":"Read","toolCallId":"read-1"}
        ]}),
        json!({"role":"assistant","content":[
            {"type":"tool-call","toolName":"SendMessage","toolCallId":"final-1","args":{"type":"text"}}
        ]}),
        json!({"role":"assistant","content":[
            {"type":"tool-call","toolName":"Read","toolCallId":"read-2"}
        ]}),
    ];
    assert!(!turn_ended_on_silent_tool_calls(&visible_tail));

    let errored_later_delivery = vec![
        json!({"role":"user","content":"do it"}),
        json!({"role":"assistant","content":[
            {"type":"tool-call","toolName":"SendMessage","toolCallId":"ack-1","args":{"type":"text"}}
        ]}),
        json!({"role":"assistant","content":[
            {"type":"tool-call","toolName":"SendMessage","toolCallId":"failed-send","args":{"type":"text"}}
        ]}),
        json!({"role":"tool","content":[
            {"type":"tool-result","toolCallId":"failed-send"}
        ],"providerOptions":{"cursor":{"highLevelToolCallResult":{"isError":true}}}}),
        json!({"role":"assistant","content":[
            {"type":"tool-call","toolName":"Read","toolCallId":"read-2"}
        ]}),
    ];
    assert!(turn_ended_on_silent_tool_calls(&errored_later_delivery));

    let never_acked = vec![
        json!({"role":"user","content":"do it"}),
        json!({"role":"assistant","content":[
            {"type":"tool-call","toolName":"Read","toolCallId":"read-1"}
        ]}),
    ];
    assert!(!turn_ended_on_silent_tool_calls(&never_acked));

    let reminder_tail = vec![
        json!({"role":"user","content":"do it"}),
        json!({"role":"assistant","content":[
            {"type":"tool-call","toolName":"SendMessage","toolCallId":"ack-1","args":{"type":"text"}}
        ]}),
        json!({"role":"assistant","content":[
            {"type":"tool-call","toolName":"Read","toolCallId":"read-1"}
        ]}),
        json!({"role":"user","content":START_OF_TURN_ACK_REMINDER_MESSAGE,
            "providerOptions":{"cursor":{"sandStartOfTurnAckReminder":true}}}),
    ];
    assert!(turn_ended_on_silent_tool_calls(&reminder_tail));
}

#[test]
fn durable_provider_checkpoint_drives_silent_tool_tail_projection() {
    let checkpoint = RoutedProviderCheckpoint::OpenRouter(OpenRouterCheckpoint {
        conversation: vec![
            json!({"role":"user","content":"do it"}),
            json!({
                "role":"assistant",
                "content": null,
                "tool_calls":[{
                    "id":"ack-1",
                    "type":"function",
                    "function":{"name":"SendMessage","arguments":"{\"type\":\"text\"}"}
                }]
            }),
            json!({"role":"tool","tool_call_id":"ack-1","content":"{\"ok\":true}"}),
            json!({
                "role":"assistant",
                "content": null,
                "tool_calls":[{
                    "id":"read-1",
                    "type":"function",
                    "function":{"name":"Read","arguments":"{}"}
                }]
            }),
            json!({"role":"tool","tool_call_id":"read-1","content":"{\"value\":1}"}),
        ],
        text: String::new(),
        completed_steps: 2,
        tool_calls_completed: 2,
    });
    assert!(checkpoint_ended_on_silent_tool_calls(&checkpoint, ""));
    assert!(!checkpoint_ended_on_silent_tool_calls(
        &checkpoint,
        "plain model text after the last checkpoint"
    ));

    let visible = RoutedProviderCheckpoint::OpenRouter(OpenRouterCheckpoint {
        conversation: vec![
            json!({"role":"user","content":"do it"}),
            json!({
                "role":"assistant","content":null,
                "tool_calls":[{"id":"ack-1","type":"function","function":{"name":"SendMessage","arguments":"{}"}}]
            }),
            json!({
                "role":"assistant","content":null,
                "tool_calls":[{"id":"final-1","type":"function","function":{"name":"SendMessage","arguments":"{}"}}]
            }),
            json!({
                "role":"assistant","content":null,
                "tool_calls":[{"id":"read-2","type":"function","function":{"name":"Read","arguments":"{}"}}]
            }),
        ],
        text: String::new(),
        completed_steps: 3,
        tool_calls_completed: 3,
    });
    assert!(!checkpoint_ended_on_silent_tool_calls(&visible, ""));
}

#[test]
fn computer_use_subagent_reuses_the_final_display_space_contract() {
    assert_eq!(COMPUTER_USE_SUBAGENT_TYPE, "computerUse");
    assert!(is_computer_use_subagent_type(Some("Computer-Use")));
    assert!(is_computer_use_subagent_type(Some("computer_use")));
    assert!(!is_computer_use_subagent_type(Some("browserUse")));

    let with_browser = computer_use_subagent_description(true);
    assert!(with_browser.contains("For browser-only work, dispatch browserUse instead"));
    assert!(with_browser.contains("Display is 1280×800."));
    assert!(with_browser.contains("0..1279 × 0..799"));

    let without_browser = computer_use_subagent_description(false);
    assert!(without_browser.contains("browsing, signing in to sites, and using GUI apps"));
    assert!(!without_browser.contains("dispatch browserUse instead"));

    let config = create_sand_computer_use_subagent_config(true);
    assert_eq!(config.subagent_type.r#type.case, "custom");
    assert_eq!(config.subagent_type.r#type.value.name, "computerUse");
    assert!(!config.preserve_task_tool);
    assert_eq!(config.subagent_source, "builtin");
}

#[test]
fn provider_option_struct_exposes_required_runner_reminder_fields() {
    let message = MessageLike {
        role: "user".into(),
        content: MessageContent::Text("internal".into()),
        provider_options: Some(ProviderOptions {
            cursor: Some(CursorProviderOptions {
                sand_early_result_reminder: true,
                ..CursorProviderOptions::default()
            }),
        }),
    };
    assert!(is_injected_reminder_message(&message));
}
