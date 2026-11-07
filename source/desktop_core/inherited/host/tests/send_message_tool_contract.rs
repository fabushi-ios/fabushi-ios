use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::attachment_paths::{
    AgentMediaKind, persist_agent_media_bytes,
};
use mahayana_host_runtime::runner::tools::send_message_encoding::{
    create_send_message_tool_call, encode_markdown_image_destination, encode_send_message,
    encode_send_message_wire, encode_text_content, image_mime_from_path,
    resolve_box_media_attachment,
};
use mahayana_host_runtime::runner::tools::send_message_schema::{
    parse_send_message_input, refine_send_message, send_message_input_schema,
    validate_send_message,
};
use mahayana_host_runtime::runner::tools::send_message_tool::{
    CountingSendMessageSink, ResolvedAttachmentSource, SAND_AWAITING_USER_SEND_MESSAGE_BLOCKED,
    SAND_SEND_MESSAGE_TOOL_NAME, SendMessageDeliveryCounter, SendMessageInteractionSink,
    SendMessageSink, SendMessageToolBridge,
};
use serde_json::{Value, json};
use prost::Message as _;
use mahayana_host_runtime::runner::agent_v1_wire::{
    AgentToolCall, SendMessageToolCall, agent_tool_call, send_message_result,
};

struct Delegate;

impl RoutedToolBridge for Delegate {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(vec![RoutedToolDefinition {
            name: "mcp.demo".into(),
            provider_identifier: "demo".into(),
            tool_name: "demo".into(),
            description: None,
            input_schema: json!({"type":"object"}),
        }])
    }

    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Ok(json!({"delegated":true}))
    }
}


#[derive(Default)]
struct RecordingSendMessageInteractionSink {
    calls: Mutex<Vec<(&'static str, AgentToolCall)>>,
}

impl SendMessageInteractionSink for RecordingSendMessageInteractionSink {
    fn on_tool_call(&self, phase: &'static str, tool_call: &AgentToolCall) {
        self.calls
            .lock()
            .expect("interaction calls")
            .push((phase, tool_call.clone()));
    }
}

#[derive(Default)]
struct Sink {
    messages: Mutex<Vec<Value>>,
}

impl SendMessageSink for Sink {
    fn send_message(
        &self,
        message: Value,
        _timestamp_ms: u64,
        tool_call_id: &str,
    ) -> Result<Option<String>, ProviderSessionError> {
        self.messages.lock().expect("messages").push(message);
        Ok(Some(format!("runner-send:{tool_call_id}")))
    }
}

struct AwaitingSink;

impl SendMessageSink for AwaitingSink {
    fn is_awaiting_user_selection(&self) -> bool {
        true
    }

    fn send_message(
        &self,
        _message: Value,
        _timestamp_ms: u64,
        _tool_call_id: &str,
    ) -> Result<Option<String>, ProviderSessionError> {
        panic!("awaiting-user guard must reject before persistence")
    }
}

#[test]
fn send_message_guard_rejects_when_shipping_state_is_waiting_on_user() {
    let bridge = SendMessageToolBridge::new(Arc::new(Delegate), Arc::new(AwaitingSink));
    let tool = bridge.list_tools().expect("tools").remove(0);
    let error = bridge
        .call_tool(
            &tool,
            json!({"type":"text","content":"must wait"}),
            "awaiting-call",
        )
        .expect_err("awaiting-user send must be blocked");
    match error {
        ProviderSessionError::Tool(message) => {
            assert_eq!(message, SAND_AWAITING_USER_SEND_MESSAGE_BLOCKED);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn schema_matches_frozen_type_fences_and_attachment_schemes() {
    let valid = parse_send_message_input(&json!({
        "type":"text",
        "content":" hello ",
        "images":[{"url":"file:///tmp/a.png","alt":" a "}]
    }))
    .expect("parse");
    assert!(validate_send_message(&valid).is_ok());
    assert_eq!(valid.content.as_deref(), Some("hello"));

    let invalid = parse_send_message_input(&json!({
        "type":"widget",
        "content":"must not ride widget",
        "widget":{"prompt":"Choose","options":[]},
        "channel":"slack:1"
    }))
    .expect("parse");
    let issues = refine_send_message(&invalid);
    assert!(issues.iter().any(|issue| issue.path == vec!["content".to_string()]));
    assert!(issues.iter().any(|issue| issue.path == vec!["channel".to_string()]));

    let attachment = parse_send_message_input(&json!({
        "type":"attachment",
        "url":"ftp://example.test/file"
    }))
    .expect("parse");
    assert!(validate_send_message(&attachment).is_err());
}

#[test]
fn widget_schema_matches_frozen_choice_contract_and_normalization() {
    let valid = parse_send_message_input(&json!({
        "type":"widget",
        "widget":{
            "prompt":"  Which environment?  ",
            "helpText":"  Pick one  ",
            "options":[
                {
                    "label":"  Production  ",
                    "value":"  prod  ",
                    "description":"  User-facing production  ",
                    "style":"danger",
                    "ignored":"strip me"
                },
                {"label":"  Staging  ","style":"primary"}
            ],
            "allowCustom":true,
            "dismissOnMoveOn":false,
            "ignored":"strip me"
        }
    }))
    .expect("parse widget");
    assert!(validate_send_message(&valid).is_ok());
    let widget = valid.widget.expect("normalized widget");
    assert_eq!(widget["prompt"], "Which environment?");
    assert_eq!(widget["helpText"], "Pick one");
    assert_eq!(widget["options"][0]["label"], "Production");
    assert_eq!(widget["options"][0]["value"], "prod");
    assert!(widget.get("ignored").is_none());
    assert!(widget["options"][0].get("ignored").is_none());

    let invalid = parse_send_message_input(&json!({
        "type":"widget",
        "widget":{
            "prompt":" ",
            "options":[
                {"label":" ","value":" ","style":"loud"}
            ],
            "allowCustom":"yes"
        }
    }))
    .expect("parse invalid widget");
    let issues = validate_send_message(&invalid).expect_err("invalid widget rejected");
    assert!(issues.iter().any(|issue| issue.path == vec!["widget","prompt"]));
    assert!(issues.iter().any(|issue| issue.path == vec!["widget","options","0","label"]));
    assert!(issues.iter().any(|issue| issue.path == vec!["widget","options","0","value"]));
    assert!(issues.iter().any(|issue| issue.path == vec!["widget","options","0","style"]));
    assert!(issues.iter().any(|issue| issue.path == vec!["widget","allowCustom"]));

    let too_many = parse_send_message_input(&json!({
        "type":"widget",
        "widget":{
            "prompt":"Choose",
            "options":[
                {"label":"1"},{"label":"2"},{"label":"3"},{"label":"4"},
                {"label":"5"},{"label":"6"},{"label":"7"}
            ]
        }
    }))
    .expect("parse oversized widget");
    assert!(validate_send_message(&too_many).expect_err("max six options")
        .iter()
        .any(|issue| issue.path == vec!["widget","options"]));
}

#[test]
fn generated_send_message_schema_exposes_frozen_widget_constraints() {
    let schema = send_message_input_schema();
    let widget = &schema["properties"]["widget"];
    assert_eq!(widget["required"], json!(["prompt","options"]));
    assert_eq!(widget["additionalProperties"], false);
    assert_eq!(widget["properties"]["options"]["minItems"], 1);
    assert_eq!(widget["properties"]["options"]["maxItems"], 6);
    assert_eq!(
        widget["properties"]["options"]["items"]["properties"]["style"]["enum"],
        json!(["default","primary","danger"])
    );
    assert_eq!(widget["properties"]["allowCustom"]["type"], "boolean");
    assert_eq!(widget["properties"]["dismissOnMoveOn"]["type"], "boolean");
}

#[test]
fn encoding_preserves_frozen_text_markdown_and_summary_projection() {
    assert_eq!(
        encode_markdown_image_destination("https://example.test/a<1>\n"),
        "<https://example.test/a%3C1%3E%0A>"
    );
    let images = vec![json!({"url":"https://x.test/a.png","alt":"[cat]\nphoto"})];
    assert_eq!(
        encode_text_content("hello", Some(images.as_slice())),
        "hello\n\n![cat  photo](<https://x.test/a.png>)"
    );
    assert_eq!(
        encode_send_message(&json!({
            "type":"connector",
            "variant":"connected",
            "connector":"Slack"
        }))
        .expect("encode")["message"]["value"]["content"],
        "Confirmed the Slack connector is connected"
    );
    assert_eq!(
        encode_send_message(&json!({
            "type":"permission-request",
            "permission":{"title":"Files","reason":"Read a report"}
        }))
        .expect("encode")["message"]["value"]["content"],
        "Legacy permission request (no longer actionable): Files — Read a report"
    );
}

#[test]
fn send_message_uses_frozen_agent_v1_wire_types_and_tool_call_tag() {
    let args = encode_send_message_wire(&json!({
        "type":"text",
        "content":"hello"
    }))
    .expect("wire encode");

    assert_eq!(
        args.encode_to_vec(),
        vec![0x0a, 0x07, 0x0a, 0x05, b'h', b'e', b'l', b'l', b'o']
    );

    let wrapped = create_send_message_tool_call(SendMessageToolCall {
        args: Some(args.clone()),
        result: None,
    });
    let bytes = wrapped.encode_to_vec();

    assert_eq!(
        &bytes[..2],
        &[0xba, 0x03],
        "agent.v1.ToolCall send_message_tool_call must use frozen tag 55"
    );

    let decoded = AgentToolCall::decode(bytes.as_slice()).expect("decode frozen wire");
    match decoded.tool {
        Some(agent_tool_call::Tool::SendMessageToolCall(call)) => {
            assert_eq!(call.args, Some(args));
            assert!(call.result.is_none());
        }
        _ => panic!("unexpected tool-call oneof"),
    }
}

#[test]
fn send_message_bridge_is_first_party_and_delegates_other_tools() {
    let sink = Arc::new(Sink::default());
    let bridge = SendMessageToolBridge::new(Arc::new(Delegate), sink.clone());
    let tools = bridge.list_tools().expect("tools");
    assert_eq!(tools[0].name, SAND_SEND_MESSAGE_TOOL_NAME);
    assert!(tools.iter().any(|tool| tool.name == "mcp.demo"));

    let result = bridge.call_tool(
        &tools[0],
        json!({
            "type":"text",
            "content":"hello",
            "images":[{"url":"https://example.test/a.png","alt":"a"}]
        }),
        "tool-1",
    )
    .expect("send");
    assert_eq!(result["sent"], true);
    assert_eq!(result["messageId"], "runner-send:tool-1");
    assert_eq!(
        sink.messages.lock().expect("messages")[0]["content"],
        "hello"
    );

    let delegated = bridge
        .call_tool(
            tools.iter().find(|tool| tool.name == "mcp.demo").expect("demo"),
            json!({}),
            "tool-2",
        )
        .expect("delegate");
    assert_eq!(delegated["delegated"], true);
}


#[derive(Default)]
struct ResolvingSink {
    messages: Mutex<Vec<Value>>,
    sources: Mutex<Vec<String>>,
}

impl SendMessageSink for ResolvingSink {
    fn resolve_cloud_agent_title(&self, bc_id: &str) -> Option<String> {
        (bc_id == "bc-title").then(|| "  Release helper  ".to_string())
    }

    fn read_media_dimensions(&self, resolved_url: &str) -> Option<(u32, u32)> {
        resolved_url.ends_with(".png").then_some((640, 480))
    }

    fn resolve_attachment_source(
        &self,
        source_url: &str,
        _tool_call_id: &str,
    ) -> Result<ResolvedAttachmentSource, ProviderSessionError> {
        self.sources.lock().expect("sources").push(source_url.to_string());
        let file_name = source_url
            .rsplit('/')
            .next()
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        Ok(ResolvedAttachmentSource {
            url: source_url.replace("file:///workspace/", "file:///persisted/"),
            file_name,
        })
    }

    fn send_message(
        &self,
        message: Value,
        _timestamp_ms: u64,
        tool_call_id: &str,
    ) -> Result<Option<String>, ProviderSessionError> {
        self.messages.lock().expect("messages").push(message);
        Ok(Some(format!("runner-send:{tool_call_id}")))
    }
}

#[test]
fn cursor_agent_message_resolves_shipping_title_fail_soft() {
    let sink = Arc::new(ResolvingSink::default());
    let bridge = SendMessageToolBridge::new(Arc::new(Delegate), sink.clone());
    let tool = bridge.list_tools().expect("tools").remove(0);

    bridge.call_tool(
        &tool,
        json!({"type":"cursor-agent","bcId":"bc-title"}),
        "cloud-title",
    ).expect("title lookup send");
    bridge.call_tool(
        &tool,
        json!({"type":"cursor-agent","bcId":"bc-missing"}),
        "cloud-missing",
    ).expect("missing title is fail-soft");

    let messages = sink.messages.lock().expect("messages");
    assert_eq!(messages[0]["title"], "Release helper");
    assert!(messages[1].get("title").is_none());
}

#[test]
fn send_message_interaction_emits_frozen_initial_and_completed_agent_v1_wire() {
    let sink = Arc::new(Sink::default());
    let interactions = Arc::new(RecordingSendMessageInteractionSink::default());
    let bridge = SendMessageToolBridge::new(Arc::new(Delegate), sink)
        .with_interaction_sink(interactions.clone());
    let tool = bridge.list_tools().expect("tools").remove(0);

    bridge.call_tool(
        &tool,
        json!({"type":"text","content":"hello"}),
        "send-wire-1",
    ).expect("send");

    let calls = interactions.calls.lock().expect("interaction calls");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "initial");
    assert_eq!(calls[1].0, "completed");
    assert_eq!(calls[0].1.tool_call_id.as_deref(), Some("send-wire-1"));
    assert!(calls[0].1.started_at_ms.is_some());
    assert!(calls[0].1.completed_at_ms.is_none());
    assert_eq!(calls[1].1.tool_call_id.as_deref(), Some("send-wire-1"));
    assert_eq!(calls[1].1.started_at_ms, calls[0].1.started_at_ms);
    assert!(calls[1].1.completed_at_ms.is_some());

    match calls[0].1.tool.as_ref() {
        Some(agent_tool_call::Tool::SendMessageToolCall(call)) => {
            assert!(call.args.is_some());
            assert!(call.result.is_none());
        }
        _ => panic!("initial must be frozen SendMessage ToolCall"),
    }
    match calls[1].1.tool.as_ref() {
        Some(agent_tool_call::Tool::SendMessageToolCall(call)) => {
            assert!(call.args.is_some());
            match call.result.as_ref().and_then(|value| value.result.as_ref()) {
                Some(send_message_result::Result::Success(success)) => {
                    assert_eq!(success.message_id, "runner-send:send-wire-1");
                    assert!(success.timestamp > 0);
                }
                _ => panic!("completed must carry SendMessage success"),
            }
        }
        _ => panic!("completed must be frozen SendMessage ToolCall"),
    }
}

#[test]
fn awaiting_user_interaction_completes_with_frozen_error_result_before_rejecting() {
    let interactions = Arc::new(RecordingSendMessageInteractionSink::default());
    let bridge = SendMessageToolBridge::new(Arc::new(Delegate), Arc::new(AwaitingSink))
        .with_interaction_sink(interactions.clone());
    let tool = bridge.list_tools().expect("tools").remove(0);

    let error = bridge.call_tool(
        &tool,
        json!({"type":"text","content":"must wait"}),
        "awaiting-wire",
    ).expect_err("awaiting user rejects send");
    assert!(matches!(error, ProviderSessionError::Tool(_)));

    let calls = interactions.calls.lock().expect("interaction calls");
    assert_eq!(calls.len(), 2);
    match calls[1].1.tool.as_ref() {
        Some(agent_tool_call::Tool::SendMessageToolCall(call)) => {
            match call.result.as_ref().and_then(|value| value.result.as_ref()) {
                Some(send_message_result::Result::Error(error)) => {
                    assert_eq!(error.error, SAND_AWAITING_USER_SEND_MESSAGE_BLOCKED);
                }
                _ => panic!("awaiting completion must carry SendMessage error"),
            }
        }
        _ => panic!("awaiting completion must be frozen SendMessage ToolCall"),
    }
}

#[test]
fn box_media_resolution_matches_frozen_root_desktop_and_image_fences() {
    assert_eq!(image_mime_from_path("/workspace/a.PNG"), Some("image/png"));
    assert_eq!(image_mime_from_path("/workspace/a.heic"), None);
    assert_eq!(
        resolve_box_media_attachment(
            "/workspace/a.png",
            true,
            |_| Some(vec![1, 2, 3]),
            |bytes, mime| Some(format!("image:{mime}:{}", bytes.len())),
            |name, bytes| Some(format!("media:{name}:{}", bytes.len())),
        ),
        Some("image:image/png:3".to_string())
    );
    assert_eq!(
        resolve_box_media_attachment(
            "/workspace/movie.mp4",
            true,
            |_| Some(vec![1, 2]),
            |_bytes, _mime| Some("wrong".to_string()),
            |name, bytes| Some(format!("media:{name}:{}", bytes.len())),
        ),
        Some("media:movie.mp4:2".to_string())
    );
    assert!(resolve_box_media_attachment(
        "/tmp/not-box.png",
        true,
        |_| Some(vec![1]),
        |_bytes, _mime| Some("image".into()),
        |_name, _bytes| Some("media".into()),
    ).is_none());
    assert!(resolve_box_media_attachment(
        "/workspace/a.png",
        false,
        |_| Some(vec![1]),
        |_bytes, _mime| Some("image".into()),
        |_name, _bytes| Some("media".into()),
    ).is_none());
}

#[test]
fn send_message_bridge_resolves_text_images_and_standalone_attachments_before_persisting() {
    let sink = Arc::new(ResolvingSink::default());
    let bridge = SendMessageToolBridge::new(Arc::new(Delegate), sink.clone());
    let tool = bridge.list_tools().expect("tools").remove(0);

    bridge.call_tool(
        &tool,
        json!({
            "type":"text",
            "content":"look",
            "images":[{"url":"file:///workspace/cat.png","alt":"cat"}]
        }),
        "image-call",
    ).expect("image message");
    bridge.call_tool(
        &tool,
        json!({
            "type":"attachment",
            "url":"file:///workspace/report.pdf",
            "alt":"report"
        }),
        "attachment-call",
    ).expect("attachment message");
    bridge.call_tool(
        &tool,
        json!({
            "type":"attachment",
            "url":"file:///workspace/standalone.png",
            "alt":"standalone"
        }),
        "image-attachment-call",
    ).expect("standalone image attachment");

    let messages = sink.messages.lock().expect("messages");
    assert_eq!(messages[0]["images"][0]["url"], "file:///persisted/cat.png");
    assert_eq!(messages[0]["images"][0]["width"], 640);
    assert_eq!(messages[0]["images"][0]["height"], 480);
    assert_eq!(messages[1]["url"], "file:///persisted/report.pdf");
    assert_eq!(messages[1]["file_name"], "report.pdf");
    assert!(messages[1].get("width").is_none());
    assert_eq!(messages[2]["url"], "file:///persisted/standalone.png");
    assert_eq!(messages[2]["width"], 640);
    assert_eq!(messages[2]["height"], 480);
    assert_eq!(
        sink.sources.lock().expect("sources").as_slice(),
        &[
            "file:///workspace/cat.png".to_string(),
            "file:///workspace/report.pdf".to_string(),
            "file:///workspace/standalone.png".to_string()
        ]
    );
}

#[test]
fn agent_media_persistence_is_atomic_and_separates_images_from_attachments() {
    let root = std::env::temp_dir().join(format!(
        "fabushi-send-message-media-{}",
        uuid::Uuid::new_v4()
    ));
    let image = persist_agent_media_bytes(
        &root,
        "/workspace/folder/cat.png",
        b"png-bytes",
        AgentMediaKind::Image,
    ).expect("persist image");
    let file = persist_agent_media_bytes(
        &root,
        "/workspace/folder/report.pdf",
        b"pdf-bytes",
        AgentMediaKind::Attachment,
    ).expect("persist attachment");
    assert_eq!(image.parent().and_then(|value| value.file_name()).and_then(|value| value.to_str()), Some("assets"));
    assert_eq!(file.parent().and_then(|value| value.file_name()).and_then(|value| value.to_str()), Some("attachments"));
    assert!(image.file_name().and_then(|value| value.to_str()).is_some_and(|value| value.ends_with("-cat.png")));
    assert!(file.file_name().and_then(|value| value.to_str()).is_some_and(|value| value.ends_with("-report.pdf")));
    assert_eq!(std::fs::read(&image).expect("image bytes"), b"png-bytes");
    assert_eq!(std::fs::read(&file).expect("file bytes"), b"pdf-bytes");
    assert!(!root.join("assets").read_dir().expect("assets").any(|entry| {
        entry.ok().and_then(|entry| entry.file_name().into_string().ok()).is_some_and(|name| name.ends_with(".tmp"))
    }));
    let _ = std::fs::remove_dir_all(root);
}


struct FailingSink;

impl SendMessageSink for FailingSink {
    fn send_message(
        &self,
        _message: Value,
        _timestamp_ms: u64,
        _tool_call_id: &str,
    ) -> Result<Option<String>, ProviderSessionError> {
        Err(ProviderSessionError::Tool("persist failed".into()))
    }
}

#[test]
fn delivery_counter_tracks_only_successful_shipping_send_side_effects() {
    let counter = SendMessageDeliveryCounter::default();
    let success = CountingSendMessageSink::new(Arc::new(Sink::default()), counter.clone());
    success
        .send_message(json!({"type":"widget","widget":{"prompt":"Choose"}}), 1, "ok-1")
        .expect("first successful send");
    success
        .send_message(json!({"type":"attachment","url":"file:///tmp/a"}), 2, "ok-2")
        .expect("second successful send");
    assert_eq!(counter.count(), 2);

    let failure = CountingSendMessageSink::new(Arc::new(FailingSink), counter.clone());
    assert!(failure
        .send_message(json!({"type":"text","content":"not persisted"}), 3, "bad")
        .is_err());
    assert_eq!(counter.count(), 2, "failed persistence must not count as delivered");
}

#[test]
fn shipping_send_message_composes_cloud_title_and_generated_interaction_observation() {
    const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");
    const TURN_COMPOSITION: &str = include_str!("../src/runner/turn_agent_composition.rs");
    const TURN_TOOLSET: &str = include_str!("../src/runner/tools/turn_toolset.rs");

    assert!(SHIPPING_HOST.contains("cloud_agents: Arc<SandCloudAgentManager>"));
    assert!(SHIPPING_HOST.contains("fn resolve_cloud_agent_title(&self, bc_id: &str)"));
    assert!(SHIPPING_HOST.contains("detail.summary.name.trim().to_string()"));
    assert!(SHIPPING_HOST.contains("cloud_agents: Arc::clone(&worker_cloud_agents)"));
    assert!(SHIPPING_HOST.contains("\"deliverToChannel\""));

    assert!(TURN_COMPOSITION.contains("TurnObservationSendMessageSink::new"));
    assert!(TURN_COMPOSITION.contains("send_message_interaction_sink,"));
    assert!(TURN_TOOLSET.contains("send_message.with_interaction_sink")
        || TURN_TOOLSET.contains("send_bridge.with_interaction_sink"));
}
