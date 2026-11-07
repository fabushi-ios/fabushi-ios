const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");
const CLOUD_AGENT_TOOL: &str = include_str!("../src/cloud_agents/cloud_agent_tool.rs");

fn compact_source(source: &str) -> String {
    source.chars().filter(|value| !value.is_whitespace()).collect()
}

#[test]
fn cloud_agent_watch_preserves_explicit_and_automation_quiet_origin() {
    let compact = compact_source(SHIPPING_HOST);
    assert!(compact.contains("args.get(\"quietOrigin\")"));
    assert!(compact.contains("args.get(\"automationWake\")"));
    assert!(compact.contains("\"automation\":{"));
    assert!(compact.contains("quiet_origin:cloud_agent_quiet_origin"));
    assert!(compact.contains("quiet_origin:Option<serde_json::Value>"));
    assert!(compact.contains("CloudAgentWatchOptions::new(quiet_origin,after_followup)"));
    assert!(
        !compact.contains("cloud_watch_quiet_origin=cloud_agent_quiet_origin.clone()"),
        "shipping watch hook must not capture quiet-origin state outside the tool dependency"
    );
}

#[test]
fn cloud_agent_tool_passes_the_per_turn_quiet_origin_explicitly_to_every_watch_arm() {
    let compact = compact_source(CLOUD_AGENT_TOOL);
    assert!(compact.contains("pubquiet_origin:Option<Value>"));
    assert!(compact.contains("Fn(&str,bool,Option<Value>)"));
    assert_eq!(
        compact.matches("deps.quiet_origin.clone()").count(),
        3,
        "launch, explicit watch, and reply must all forward the per-turn quiet origin"
    );
}

#[test]
fn silence_allowed_turns_carry_the_generic_quiet_origin_marker() {
    let compact = compact_source(SHIPPING_HOST);
    assert!(compact.contains("args.get(\"isSilenceAllowed\")"));
    assert!(compact.contains(".filter(|allowed|*allowed)"));
    assert!(compact.contains(".map(|_|serde_json::json!({}))"));
}
