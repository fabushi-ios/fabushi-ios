use mahayana_host_runtime::cloud_agents::cloud_agent_tool::{
    CLOUD_AGENT_TOOL_IDENTIFIER, CLOUD_AGENT_TOOL_NAME, cloud_agent_tool_definition,
};

#[test]
fn first_party_cloud_agent_definition_exposes_the_frozen_action_surface() {
    let tool = cloud_agent_tool_definition();
    assert_eq!(tool.name, CLOUD_AGENT_TOOL_NAME);
    assert_eq!(tool.tool_name, CLOUD_AGENT_TOOL_NAME);
    assert_eq!(CLOUD_AGENT_TOOL_IDENTIFIER, "CLOUD_AGENT");
    assert_eq!(tool.provider_identifier, "fabushi-runner");
    assert_eq!(tool.input_schema["required"][0], "action");

    let actions = tool.input_schema["properties"]["action"]["enum"]
        .as_array()
        .expect("action enum")
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        actions,
        vec![
            "launch",
            "list",
            "models",
            "get",
            "dump",
            "watch",
            "reply",
            "rename",
            "cancel",
            "archive",
            "unarchive",
            "delete",
            "list_artifacts",
        ]
    );
    assert_eq!(
        tool.input_schema["properties"]["environment"]["properties"]["type"]["enum"],
        serde_json::json!(["cloud", "pool", "machine", "environment"])
    );
    assert_eq!(
        tool.input_schema["properties"]["confirm"]["type"],
        "boolean"
    );
}
