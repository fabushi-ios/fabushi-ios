use mahayana_host_runtime::automations::automation_trigger::parse_stored_trigger;
use mahayana_host_runtime::extensions::automations::listener_connect_watcher::ListenerConnectWatcher;
use mahayana_host_runtime::extensions::automations::sand_automation_cloud_trigger::backend_cloud_trigger;
use mahayana_host_runtime::extensions::automations::sand_trigger_hub::{
    desired_listeners_by_kind, matching_fires, ScheduledAutomation,
};
use serde_json::json;

#[test]
fn listener_watcher_requires_a_seen_disconnect_before_reconnect_notification() {
    let mut watcher = ListenerConnectWatcher::new(100);
    watcher.watch("agent-a", "slack", 0);
    assert!(watcher.tick(10, |_| Ok(false)).is_empty());
    assert_eq!(
        watcher.tick(20, |_| Ok(true)),
        vec![("agent-a".to_string(), "slack".to_string())]
    );

    watcher.watch("agent-b", "github", 30);
    watcher.suspend();
    assert!(watcher.tick(40, |_| Ok(false)).is_empty());
    watcher.resume();
    assert!(watcher.tick(50, |_| Ok(false)).is_empty());
    watcher.dispose();
    assert!(watcher.pending().is_empty());
    assert!(watcher.tick(60, |_| Ok(true)).is_empty());
}

#[test]
fn cloud_trigger_projection_preserves_frozen_backend_cases() {
    let teams = parse_stored_trigger(&json!({
        "type":"microsoftTeams",
        "tenantId":"tenant",
        "teamIds":["team"],
        "channelIds":["channel"],
        "messageContains":"deploy"
    })).unwrap();
    let projected = backend_cloud_trigger(&teams).unwrap();
    assert_eq!(projected.case, "microsoftTeamsTrigger");
    assert_eq!(projected.value["tenantId"], "tenant");

    let linear = parse_stored_trigger(&json!({
        "type":"linear",
        "event":{"case":"statusChanged","statusIds":["done"]},
        "projectIds":["p"],
        "teamIds":["t"]
    })).unwrap();
    let projected = backend_cloud_trigger(&linear).unwrap();
    assert_eq!(projected.case, "linear");
    assert_eq!(projected.value["event"]["case"], "statusChanged");
}

#[test]
fn trigger_hub_derives_enabled_local_listeners_and_fire_intents() {
    let scheduled = vec![
        ScheduledAutomation {
            agent_id:"a".into(),
            automation_id:"one".into(),
            is_enabled:true,
            trigger:parse_stored_trigger(&json!({
                "type":"slack",
                "channel":"#eng",
                "match":{"kind":"keyword","keyword":"deploy"}
            })).unwrap(),
        },
        ScheduledAutomation {
            agent_id:"b".into(),
            automation_id:"two".into(),
            is_enabled:false,
            trigger:parse_stored_trigger(&json!({
                "type":"github",
                "repo":"org/repo",
                "events":["pr-opened"]
            })).unwrap(),
        },
    ];

    let desired = desired_listeners_by_kind(&scheduled, |_, _| true);
    assert_eq!(desired.get("slack").map(Vec::len), Some(1));
    assert!(!desired.contains_key("github"));

    let fires = matching_fires(
        &scheduled,
        &json!({"source":"slack","channel":"eng","text":"DEPLOY prod"}),
        true,
        false,
        false,
        |_, _| true,
    );
    assert_eq!(fires.len(), 1);
    assert_eq!(fires[0].agent_id, "a");
    assert_eq!(fires[0].automation_id, "one");

    assert!(matching_fires(
        &scheduled,
        &json!({"source":"slack","channel":"eng","text":"deploy"}),
        false,
        false,
        false,
        |_, _| true,
    ).is_empty());
}
