use mahayana_host_runtime::extensions::mcp::coordinator_relay::{
    CoordinatorMcpLifecycleRelay, CoordinatorMcpLifecycleRelayError,
    CoordinatorPluginSkillsLoader, HOST_MCP_LIFECYCLE_REQUEST_EVENT_CHANNEL,
    MCP_LIST_CATALOG_METHOD, MCP_LOAD_PLUGIN_SKILLS_METHOD,
    MCP_RESOLVE_LIFECYCLE_GATEWAY_METHOD, decode_effective_plugins,
    decode_mcp_catalog, decode_mcp_server_state,
};
use mahayana_host_runtime::extensions::mcp::plugin_skills::PluginSkillsLoader;
use mahayana_host_runtime::gateway_server::GatewayEventHub;
use serde_json::json;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[test]
fn host_mcp_lifecycle_relay_round_trips_through_gateway_event_correlation() {
    let events = GatewayEventHub::default();
    let receiver = events.subscribe();
    let relay = Arc::new(CoordinatorMcpLifecycleRelay::with_timeout(
        events.clone(),
        Duration::from_secs(2),
    ));
    let worker = Arc::clone(&relay);
    let handle = thread::spawn(move || {
        worker.request(MCP_LIST_CATALOG_METHOD, json!({"forceRefresh":true}))
    });

    let event = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("lifecycle request event");
    assert_eq!(event["channel"], HOST_MCP_LIFECYCLE_REQUEST_EVENT_CHANNEL);
    assert_eq!(event["payload"]["method"], MCP_LIST_CATALOG_METHOD);
    assert_eq!(event["payload"]["args"]["forceRefresh"], true);
    let request_id = event["payload"]["requestId"]
        .as_str()
        .expect("request id")
        .to_string();

    let settlement = relay
        .resolve(&json!({
            "requestId": request_id,
            "ok": true,
            "result": [{"id":"plugin-1"}],
        }))
        .expect("settle request");
    assert_eq!(settlement["resolved"], true);
    assert_eq!(
        handle
            .join()
            .expect("relay worker")
            .expect("relay result")[0]["id"],
        "plugin-1"
    );
}

#[test]
fn host_mcp_lifecycle_relay_rejects_unapproved_methods_and_remote_failures() {
    let denied = CoordinatorMcpLifecycleRelay::with_timeout(
        GatewayEventHub::default(),
        Duration::from_millis(25),
    );
    assert!(matches!(
        denied.request("spawnLocalExecDaemon", json!({})),
        Err(CoordinatorMcpLifecycleRelayError::UnsupportedMethod(_))
    ));

    let events = GatewayEventHub::default();
    let receiver = events.subscribe();
    let relay = Arc::new(CoordinatorMcpLifecycleRelay::with_timeout(
        events,
        Duration::from_secs(2),
    ));
    let worker = Arc::clone(&relay);
    let handle = thread::spawn(move || worker.request(MCP_LIST_CATALOG_METHOD, json!({})));
    let event = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("request event");
    let request_id = event["payload"]["requestId"]
        .as_str()
        .expect("request id")
        .to_string();
    relay
        .resolve(&json!({
            "requestId": request_id,
            "ok": false,
            "error": "owner offline",
        }))
        .expect("settle failed request");
    assert!(matches!(
        handle.join().expect("relay worker"),
        Err(CoordinatorMcpLifecycleRelayError::Remote { message, .. }) if message == "owner offline"
    ));
    assert_eq!(
        MCP_RESOLVE_LIFECYCLE_GATEWAY_METHOD,
        "mcp.resolveLifecycleRequest"
    );
}


#[test]
fn coordinator_owner_snapshots_decode_into_the_rust_mcp_service_contract() {
    let servers = decode_mcp_server_state(json!({
        "servers": [{
            "id":"server-1",
            "name":"Calendar",
            "serverIdentifier":"calendar",
            "accountKey":"default",
            "pluginId":"plugin-1",
            "isTeamServer":false,
            "status":"connected",
            "transport":"stdio",
            "toolCount":2,
            "disabledToolCount":1,
            "customInstructions":"be concise"
        }]
    }))
    .expect("server snapshot");
    assert_eq!(servers[0].server_identifier, "calendar");
    assert_eq!(servers[0].disabled_tool_count, Some(1));

    let catalog = decode_mcp_catalog(json!([{
        "id":"plugin-1",
        "name":"calendar",
        "displayName":"Calendar",
        "description":"calendar tools",
        "category":"productivity",
        "connectors":[{"name":"calendar"}],
        "fields":[{"key":"token","label":"Token","hint":"secret","isRequired":true,"isSecret":true}],
        "skills":[{"name":"Schedule","description":"schedule events","sourceUrl":"https://github.com/o/r/blob/main/skills/Schedule/SKILL.md"}]
    }]))
    .expect("catalog snapshot");
    assert_eq!(catalog[0].connector_count, 1);
    assert_eq!(catalog[0].fields[0].key, "token");
    assert_eq!(catalog[0].skills[0].name, "Schedule");

    let effective = decode_effective_plugins(json!([{
        "pluginId":"plugin-1",
        "installMode":"team-required",
        "isEnabled":true,
        "hasTeamConfiguredVariables":true
    }]))
    .expect("effective plugins snapshot");
    assert!(effective[0].is_enabled);
    assert!(effective[0].has_team_configured_variables);
}


#[test]
fn plugin_skills_loader_uses_the_same_correlated_coordinator_relay() {
    let events = GatewayEventHub::default();
    let receiver = events.subscribe();
    let relay = Arc::new(CoordinatorMcpLifecycleRelay::with_timeout(
        events,
        Duration::from_secs(2),
    ));
    let loader = CoordinatorPluginSkillsLoader::new(Arc::clone(&relay));
    let handle = thread::spawn(move || loader.load());

    let event = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("plugin skills request event");
    assert_eq!(event["payload"]["method"], MCP_LOAD_PLUGIN_SKILLS_METHOD);
    let request_id = event["payload"]["requestId"]
        .as_str()
        .expect("request id")
        .to_string();
    relay
        .resolve(&json!({
            "requestId": request_id,
            "ok": true,
            "result": {
                "plugins": [{
                    "identifier": {
                        "source": "cursor-first-party",
                        "name": "calendar",
                        "pluginDbId": "41",
                        "version": "abc123"
                    },
                    "displayName": "Calendar",
                    "installPath": "/tmp/plugins/calendar",
                    "skills": [{
                        "name": "Schedule",
                        "description": "schedule events",
                        "path": "skills/Schedule/SKILL.md"
                    }]
                }],
                "authBlocked": [],
                "listedPluginIds": ["41"],
                "listedCacheKeys": [{"marketplaceSlug":"cursor","pluginId":"calendar"}],
                "publisherFacts": {"41":{"publisherUserId":7,"marketplaceTeamId":9}},
                "currentUserId": 11
            }
        }))
        .expect("settle plugin skills load");

    let loaded = handle
        .join()
        .expect("loader worker")
        .expect("plugin skills payload");
    assert_eq!(loaded.plugins.len(), 1);
    assert_eq!(loaded.plugins[0].identifier.plugin_db_id.as_deref(), Some("41"));
    assert_eq!(loaded.plugins[0].skills[0].name.as_deref(), Some("Schedule"));
    assert_eq!(loaded.current_user_id, Some(11));
    assert_eq!(loaded.publisher_facts["41"].marketplace_team_id, Some(9));
}
