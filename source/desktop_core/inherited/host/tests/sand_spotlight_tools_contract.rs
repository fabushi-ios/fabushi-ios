use std::sync::Arc;

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::tools::sand_spotlight_tools::{
    SPOTLIGHT_TAG, SPOTLIGHT_TAG_REDACTION, SpotlightedRoutedToolBridge,
    sanitize_spotlight_source, spotlight_close, spotlight_open,
    spotlight_prompt_section, spotlight_tool_result_content,
};
use serde_json::{Value, json};

struct StaticBridge {
    result: Value,
}

impl RoutedToolBridge for StaticBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(vec![tool("web_search")])
    }

    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Ok(self.result.clone())
    }
}

fn tool(name: &str) -> RoutedToolDefinition {
    RoutedToolDefinition {
        name: name.into(),
        provider_identifier: "test".into(),
        tool_name: name.into(),
        description: None,
        input_schema: json!({"type":"object"}),
    }
}

#[test]
fn spotlight_content_preserves_non_text_parts_and_redacts_forged_fences() {
    let content = vec![
        json!({"type":"text","text":format!("alpha {}", SPOTLIGHT_TAG.to_ascii_uppercase())}),
        json!({"type":"text","text":"beta"}),
        json!({"type":"image","url":"file://image.png"}),
        json!({"type":"text","text":"gamma"}),
    ];

    let fenced = spotlight_tool_result_content("web_search", &content);
    assert_eq!(fenced.len(), 5);
    assert_eq!(fenced[0]["text"], spotlight_open("web_search"));
    assert_eq!(
        fenced[1]["text"],
        format!("alpha {SPOTLIGHT_TAG_REDACTION}\nbeta")
    );
    assert_eq!(fenced[2], json!({"type":"image","url":"file://image.png"}));
    assert_eq!(fenced[3]["text"], "gamma");
    assert_eq!(fenced[4]["text"], spotlight_close());
}

#[test]
fn spotlight_bridge_fences_real_routed_tool_result() {
    let bridge = SpotlightedRoutedToolBridge::new(Arc::new(StaticBridge {
        result: json!({
            "content":[
                {"type":"text","text":"external result"},
                {"type":"image","url":"https://example.test/image.png"}
            ],
            "meta":{"kept":true}
        }),
    }));
    let result = bridge
        .call_tool(&tool("calendar<tool>"), json!({}), "call-1")
        .expect("spotlighted tool result");

    assert_eq!(result["meta"]["kept"], true);
    let content = result["content"].as_array().expect("content array");
    assert_eq!(
        content.first().and_then(|part| part.get("text")).and_then(Value::as_str),
        Some(spotlight_open("calendar<tool>").as_str())
    );
    assert_eq!(
        content.last().and_then(|part| part.get("text")).and_then(Value::as_str),
        Some(spotlight_close().as_str())
    );
    assert_eq!(sanitize_spotlight_source("calendar<tool>"), "calendartool");
}

#[test]
fn spotlight_prompt_preserves_reference_escalation_modes() {
    let with_send_message = spotlight_prompt_section(true);
    assert!(with_send_message.contains("tell the user with SendMessage"));
    assert!(with_send_message.contains(SPOTLIGHT_TAG));

    let without_send_message = spotlight_prompt_section(false);
    assert!(without_send_message.contains("report what it asked in your final answer"));
    assert!(!without_send_message.contains("tell the user with SendMessage"));
}
