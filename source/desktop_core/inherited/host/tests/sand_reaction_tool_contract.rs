use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::tools::sand_reaction_tool::{
    CountingReactionSink, ReactionDeliveryCounter, ReactionSink, ReactionToolBridge,
    SAND_REACT_TO_MESSAGE_TOOL_NAME, is_message_address, reaction_tool_definition,
};
use serde_json::{Value, json};

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
struct Sink {
    reactions: Mutex<Vec<(String, String)>>,
}

impl ReactionSink for Sink {
    fn react(
        &self,
        message_address: &str,
        emoji: &str,
    ) -> Result<(), ProviderSessionError> {
        self.reactions
            .lock()
            .expect("reactions")
            .push((message_address.to_string(), emoji.to_string()));
        Ok(())
    }
}

#[test]
fn message_address_validation_matches_frozen_message_reference_contract() {
    for valid in ["t0u", "t12u", "t3ua0", "t4a2", "t9s1", "tba0", "tbs7"] {
        assert!(is_message_address(valid), "{valid}");
    }
    for invalid in ["", "u3", "t", "t3", "tbu", "t3u0", "t3uaa", "t3x1", "t3a"] {
        assert!(!is_message_address(invalid), "{invalid}");
    }
}

#[test]
fn reaction_tool_schema_and_bridge_match_frozen_toggle_contract() {
    let definition = reaction_tool_definition();
    assert_eq!(definition.name, SAND_REACT_TO_MESSAGE_TOOL_NAME);
    assert_eq!(
        definition.input_schema["required"],
        json!(["message_address", "emoji"])
    );
    assert_eq!(
        definition.input_schema["properties"]["emoji"]["maxLength"],
        16
    );

    let sink = Arc::new(Sink::default());
    let bridge = ReactionToolBridge::new(Arc::new(Delegate), sink.clone());
    let tools = bridge.list_tools().expect("tools");
    assert_eq!(tools[0].name, SAND_REACT_TO_MESSAGE_TOOL_NAME);
    assert!(tools.iter().any(|tool| tool.name == "mcp.demo"));

    let reacted = bridge
        .call_tool(
            &tools[0],
            json!({"message_address":"  t3u  ","emoji":" 👍 "}),
            "reaction-1",
        )
        .expect("reaction");
    assert_eq!(
        reacted,
        Value::String(
            "Reacted 👍 on t3u. (Reactions toggle: react the same emoji again to take it back.)"
                .into()
        )
    );
    assert_eq!(
        sink.reactions.lock().expect("reactions").as_slice(),
        &[("t3u".into(), "👍".into())]
    );

    let invalid = bridge
        .call_tool(
            &tools[0],
            json!({"message_address":"not-an-address","emoji":"🎉"}),
            "reaction-2",
        )
        .expect("invalid address is a tool result");
    assert!(
        invalid
            .as_str()
            .is_some_and(|value| value.contains("isn't a valid message address"))
    );
    assert_eq!(sink.reactions.lock().expect("reactions").len(), 1);

    let delegated = bridge
        .call_tool(
            tools.iter().find(|tool| tool.name == "mcp.demo").expect("demo"),
            json!({}),
            "reaction-3",
        )
        .expect("delegate");
    assert_eq!(delegated["delegated"], true);
}

#[test]
fn reaction_tool_rejects_missing_or_overlong_emoji_before_transport() {
    let sink = Arc::new(Sink::default());
    let bridge = ReactionToolBridge::new(Arc::new(Delegate), sink.clone());
    let tool = reaction_tool_definition();

    assert!(
        bridge
            .call_tool(
                &tool,
                json!({"message_address":"t1u","emoji":""}),
                "missing",
            )
            .is_err()
    );
    assert!(
        bridge
            .call_tool(
                &tool,
                json!({"message_address":"t1u","emoji":"abcdefghijklmnopq"}),
                "long",
            )
            .is_err()
    );
    assert!(sink.reactions.lock().expect("reactions").is_empty());
}


#[test]
fn successful_reactions_increment_the_per_turn_delivery_counter_only_after_sink_success() {
    let sink = Arc::new(Sink::default());
    let counter = ReactionDeliveryCounter::default();
    let counted: Arc<dyn ReactionSink> =
        Arc::new(CountingReactionSink::new(sink.clone(), counter.clone()));
    assert_eq!(counter.count(), 0);
    counted.react("t1u", "✅").expect("reaction");
    assert_eq!(counter.count(), 1);
    assert_eq!(
        sink.reactions.lock().expect("reactions").as_slice(),
        &[("t1u".into(), "✅".into())]
    );
}
