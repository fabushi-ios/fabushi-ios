use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::automations::automation_id::stable_automation_id;
use mahayana_host_runtime::extensions::automations::automations_proto::{
    Automation, AutomationWithOwner, CreateAutomationRequest, ListAutomationsResponse, Trigger,
    UpdateAutomationRequest,
};
use mahayana_host_runtime::extensions::automations::automations_proto::trigger;
use mahayana_host_runtime::extensions::automations::connect_unary::SandConnectError;
use mahayana_host_runtime::extensions::automations::sand_automation_cloud_sync::{
    CloudSyncClient, SAND_SHADOW_MARKER_PREFIX, SandAutomationCloudSync, ScheduledCloudAutomation,
    sand_cloud_definition,
};
use serde_json::json;

fn scheduled(trigger: serde_json::Value) -> ScheduledCloudAutomation {
    ScheduledCloudAutomation {
        id: "routine-1".into(),
        name: "Daily review".into(),
        prompt: "Review the latest changes".into(),
        is_enabled: true,
        trigger,
    }
}

#[test]
fn frozen_cloud_definition_uses_binary_trigger_contract_and_revision_fence() {
    let automation = scheduled(json!({
        "type": "group",
        "listeners": [
            {"type": "cron", "schedule": "@daily"},
            {"type": "slack", "channel": "#shipping", "match": {"kind": "keyword", "keyword": "release"}},
            {"type": "github", "repo": "bhrumom/fabushi-desktop", "events": ["pr-opened", "ci-passed"], "ciBranch": "main"}
        ]
    }));
    let definition = sand_cloud_definition("agent-1", &automation, Some("America/Los_Angeles"))
        .expect("server schedulable");

    assert_eq!(definition.automation_id, stable_automation_id("agent-1", "routine-1"));
    assert!(definition.marker.starts_with(SAND_SHADOW_MARKER_PREFIX));
    assert_eq!(definition.marker, format!("{SAND_SHADOW_MARKER_PREFIX}{}", definition.hash));
    assert_eq!(definition.workflow.prompts[0].prompt, "Review the latest changes");
    assert_eq!(definition.workflow.triggers.len(), 4);

    match definition.workflow.triggers[0].trigger.as_ref() {
        Some(trigger::Trigger::Cron(cron)) => {
            assert_eq!(cron.cron, "CRON_TZ=America/Los_Angeles 0 0 * * *");
        }
        other => panic!("expected cron trigger, got {other:?}"),
    }
    match definition.workflow.triggers[1].trigger.as_ref() {
        Some(trigger::Trigger::SlackTrigger(slack)) => {
            assert_eq!(slack.channels, vec!["#shipping"]);
            assert_eq!(slack.message_contains, "release");
            assert_eq!(slack.top_level_only, Some(false));
        }
        other => panic!("expected slack trigger, got {other:?}"),
    }
    match definition.workflow.triggers[2].trigger.as_ref() {
        Some(trigger::Trigger::Git(git)) => match git.event.as_ref() {
            Some(mahayana_host_runtime::extensions::automations::automations_proto::git_trigger::Event::PullRequest(pr)) => {
                assert_eq!(pr.repos, vec!["https://github.com/bhrumom/fabushi-desktop"]);
                assert_eq!(pr.pr_action, 1);
            }
            other => panic!("expected PR opened trigger, got {other:?}"),
        },
        other => panic!("expected git trigger, got {other:?}"),
    }
    match definition.workflow.triggers[3].trigger.as_ref() {
        Some(trigger::Trigger::Git(git)) => match git.event.as_ref() {
            Some(mahayana_host_runtime::extensions::automations::automations_proto::git_trigger::Event::CiCompleted(ci)) => {
                assert_eq!(ci.repos, vec!["https://github.com/bhrumom/fabushi-desktop"]);
                assert_eq!(ci.condition, 2);
                assert_eq!(ci.branch, "main");
            }
            other => panic!("expected CI trigger, got {other:?}"),
        },
        other => panic!("expected git trigger, got {other:?}"),
    }

    let mut renamed = automation.clone();
    renamed.name = "Renamed review".into();
    let renamed = sand_cloud_definition("agent-1", &renamed, Some("America/Los_Angeles")).unwrap();
    assert_ne!(definition.hash, renamed.hash);

    let mut disabled = automation.clone();
    disabled.is_enabled = false;
    let disabled = sand_cloud_definition("agent-1", &disabled, Some("America/Los_Angeles")).unwrap();
    assert_ne!(definition.hash, disabled.hash);
    assert!(!disabled.enabled);
}

#[test]
fn slack_dm_listener_is_never_cloud_schedulable() {
    let automation = scheduled(json!({
        "type": "slack",
        "channel": "@alice",
        "match": {"kind": "mention"}
    }));
    assert!(sand_cloud_definition("agent-1", &automation, None).is_none());
}

#[derive(Default)]
struct FakeState {
    by_agent: BTreeMap<String, Vec<AutomationWithOwner>>,
    creates: usize,
    updates: usize,
    deletes: usize,
    fail_delete_once: bool,
}

#[derive(Default)]
struct FakeClient {
    state: Mutex<FakeState>,
}

impl FakeClient {
    fn shadows(&self, agent_id: &str) -> Vec<Automation> {
        self.state
            .lock()
            .unwrap()
            .by_agent
            .get(agent_id)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.workflow.clone())
            .collect()
    }

    fn counters(&self) -> (usize, usize, usize) {
        let state = self.state.lock().unwrap();
        (state.creates, state.updates, state.deletes)
    }
}

impl CloudSyncClient for FakeClient {
    fn list_sand_automations(&self, agent_id: &str) -> Result<ListAutomationsResponse, SandConnectError> {
        Ok(ListAutomationsResponse {
            workflows: self
                .state
                .lock()
                .unwrap()
                .by_agent
                .get(agent_id)
                .cloned()
                .unwrap_or_default(),
        })
    }

    fn create_sand_automation(&self, request: CreateAutomationRequest) -> Result<(), SandConnectError> {
        let mut state = self.state.lock().unwrap();
        state.creates += 1;
        let agent_id = request.sand_agent_id.unwrap();
        let automation_id = request.sand_automation_id.unwrap();
        state.by_agent.entry(agent_id).or_default().push(AutomationWithOwner {
            workflow: Some(Automation {
                enabled: request.enabled.unwrap_or(true),
                description: request.description,
                automation_id,
            }),
        });
        Ok(())
    }

    fn update_sand_automation(&self, request: UpdateAutomationRequest) -> Result<(), SandConnectError> {
        let mut state = self.state.lock().unwrap();
        state.updates += 1;
        for entries in state.by_agent.values_mut() {
            for entry in entries {
                let Some(remote) = entry.workflow.as_mut() else { continue };
                if remote.automation_id == request.automation_id {
                    if let Some(enabled) = request.enabled {
                        remote.enabled = enabled;
                    }
                    if request.description.is_some() {
                        remote.description = request.description.clone();
                    }
                }
            }
        }
        Ok(())
    }

    fn delete_sand_automation(&self, automation_id: &str) -> Result<(), SandConnectError> {
        let mut state = self.state.lock().unwrap();
        state.deletes += 1;
        if state.fail_delete_once {
            state.fail_delete_once = false;
            return Err(SandConnectError::Transport("injected delete failure".into()));
        }
        for entries in state.by_agent.values_mut() {
            entries.retain(|entry| entry.workflow.as_ref().is_none_or(|remote| remote.automation_id != automation_id));
        }
        Ok(())
    }
}

#[test]
fn reconcile_mutates_then_readbacks_before_granting_remote_authority() {
    let automation = scheduled(json!({
        "type": "slack",
        "channel": "#shipping",
        "match": {"kind": "message"}
    }));
    let desired = sand_cloud_definition("agent-1", &automation, None).unwrap();
    let client = Arc::new(FakeClient::default());
    {
        let mut state = client.state.lock().unwrap();
        state.by_agent.insert(
            "agent-1".into(),
            vec![AutomationWithOwner {
                workflow: Some(Automation {
                    enabled: true,
                    description: Some(format!("{SAND_SHADOW_MARKER_PREFIX}stale")),
                    automation_id: "stale-shadow".into(),
                }),
            }],
        );
    }

    let trait_client: Arc<dyn CloudSyncClient> = client.clone();
    let mut sync = SandAutomationCloudSync::new(trait_client);
    let outcome = sync
        .reconcile_agent("agent-1", vec![desired.clone()])
        .expect("reconcile");
    assert!(outcome.converged);
    assert!(outcome.scheduling_authority_changed);
    assert_eq!(client.counters(), (1, 0, 1));
    let prunes = sync.drain_shadow_prune_reports();
    assert_eq!(prunes.len(), 1);
    assert_eq!(prunes[0].automation_id, "stale-shadow");
    assert_eq!(prunes[0].outcome, "deleted");

    let remote = client.shadows("agent-1");
    assert_eq!(remote.len(), 1);
    assert_eq!(remote[0].automation_id, desired.automation_id);
    assert_eq!(remote[0].description.as_deref(), Some(desired.marker.as_str()));
    assert!(remote[0].enabled);

    // Once readback proves the enabled cloud shadow exists, local listener execution yields.
    assert!(!sync.should_schedule_locally("agent-1", &automation));

    let second = sync
        .reconcile_agent("agent-1", vec![desired])
        .expect("second reconcile");
    assert!(second.converged);
    assert!(!second.scheduling_authority_changed);
    assert_eq!(client.counters(), (1, 0, 1));
}

#[test]
fn delete_agent_requires_empty_readback_before_clearing_pending_state() {
    let client = Arc::new(FakeClient::default());
    {
        client.state.lock().unwrap().by_agent.insert(
            "agent-1".into(),
            vec![AutomationWithOwner {
                workflow: Some(Automation {
                    enabled: true,
                    description: Some(format!("{SAND_SHADOW_MARKER_PREFIX}old")),
                    automation_id: "remote-1".into(),
                }),
            }],
        );
    }
    let trait_client: Arc<dyn CloudSyncClient> = client.clone();
    let mut sync = SandAutomationCloudSync::new(trait_client);
    let outcome = sync.delete_agent("agent-1").expect("delete agent");
    assert!(outcome.converged);
    assert!(sync.pending_agent_deletions().is_empty());
    assert!(client.shadows("agent-1").is_empty());
    assert_eq!(client.counters().2, 1);
}

#[test]
fn pending_agent_deletion_is_redriven_after_a_transient_failure() {
    let client = Arc::new(FakeClient::default());
    {
        let mut state = client.state.lock().unwrap();
        state.fail_delete_once = true;
        state.by_agent.insert(
            "agent-1".into(),
            vec![AutomationWithOwner {
                workflow: Some(Automation {
                    enabled: true,
                    description: Some(format!("{SAND_SHADOW_MARKER_PREFIX}old")),
                    automation_id: "remote-1".into(),
                }),
            }],
        );
    }
    let trait_client: Arc<dyn CloudSyncClient> = client.clone();
    let mut sync = SandAutomationCloudSync::new(trait_client);

    let first = sync.delete_agent("agent-1").expect("first delete attempt");
    assert!(!first.converged);
    assert!(sync.pending_agent_deletions().contains("agent-1"));
    assert!(sync.failed_agent_ids().contains("agent-1"));

    let retried = sync.retry_pending_agent_deletions();
    assert_eq!(retried.len(), 1);
    assert!(retried[0].1.as_ref().expect("retry").converged);
    assert!(sync.pending_agent_deletions().is_empty());
    assert!(!sync.failed_agent_ids().contains("agent-1"));
    assert!(client.shadows("agent-1").is_empty());
}

#[test]
fn workflow_wire_roundtrip_keeps_frozen_oneof_tags() {
    let automation = scheduled(json!({
        "type": "slack",
        "channel": "#shipping",
        "match": {"kind": "mention"}
    }));
    let definition = sand_cloud_definition("agent-1", &automation, None).unwrap();
    let bytes = prost::Message::encode_to_vec(&definition.workflow);
    let decoded = <mahayana_host_runtime::extensions::automations::automations_proto::Workflow as prost::Message>::decode(bytes.as_slice()).unwrap();
    assert_eq!(decoded, definition.workflow);
    assert!(matches!(
        decoded.triggers[0].trigger,
        Some(trigger::Trigger::SlackMention(_))
    ));
}
