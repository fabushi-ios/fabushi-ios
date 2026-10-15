// Included by implementation.rs: cloud reconciliation remains part of the
// existing FeatureHostController/FeatureState owner. Native Swift only
// transports opaque protobuf bytes produced and consumed here.

use crate::automation_cloud_definition::{
    automation_cloud_definition, is_server_schedulable, stable_automation_id,
    AutomationCloudDefinition, SAND_SHADOW_MARKER_PREFIX,
};
use crate::automation_cloud_proto::{
    CreateAutomationRequest, DeleteAutomationRequest, ListAutomationsResponse,
    ListSandAutomationsRequest, UpdateAutomationRequest,
};
use prost::Message as _;

#[derive(Debug, Default)]
struct AutomationCloudSyncState {
    known_agents: BTreeSet<String>,
    pending_agent_deletions: BTreeSet<String>,
    enabled_ids_by_agent: BTreeMap<String, BTreeSet<String>>,
    failed_agents: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct RemoteAutomationShadow {
    automation_id: String,
    enabled: bool,
    marker: String,
}

fn cloud_message_base64<M: prost::Message>(message: &M) -> String {
    base64::engine::general_purpose::STANDARD.encode(message.encode_to_vec())
}

fn cloud_response_bytes(value: &str) -> Result<Vec<u8>, FeatureHostError> {
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|error| FeatureHostError::Contract(format!(
            "decode automation cloud response: {error}"
        )))
}

fn automation_has_listener(trigger: &AutomationTrigger) -> bool {
    match trigger {
        AutomationTrigger::Schedule { .. } => false,
        AutomationTrigger::Event { .. } => true,
        AutomationTrigger::Group { listeners } => listeners.iter().any(automation_has_listener),
    }
}

fn automation_cloud_should_schedule_locally(
    state: &FeatureState,
    automation: &AutomationSummary,
) -> bool {
    let trigger = automation.trigger.clone().unwrap_or_else(|| {
        AutomationTrigger::Schedule {
            schedule: automation.schedule.clone(),
        }
    });
    if !is_server_schedulable(&trigger) {
        return true;
    }
    if !automation_has_listener(&trigger) {
        return false;
    }
    let agent_id = automation
        .agent_id
        .as_deref()
        .unwrap_or("mahayana-assistant");
    let stable_id = stable_automation_id(agent_id, &automation.id);
    state
        .automation_cloud_sync
        .enabled_ids_by_agent
        .get(agent_id)
        .is_some_and(|enabled| !enabled.contains(&stable_id))
}

fn desired_cloud_automations(
    state: &FeatureState,
    agent_id: &str,
    time_zone: Option<&str>,
) -> BTreeMap<String, AutomationCloudDefinition> {
    state
        .automations
        .values()
        .filter(|automation| {
            automation.agent_id.as_deref().unwrap_or("mahayana-assistant") == agent_id
        })
        .filter_map(|automation| {
            automation_cloud_definition(agent_id, automation, time_zone)
                .map(|definition| (definition.automation_id.clone(), definition))
        })
        .collect()
}

fn remote_cloud_shadows(
    response_base64: &str,
) -> Result<BTreeMap<String, RemoteAutomationShadow>, FeatureHostError> {
    let bytes = cloud_response_bytes(response_base64)?;
    let response = ListAutomationsResponse::decode(bytes.as_slice()).map_err(|error| {
        FeatureHostError::Contract(format!("decode ListSandAutomations response: {error}"))
    })?;
    Ok(response
        .workflows
        .into_iter()
        .filter_map(|owned| owned.workflow)
        .filter_map(|automation| {
            let marker = automation.description?;
            if !marker.starts_with(SAND_SHADOW_MARKER_PREFIX) {
                return None;
            }
            Some((
                automation.automation_id.clone(),
                RemoteAutomationShadow {
                    automation_id: automation.automation_id,
                    enabled: automation.enabled,
                    marker,
                },
            ))
        })
        .collect())
}

fn cloud_is_converged(
    desired: &BTreeMap<String, AutomationCloudDefinition>,
    remote: &BTreeMap<String, RemoteAutomationShadow>,
) -> bool {
    desired.len() == remote.len()
        && desired.iter().all(|(id, definition)| {
            remote.get(id).is_some_and(|shadow| {
                shadow.enabled == definition.enabled && shadow.marker == definition.marker
            })
        })
}

fn cloud_mutations(
    agent_id: &str,
    desired: &BTreeMap<String, AutomationCloudDefinition>,
    remote: &BTreeMap<String, RemoteAutomationShadow>,
) -> (Vec<Value>, usize) {
    let mut mutations = Vec::new();
    let mut shadow_prune_count = 0usize;
    for remote_id in remote.keys().filter(|id| !desired.contains_key(*id)) {
        shadow_prune_count += 1;
        let request = DeleteAutomationRequest {
            automation_id: remote_id.clone(),
        };
        mutations.push(json!({
            "method": "DeleteSandAutomation",
            "automationId": remote_id,
            "bodyBase64": cloud_message_base64(&request),
        }));
    }
    for (automation_id, definition) in desired {
        match remote.get(automation_id) {
            None => {
                let request = CreateAutomationRequest {
                    name: definition.name.clone(),
                    workflow: Some(definition.workflow.clone()),
                    description: Some(definition.marker.clone()),
                    enabled: Some(definition.enabled),
                    sand_agent_id: Some(agent_id.to_string()),
                    sand_automation_id: Some(automation_id.clone()),
                };
                mutations.push(json!({
                    "method": "CreateSandAutomation",
                    "automationId": automation_id,
                    "bodyBase64": cloud_message_base64(&request),
                }));
            }
            Some(shadow)
                if shadow.enabled != definition.enabled
                    || shadow.marker != definition.marker =>
            {
                let request = UpdateAutomationRequest {
                    name: Some(definition.name.clone()),
                    workflow: Some(definition.workflow.clone()),
                    enabled: Some(definition.enabled),
                    description: Some(definition.marker.clone()),
                    automation_id: automation_id.clone(),
                };
                mutations.push(json!({
                    "method": "UpdateSandAutomation",
                    "automationId": automation_id,
                    "bodyBase64": cloud_message_base64(&request),
                }));
            }
            Some(_) => {}
        }
    }
    (mutations, shadow_prune_count)
}

fn record_cloud_authority(
    state: &mut FeatureState,
    agent_id: &str,
    desired: &BTreeMap<String, AutomationCloudDefinition>,
    remote: &BTreeMap<String, RemoteAutomationShadow>,
) {
    if desired.is_empty() && remote.is_empty() {
        state.automation_cloud_sync.known_agents.remove(agent_id);
        state.automation_cloud_sync.pending_agent_deletions.remove(agent_id);
        state.automation_cloud_sync.enabled_ids_by_agent.remove(agent_id);
        state.automation_cloud_sync.failed_agents.remove(agent_id);
        return;
    }
    state.automation_cloud_sync.known_agents.insert(agent_id.to_string());
    state.automation_cloud_sync.pending_agent_deletions.remove(agent_id);
    state.automation_cloud_sync.failed_agents.remove(agent_id);
    let enabled = remote
        .values()
        .filter(|shadow| shadow.enabled)
        .map(|shadow| shadow.automation_id.clone())
        .collect::<BTreeSet<_>>();
    state
        .automation_cloud_sync
        .enabled_ids_by_agent
        .insert(agent_id.to_string(), enabled);
}

impl FeatureHostController {
    pub fn automation_cloud_sync_snapshot(&self) -> Result<Value, FeatureHostError> {
        let time_zone = self.host_settings_user_timezone_direct()?;
        let state = self.state()?;
        ensure_open(&state)?;
        let mut agents = state
            .automation_cloud_sync
            .known_agents
            .union(&state.automation_cloud_sync.pending_agent_deletions)
            .cloned()
            .collect::<BTreeSet<_>>();
        for automation in state.automations.values() {
            let agent_id = automation
                .agent_id
                .as_deref()
                .unwrap_or("mahayana-assistant");
            if automation_cloud_definition(agent_id, automation, time_zone.as_deref()).is_some() {
                agents.insert(agent_id.to_string());
            }
        }
        Ok(json!({
            "agents": agents.into_iter().map(|agent_id| {
                let body = ListSandAutomationsRequest {
                    sand_agent_id: agent_id.clone(),
                };
                json!({
                    "agentId": agent_id,
                    "listRequestBase64": cloud_message_base64(&body),
                })
            }).collect::<Vec<_>>(),
        }))
    }

    pub fn automation_cloud_sync_plan(
        &self,
        agent_id: &str,
        response_base64: &str,
    ) -> Result<Value, FeatureHostError> {
        let agent_id = agent_id.trim();
        if agent_id.is_empty() {
            return Err(FeatureHostError::Contract(
                "automation cloud sync agentId is required".into(),
            ));
        }
        let time_zone = self.host_settings_user_timezone_direct()?;
        let remote = remote_cloud_shadows(response_base64)?;
        let desired = {
            let state = self.state()?;
            ensure_open(&state)?;
            desired_cloud_automations(&state, agent_id, time_zone.as_deref())
        };
        if cloud_is_converged(&desired, &remote) {
            let mut state = self.state()?;
            record_cloud_authority(&mut state, agent_id, &desired, &remote);
            return Ok(json!({
                "converged": true,
                "mutations": [],
                "shadowPruneCount": 0,
            }));
        }
        {
            let mut state = self.state()?;
            state.automation_cloud_sync.enabled_ids_by_agent.remove(agent_id);
            state.automation_cloud_sync.failed_agents.remove(agent_id);
            if desired.is_empty() {
                state
                    .automation_cloud_sync
                    .pending_agent_deletions
                    .insert(agent_id.to_string());
            }
        }
        let (mutations, shadow_prune_count) =
            cloud_mutations(agent_id, &desired, &remote);
        Ok(json!({
            "converged": false,
            "mutations": mutations,
            "shadowPruneCount": shadow_prune_count,
        }))
    }

    pub fn automation_cloud_sync_commit(
        &self,
        agent_id: &str,
        response_base64: &str,
    ) -> Result<Value, FeatureHostError> {
        let agent_id = agent_id.trim();
        if agent_id.is_empty() {
            return Err(FeatureHostError::Contract(
                "automation cloud sync agentId is required".into(),
            ));
        }
        let time_zone = self.host_settings_user_timezone_direct()?;
        let remote = remote_cloud_shadows(response_base64)?;
        let desired = {
            let state = self.state()?;
            ensure_open(&state)?;
            desired_cloud_automations(&state, agent_id, time_zone.as_deref())
        };
        let converged = cloud_is_converged(&desired, &remote);
        let mut state = self.state()?;
        if converged {
            record_cloud_authority(&mut state, agent_id, &desired, &remote);
        } else {
            state.automation_cloud_sync.enabled_ids_by_agent.remove(agent_id);
            state.automation_cloud_sync.failed_agents.insert(agent_id.to_string());
        }
        Ok(json!({
            "converged": converged,
            "enabledAutomationIds": state
                .automation_cloud_sync
                .enabled_ids_by_agent
                .get(agent_id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect::<Vec<_>>(),
        }))
    }

    pub fn automation_cloud_sync_mark_failed(
        &self,
        agent_id: &str,
    ) -> Result<(), FeatureHostError> {
        let agent_id = agent_id.trim();
        if agent_id.is_empty() {
            return Err(FeatureHostError::Contract(
                "automation cloud sync agentId is required".into(),
            ));
        }
        let mut state = self.state()?;
        state.automation_cloud_sync.enabled_ids_by_agent.remove(agent_id);
        state.automation_cloud_sync.failed_agents.insert(agent_id.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod automation_cloud_sync_tests {
    use super::*;

    fn definition(id: &str, marker: &str, enabled: bool) -> AutomationCloudDefinition {
        AutomationCloudDefinition {
            automation_id: id.to_string(),
            enabled,
            hash: marker.trim_start_matches(SAND_SHADOW_MARKER_PREFIX).to_string(),
            local_id: format!("local-{id}"),
            marker: marker.to_string(),
            name: format!("Automation {id}"),
            workflow: crate::automation_cloud_proto::Workflow {
                triggers: Vec::new(),
                prompts: Vec::new(),
            },
        }
    }

    #[test]
    fn cloud_mutation_plan_creates_updates_and_prunes_shadows() {
        let desired = BTreeMap::from([
            (
                "aaaaaaaa-aaaa-5aaa-8aaa-aaaaaaaaaaaa".to_string(),
                definition(
                    "aaaaaaaa-aaaa-5aaa-8aaa-aaaaaaaaaaaa",
                    "sand-shadow:new-a",
                    true,
                ),
            ),
            (
                "bbbbbbbb-bbbb-5bbb-8bbb-bbbbbbbbbbbb".to_string(),
                definition(
                    "bbbbbbbb-bbbb-5bbb-8bbb-bbbbbbbbbbbb",
                    "sand-shadow:new-b",
                    false,
                ),
            ),
        ]);
        let remote = BTreeMap::from([
            (
                "aaaaaaaa-aaaa-5aaa-8aaa-aaaaaaaaaaaa".to_string(),
                RemoteAutomationShadow {
                    automation_id: "aaaaaaaa-aaaa-5aaa-8aaa-aaaaaaaaaaaa".into(),
                    enabled: false,
                    marker: "sand-shadow:old-a".into(),
                },
            ),
            (
                "cccccccc-cccc-5ccc-8ccc-cccccccccccc".to_string(),
                RemoteAutomationShadow {
                    automation_id: "cccccccc-cccc-5ccc-8ccc-cccccccccccc".into(),
                    enabled: true,
                    marker: "sand-shadow:orphan".into(),
                },
            ),
        ]);
        let (mutations, pruned) =
            cloud_mutations("mahayana-assistant", &desired, &remote);
        let methods = mutations
            .iter()
            .filter_map(|mutation| mutation["method"].as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(pruned, 1);
        assert_eq!(methods.len(), 3);
        assert!(methods.contains("CreateSandAutomation"));
        assert!(methods.contains("UpdateSandAutomation"));
        assert!(methods.contains("DeleteSandAutomation"));
    }

    #[test]
    fn cloud_readback_requires_exact_marker_enabled_and_key_set() {
        let desired = BTreeMap::from([(
            "aaaaaaaa-aaaa-5aaa-8aaa-aaaaaaaaaaaa".to_string(),
            definition(
                "aaaaaaaa-aaaa-5aaa-8aaa-aaaaaaaaaaaa",
                "sand-shadow:exact",
                true,
            ),
        )]);
        let exact = BTreeMap::from([(
            "aaaaaaaa-aaaa-5aaa-8aaa-aaaaaaaaaaaa".to_string(),
            RemoteAutomationShadow {
                automation_id: "aaaaaaaa-aaaa-5aaa-8aaa-aaaaaaaaaaaa".into(),
                enabled: true,
                marker: "sand-shadow:exact".into(),
            },
        )]);
        assert!(cloud_is_converged(&desired, &exact));
        let mut stale = exact.clone();
        stale
            .get_mut("aaaaaaaa-aaaa-5aaa-8aaa-aaaaaaaaaaaa")
            .expect("shadow")
            .marker = "sand-shadow:stale".into();
        assert!(!cloud_is_converged(&desired, &stale));
    }

    #[test]
    fn scheduling_authority_is_cloud_first_except_local_only_listener() {
        let mut state = FeatureState::default();
        let cloud_event = AutomationSummary {
            id: "cloud-event".into(),
            agent_id: None,
            name: "Cloud event".into(),
            prompt: "Handle it".into(),
            schedule: "event:github".into(),
            trigger: Some(AutomationTrigger::Event {
                source: ListenerPlatform::Github,
                event: "pr-opened".into(),
                filter: None,
                filters: Some(BTreeMap::from([("repo".into(), json!("owner/repo"))])),
            }),
            enabled: true,
            created_at_ms: 1,
            raised_notices: Vec::new(),
            runs: Vec::new(),
            last_run_at_ms: None,
            next_run_at_ms: None,
        };
        assert!(!automation_cloud_should_schedule_locally(&state, &cloud_event));
        let stable = stable_automation_id("mahayana-assistant", "cloud-event");
        state
            .automation_cloud_sync
            .enabled_ids_by_agent
            .insert("mahayana-assistant".into(), BTreeSet::new());
        assert!(automation_cloud_should_schedule_locally(&state, &cloud_event));
        state
            .automation_cloud_sync
            .enabled_ids_by_agent
            .get_mut("mahayana-assistant")
            .expect("authority")
            .insert(stable);
        assert!(!automation_cloud_should_schedule_locally(&state, &cloud_event));

        let local_only = AutomationSummary {
            id: "slack-dm".into(),
            trigger: Some(AutomationTrigger::Event {
                source: ListenerPlatform::Slack,
                event: "mention".into(),
                filter: None,
                filters: Some(BTreeMap::from([("channel".into(), json!("@alice"))])),
            }),
            ..cloud_event
        };
        assert!(automation_cloud_should_schedule_locally(&state, &local_only));
    }
}
