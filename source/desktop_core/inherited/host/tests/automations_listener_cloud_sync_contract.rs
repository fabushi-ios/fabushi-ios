use mahayana_host_runtime::automations::automation_trigger::parse_stored_trigger;
use mahayana_host_runtime::extensions::automations::listener_integrations::{
    ListenerIntegrations, count_listener_platforms, filter_listener_agent_channels,
    listener_agent_channels_view,
};
use mahayana_host_runtime::extensions::automations::sand_automation_cloud_sync::desired_cloud_triggers;
use mahayana_host_runtime::extensions::automations::sand_trigger_hub::ScheduledAutomation;
use serde_json::json;

#[test]
fn listener_integrations_project_supported_connection_state() {
    let mut integrations=ListenerIntegrations::default();
    integrations.set_connected("slack",false);
    integrations.set_connected("github",true);
    let slack=parse_stored_trigger(&json!({
        "type":"slack","channel":"#eng","match":{"kind":"message"}
    })).unwrap();
    let github=parse_stored_trigger(&json!({
        "type":"github","repo":"org/repo","events":["pr-opened"]
    })).unwrap();
    assert_eq!(integrations.listener_is_connected(&slack),Some(false));
    assert_eq!(integrations.listener_is_connected(&github),Some(true));
    assert_eq!(integrations.connection_states().len(),2);
}

#[test]
fn cloud_sync_projects_only_enabled_local_cloud_triggers_deterministically() {
    let scheduled=vec![
        ScheduledAutomation{
            agent_id:"b".into(),automation_id:"two".into(),is_enabled:true,
            trigger:parse_stored_trigger(&json!({
                "type":"linear","event":{"case":"issueCreated"},
                "projectIds":["p"],"teamIds":[]
            })).unwrap(),
        },
        ScheduledAutomation{
            agent_id:"a".into(),automation_id:"one".into(),is_enabled:true,
            trigger:parse_stored_trigger(&json!({
                "type":"microsoftTeams","tenantId":"t","teamIds":["team"],
                "channelIds":[],"messageContains":"deploy"
            })).unwrap(),
        },
        ScheduledAutomation{
            agent_id:"c".into(),automation_id:"three".into(),is_enabled:false,
            trigger:parse_stored_trigger(&json!({
                "type":"sentry","event":{"case":"issueAny"},"projectIds":[]
            })).unwrap(),
        },
    ];
    let desired=desired_cloud_triggers(&scheduled,|agent,_|agent!="b");
    assert_eq!(desired.len(),1);
    assert_eq!(desired[0].agent_id,"a");
    assert_eq!(desired[0].trigger.case,"microsoftTeamsTrigger");
}


#[test]
fn listener_integration_counts_enabled_routines_once_per_platform() {
    let first = json!({
        "type":"group",
        "listeners":[
            {"type":"slack","channel":"#eng","match":{"kind":"message"}},
            {"type":"slack","channel":"#alerts","match":{"kind":"message"}},
            {"type":"github","repo":"org/repo","events":["pr-opened"]}
        ]
    });
    let disabled = json!({
        "type":"github","repo":"org/disabled","events":["pr-opened"]
    });
    let counts = count_listener_platforms([
        (true, &first),
        (false, &disabled),
    ]);
    assert_eq!(counts["slack"], 1);
    assert_eq!(counts["github"], 1);
}

#[test]
fn listener_agent_channels_keep_only_shipping_listener_connectors() {
    let channels = vec![
        json!({"platform":"slack","label":"Slack","status":"configured"}),
        json!({"platform":"github","label":"GitHub","status":"configured"}),
        json!({"platform":"discord","label":"Discord","status":"configured"}),
    ];
    assert_eq!(
        filter_listener_agent_channels(&channels),
        vec![channels[0].clone(), channels[1].clone()]
    );
}


#[test]
fn listener_agent_channels_view_matches_frozen_automations_surface() {
    let channels = vec![
        json!({"platform":"slack","label":"Slack","status":"configured"}),
        json!({"platform":"github","label":"GitHub","status":"configured"}),
        json!({"platform":"discord","label":"Discord","status":"configured"}),
    ];
    assert_eq!(
        listener_agent_channels_view(&channels),
        json!({
            "manifests":[{"platform":"slack"},{"platform":"github"}],
            "connections":[channels[0].clone(), channels[1].clone()]
        })
    );
}
