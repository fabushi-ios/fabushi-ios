use std::path::PathBuf;

use mahayana_host_runtime::automations::automation::{AutomationRecord, AutomationRun};
use mahayana_host_runtime::automations::automation_id::stable_automation_id;
use mahayana_host_runtime::extensions::automations::fire_delivery::{
    PreparedBackendFire, admits_bare_fire, fire_event_matches_trigger, parse_fire_trigger_event,
    prepare_backend_fire,
};
use mahayana_host_runtime::extensions::automations::sand_automation_fire_consumer::BackendAutomationFire;
use mahayana_host_runtime::extensions::session::agent_session::AgentAutomationEntry;
use serde_json::{Value, json};

fn entry(trigger: Value) -> AgentAutomationEntry {
    AgentAutomationEntry {
        agent_id: "agent-1".into(),
        automation: AutomationRecord {
            id: "local-1".into(),
            name: "Routine".into(),
            prompt: "Do it".into(),
            trigger,
            is_enabled: true,
            created_at: 1.0,
            last_run_at: None,
            raised_notices: Vec::new(),
            schedule: String::new(),
            trigger_description: String::new(),
            next_run_at: None,
            runs: Vec::new(),
            file_path: PathBuf::from("/tmp/automation.md"),
        },
    }
}

fn fire(event: Option<Value>) -> BackendAutomationFire {
    BackendAutomationFire {
        id: "run-1".into(),
        sand_agent_id: "agent-1".into(),
        automation_id: stable_automation_id("agent-1", "local-1"),
        timestamp_ms: 100,
        definition_revision: None,
        scheduled_for_ms: None,
        event,
    }
}

#[test]
fn prepares_schedule_and_event_delivery_only_after_target_validation() {
    let schedule = entry(json!({"type":"cron","schedule":"0 * * * *"}));
    let mut scheduled_fire = fire(None);
    scheduled_fire.scheduled_for_ms = Some(90);
    assert!(matches!(
        prepare_backend_fire(&[schedule], &scheduled_fire, |_| None),
        PreparedBackendFire::Schedule {
            ref agent_id,
            ref automation_id,
            ref run_uuid,
            scheduled_for_ms: Some(90),
        } if agent_id == "agent-1" && automation_id == "local-1" && run_uuid == "run-1"
    ));

    let listener = entry(json!({
        "type":"github",
        "repo":"openai/repo",
        "events":["pr-opened"]
    }));
    let event_fire = fire(Some(json!({
        "source":"github",
        "repo":"openai/repo",
        "kind":"pr-opened",
        "actor":"octocat",
        "timestampMs":99
    })));
    assert!(matches!(
        prepare_backend_fire(&[listener], &event_fire, |_| None),
        PreparedBackendFire::Event {
            ref agent_id,
            ref automation_id,
            ref run_uuid,
            ..
        } if agent_id == "agent-1" && automation_id == "local-1" && run_uuid == "run-1"
    ));
}

#[test]
fn existing_completed_run_is_not_executed_twice() {
    let mut target = entry(json!({"type":"cron","schedule":"0 * * * *"}));
    target.automation.runs.push(AutomationRun {
        id: "other".into(),
        trigger: "schedule".into(),
        started_at: 1.0,
        finished_at: Some(2.0),
        status: "ok".into(),
        detail: None,
        event: None,
        coalesced_run_ids: Some(vec!["run-1".into()]),
    });
    let prepared = prepare_backend_fire(&[target], &fire(None), |_| None);
    assert!(matches!(
        prepared,
        PreparedBackendFire::Complete {
            ref completion,
            reason: "existing_run"
        } if completion.status == "succeeded"
    ));
}

#[test]
fn stale_or_unverifiable_definition_never_executes() {
    let target = entry(json!({"type":"cron","schedule":"0 * * * *"}));
    let mut revisioned = fire(None);
    revisioned.scheduled_for_ms = Some(90);
    revisioned.definition_revision = Some("remote-hash".into());

    assert_eq!(
        prepare_backend_fire(&[target.clone()], &revisioned, |_| None),
        PreparedBackendFire::Abandon {
            reason: "definition_revision_unavailable"
        }
    );
    assert!(matches!(
        prepare_backend_fire(&[target], &revisioned, |_| Some("local-hash".into())),
        PreparedBackendFire::Complete {
            reason: "definition_changed",
            ..
        }
    ));
}

#[test]
fn legacy_schedule_slot_covered_by_last_run_is_rejected() {
    let mut target = entry(json!({"type":"cron","schedule":"0 * * * *"}));
    target.automation.last_run_at = Some(101.0);
    let mut backend_fire = fire(None);
    backend_fire.scheduled_for_ms = Some(100);
    assert!(matches!(
        prepare_backend_fire(&[target], &backend_fire, |_| None),
        PreparedBackendFire::Complete {
            reason: "slot_already_covered",
            ..
        }
    ));
}

#[test]
fn slack_channel_id_alias_can_match_trigger() {
    let parsed = parse_fire_trigger_event(
        &json!({
            "source":"slack",
            "channel":"#alerts",
            "channelId":"C123",
            "sender":"@u",
            "text":"deploy",
            "timestampMs":1
        }),
        1,
    )
    .expect("slack event");
    assert!(fire_event_matches_trigger(
        &json!({
            "type":"slack",
            "channel":"C123",
            "match":{"kind":"message"}
        }),
        &parsed,
        &json!({
            "source":"slack",
            "channel":"#alerts",
            "channelId":"C123",
            "sender":"@u",
            "text":"deploy"
        }),
    ));
}

#[test]
fn bounded_platform_event_parsing_rejects_oversized_or_incomplete_payloads() {
    assert!(parse_fire_trigger_event(
        &json!({
            "source":"microsoftTeams",
            "tenantId":"t",
            "teamId":"team",
            "channelId":"c",
            "text":"hello"
        }),
        10,
    )
    .is_some());

    assert!(parse_fire_trigger_event(
        &json!({
            "source":"microsoftTeams",
            "tenantId":"x".repeat(201),
            "teamId":"team",
            "channelId":"c"
        }),
        10,
    )
    .is_none());

    assert!(parse_fire_trigger_event(
        &json!({
            "source":"pagerduty",
            "event":"incidentTriggered",
            "incidentId":"i",
            "title":"title",
            "status":"triggered"
        }),
        10,
    )
    .is_none());
}

#[test]
fn bare_fire_requires_schedule_and_disambiguates_mixed_triggers() {
    let mut backend_fire = fire(None);
    let schedule_only = json!({"type":"cron","schedule":"0 * * * *"});
    assert!(admits_bare_fire(&schedule_only, &backend_fire));

    let mixed = json!({
        "type":"group",
        "listeners":[
            {"type":"cron","schedule":"0 * * * *"},
            {"type":"github","repo":"openai/repo","events":["pr-opened"]}
        ]
    });
    assert!(!admits_bare_fire(&mixed, &backend_fire));
    backend_fire.scheduled_for_ms = Some(100);
    assert!(admits_bare_fire(&mixed, &backend_fire));
}
