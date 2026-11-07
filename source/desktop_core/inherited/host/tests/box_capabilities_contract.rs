use std::io;

use mahayana_host_runtime::r#box::box_capabilities::{
    BoxCapabilityCallError, CapableBox, box_agent_window_index, box_apply_environment,
    box_description, box_is_available, box_is_preparing, box_load_mcp_servers,
    box_max_windows, box_mcp_resource_accessor, box_supports_multi_window,
    box_terminals_folder,
};

#[derive(Default)]
struct MinimalBox;

impl CapableBox<String> for MinimalBox {
    type Description = String;
    type EnvironmentUpdate = String;
    type McpLoadResult = Vec<String>;
    type McpAccessor = String;
    type Error = io::Error;
}

struct FullBox {
    applied: Vec<String>,
    mcp_configs: Vec<String>,
}

impl CapableBox<String> for FullBox {
    type Description = String;
    type EnvironmentUpdate = String;
    type McpLoadResult = Vec<String>;
    type McpAccessor = String;
    type Error = io::Error;

    fn max_windows(&self) -> Option<u32> {
        Some(8)
    }

    fn agent_window_index(&self, agent_id: &str) -> Option<u32> {
        (agent_id == "agent-7").then_some(3)
    }

    fn terminals_folder(&self) -> Option<String> {
        Some("/terminals".into())
    }

    fn is_available(&self) -> Option<Result<bool, Self::Error>> {
        Some(Ok(false))
    }

    fn is_preparing(&self, agent_id: &str) -> Option<bool> {
        Some(agent_id == "agent-7")
    }

    fn description(&self) -> Option<Self::Description> {
        Some("loopback".into())
    }

    fn apply_environment(
        &mut self,
        _ctx: &String,
        update: Self::EnvironmentUpdate,
    ) -> Option<Result<(), Self::Error>> {
        self.applied.push(update);
        Some(Ok(()))
    }

    fn load_mcp_servers(
        &mut self,
        _ctx: &String,
        config_json: &str,
    ) -> Option<Result<Self::McpLoadResult, Self::Error>> {
        self.mcp_configs.push(config_json.into());
        Some(Ok(vec!["filesystem".into()]))
    }

    fn mcp_resource_accessor(
        &mut self,
        _ctx: &String,
    ) -> Option<Result<Self::McpAccessor, Self::Error>> {
        Some(Ok("mcp-accessor".into()))
    }
}

#[test]
fn box_capabilities_preserve_grok_defaults_and_fail_closed_for_optional_features() {
    let mut box_ = MinimalBox;
    assert_eq!(box_max_windows::<String, _>(&box_), 1);
    assert!(!box_supports_multi_window::<String, _>(&box_));
    assert_eq!(box_agent_window_index::<String, _>(&box_, "agent"), None);
    assert_eq!(box_terminals_folder::<String, _>(&box_), None);
    assert!(box_is_available::<String, _>(&box_).expect("default available"));
    assert!(!box_is_preparing::<String, _>(&box_, "agent"));
    assert_eq!(box_description::<String, _>(&box_), None);

    assert!(matches!(
        box_apply_environment(&mut box_, &"ctx".into(), "A=1".into()),
        Err(BoxCapabilityCallError::EnvironmentUnsupported(_))
    ));
    assert!(matches!(
        box_load_mcp_servers(&mut box_, &"ctx".into(), "{}"),
        Err(BoxCapabilityCallError::McpUnsupported(_))
    ));
    assert!(matches!(
        box_mcp_resource_accessor(&mut box_, &"ctx".into()),
        Err(BoxCapabilityCallError::McpUnsupported(_))
    ));
}

#[test]
fn box_capabilities_forward_present_box_features_without_fallbacks() {
    let mut box_ = FullBox {
        applied: Vec::new(),
        mcp_configs: Vec::new(),
    };
    assert_eq!(box_max_windows::<String, _>(&box_), 8);
    assert!(box_supports_multi_window::<String, _>(&box_));
    assert_eq!(
        box_agent_window_index::<String, _>(&box_, "agent-7"),
        Some(3)
    );
    assert_eq!(
        box_terminals_folder::<String, _>(&box_).as_deref(),
        Some("/terminals")
    );
    assert!(!box_is_available::<String, _>(&box_).expect("explicit availability"));
    assert!(box_is_preparing::<String, _>(&box_, "agent-7"));
    assert_eq!(
        box_description::<String, _>(&box_).as_deref(),
        Some("loopback")
    );

    box_apply_environment(&mut box_, &"ctx".into(), "A=1".into())
        .expect("environment forwarding");
    assert_eq!(box_.applied, vec!["A=1"]);

    assert_eq!(
        box_load_mcp_servers(&mut box_, &"ctx".into(), r#"{"mcp":true}"#)
            .expect("MCP forwarding"),
        vec!["filesystem"]
    );
    assert_eq!(box_.mcp_configs, vec![r#"{"mcp":true}"#]);

    assert_eq!(
        box_mcp_resource_accessor(&mut box_, &"ctx".into())
            .expect("MCP accessor"),
        "mcp-accessor"
    );
}
