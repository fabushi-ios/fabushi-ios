use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use prost::Message;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::automations::automation_id::stable_automation_id;
use crate::automations::automation_trigger::trigger_members;

use super::automations_proto::{
    Automation, AutomationWithOwner, CreateAutomationRequest, CreateAutomationResponse,
    CronTrigger, DeleteAutomationRequest, DeleteAutomationResponse, GitCiCompletedEvent,
    GitIssueAssignedEvent, GitPullRequestEvent, GitPullRequestReviewCommentEvent,
    GitPullRequestReviewEvent, GitPullRequestReviewRequestedEvent, GitReviewThreadEvent, GitTrigger,
    LinearEndOfCycleEvent, LinearIssueCreatedEvent, LinearStatusChangedEvent, LinearTrigger,
    ListAutomationsResponse, ListSandAutomationsRequest, MicrosoftTeamsTrigger, PagerDutyIncidentAcknowledgedEvent,
    PagerDutyIncidentAnyEvent, PagerDutyIncidentEscalatedEvent, PagerDutyIncidentResolvedEvent,
    PagerDutyIncidentTriggeredEvent, PagerDutyTrigger, Prompt, SentryIssueAnyEvent,
    SentryIssueArchivedEvent, SentryIssueAssignedEvent, SentryIssueCreatedEvent,
    SentryIssueResolvedEvent, SentryIssueUnresolvedEvent, SentryTrigger,
    SlackAnyReactionAddedTrigger, SlackMentionTrigger, SlackReactionAddedTrigger, SlackTrigger,
    Trigger, UpdateAutomationRequest, UpdateAutomationResponse, Workflow,
};
use super::automations_proto::{
    git_trigger, linear_trigger, pager_duty_trigger, sentry_trigger, trigger,
};
use super::connect_unary::{SandConnectError, SandConnectUnaryClient};
use super::sand_automation_cloud_trigger::{BackendCloudTrigger, backend_cloud_trigger};
use super::sand_trigger_hub::ScheduledAutomation;

pub const SAND_SHADOW_MARKER_PREFIX: &str = "sand-shadow:";
pub const AUTOMATIONS_SERVICE: &str = "aiserver.v1.AutomationsService";
const GITHUB_SUBSCRIPTION_VERSION: &[u8] = b"github-subscriptions-v1\0";

#[derive(Debug, Clone, PartialEq)]
pub struct DesiredCloudTrigger {
    pub agent_id: String,
    pub automation_id: String,
    pub trigger: BackendCloudTrigger,
}

/// Compatibility projection retained for the pre-existing extension contract.
/// Shipping cloud reconciliation uses the protobuf-backed CloudDefinition path below.
pub fn desired_cloud_triggers(
    scheduled: &[ScheduledAutomation],
    should_sync: impl Fn(&str, &ScheduledAutomation) -> bool,
) -> Vec<DesiredCloudTrigger> {
    let mut desired = scheduled.iter()
        .filter(|entry| entry.is_enabled)
        .filter(|entry| should_sync(&entry.agent_id, entry))
        .filter_map(|entry| {
            backend_cloud_trigger(&entry.trigger).map(|trigger| DesiredCloudTrigger {
                agent_id: entry.agent_id.clone(),
                automation_id: entry.automation_id.clone(),
                trigger,
            })
        })
        .collect::<Vec<_>>();
    desired.sort_by(|left, right| {
        left.agent_id.cmp(&right.agent_id)
            .then_with(|| left.automation_id.cmp(&right.automation_id))
    });
    desired
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScheduledCloudAutomation {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub is_enabled: bool,
    pub trigger: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CloudDefinition {
    pub automation_id: String,
    pub enabled: bool,
    pub hash: String,
    pub local_id: String,
    pub marker: String,
    pub name: String,
    pub workflow: Workflow,
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

fn string(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn bool_value(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

pub fn is_server_schedulable(trigger_value: &Value) -> bool {
    !trigger_members(trigger_value).iter().any(|listener| {
        listener.get("type").and_then(Value::as_str) == Some("slack")
            && listener.get("channel").and_then(Value::as_str).is_some_and(|channel| channel.starts_with('@'))
    })
}

fn split_schedule_time_zone(schedule: &str) -> (String, Option<String>) {
    let normalized = schedule.split_whitespace().collect::<Vec<_>>().join(" ");
    let Some((prefix, rest)) = normalized.split_once(' ') else {
        return (normalized, None);
    };
    let zone = prefix.strip_prefix("CRON_TZ=").or_else(|| prefix.strip_prefix("TZ="));
    match zone {
        Some(zone) if !zone.is_empty() => (rest.to_owned(), Some(zone.to_owned())),
        _ => (normalized, None),
    }
}

fn expand_cron_alias(schedule: &str) -> String {
    match schedule.to_ascii_lowercase().as_str() {
        "@hourly" => "0 * * * *".into(),
        "@daily" | "@midnight" => "0 0 * * *".into(),
        "@weekly" => "0 0 * * 0".into(),
        "@monthly" => "0 0 1 * *".into(),
        "@yearly" | "@annually" => "0 0 1 1 *".into(),
        _ => schedule.to_owned(),
    }
}

fn cron_cloud_trigger(schedule: &str, time_zone: Option<&str>) -> Trigger {
    let (expression, pinned_zone) = split_schedule_time_zone(schedule);
    let mut expanded = expand_cron_alias(&expression);
    if expanded.len() >= 6 && expanded[..6].eq_ignore_ascii_case("@every") {
        expanded.replace_range(..6, "@every");
    }
    let zone = pinned_zone.as_deref().or(time_zone);
    let cron = match zone {
        Some(zone) if !expanded.starts_with("@every") => format!("CRON_TZ={zone} {expanded}"),
        _ => expanded,
    };
    Trigger {
        trigger: Some(trigger::Trigger::Cron(CronTrigger { cron })),
    }
}

fn slack_triggers(listener: &Value) -> Vec<Trigger> {
    let channel = string(listener, "channel");
    let channels = vec![channel];
    let rule = listener.get("match").unwrap_or(&Value::Null);
    match rule.get("kind").and_then(Value::as_str).unwrap_or_default() {
        "mention" => vec![Trigger {
            trigger: Some(trigger::Trigger::SlackMention(SlackMentionTrigger {
                channel: String::new(),
                channels,
                block_unauthenticated_slack_users: false,
            })),
        }],
        "reaction" => {
            let only_owner_reactions = bool_value(rule, "bySelf");
            let emojis = strings(rule.get("emoji"));
            if emojis.is_empty() {
                vec![Trigger {
                    trigger: Some(trigger::Trigger::SlackAnyReactionAdded(
                        SlackAnyReactionAddedTrigger {
                            channel: String::new(),
                            channels,
                            block_unauthenticated_slack_users: false,
                            only_owner_reactions,
                        },
                    )),
                }]
            } else {
                emojis
                    .into_iter()
                    .map(|emoji_name| Trigger {
                        trigger: Some(trigger::Trigger::SlackReactionAdded(
                            SlackReactionAddedTrigger {
                                channel: String::new(),
                                emoji_name,
                                channels: channels.clone(),
                                block_unauthenticated_slack_users: false,
                                only_owner_reactions,
                            },
                        )),
                    })
                    .collect()
            }
        }
        "keyword" => vec![Trigger {
            trigger: Some(trigger::Trigger::SlackTrigger(SlackTrigger {
                channel: String::new(),
                message_contains: string(rule, "keyword"),
                message_contains_is_regex: false,
                channels,
                block_unauthenticated_slack_users: false,
                top_level_only: Some(false),
            })),
        }],
        "message" => vec![Trigger {
            trigger: Some(trigger::Trigger::SlackTrigger(SlackTrigger {
                channel: String::new(),
                message_contains: String::new(),
                message_contains_is_regex: false,
                channels,
                block_unauthenticated_slack_users: false,
                top_level_only: Some(false),
            })),
        }],
        _ => Vec::new(),
    }
}

fn github_trigger(listener: &Value, event: &str, allowlist_override: Option<Vec<String>>) -> Trigger {
    let repo = format!("https://github.com/{}", string(listener, "repo"));
    let repos = vec![repo];
    let user_allowlist = allowlist_override.unwrap_or_else(|| strings(listener.get("userAllowlist")));
    let event = match event {
        "pr-opened" => Some(git_trigger::Event::PullRequest(GitPullRequestEvent {
            repos,
            pr_action: 1,
        })),
        "pr-pushed" => Some(git_trigger::Event::PullRequest(GitPullRequestEvent {
            repos,
            pr_action: 2,
        })),
        "pr-merged" => Some(git_trigger::Event::PullRequest(GitPullRequestEvent {
            repos,
            pr_action: 3,
        })),
        "pr-comment" => Some(git_trigger::Event::PullRequest(GitPullRequestEvent {
            repos,
            pr_action: 4,
        })),
        "review-requested" => Some(git_trigger::Event::PullRequestReviewRequested(
            GitPullRequestReviewRequestedEvent { repos },
        )),
        "review-approved" => Some(git_trigger::Event::PullRequestReview(
            GitPullRequestReviewEvent {
                repos,
                on_approved: true,
                on_changes_requested: false,
                on_commented: false,
            },
        )),
        "review-changes-requested" => Some(git_trigger::Event::PullRequestReview(
            GitPullRequestReviewEvent {
                repos,
                on_approved: false,
                on_changes_requested: true,
                on_commented: false,
            },
        )),
        "review-commented" => Some(git_trigger::Event::PullRequestReview(
            GitPullRequestReviewEvent {
                repos,
                on_approved: false,
                on_changes_requested: false,
                on_commented: true,
            },
        )),
        "inline-review-comment" => Some(git_trigger::Event::PullRequestReviewComment(
            GitPullRequestReviewCommentEvent { repos },
        )),
        "review-thread-resolved" => Some(git_trigger::Event::ReviewThread(
            GitReviewThreadEvent {
                repos,
                on_resolved: true,
                on_unresolved: false,
            },
        )),
        "review-thread-unresolved" => Some(git_trigger::Event::ReviewThread(
            GitReviewThreadEvent {
                repos,
                on_resolved: false,
                on_unresolved: true,
            },
        )),
        "issue-assigned" => Some(git_trigger::Event::IssueAssigned(GitIssueAssignedEvent {
            repos,
        })),
        _ => None,
    };
    Trigger {
        trigger: Some(trigger::Trigger::Git(GitTrigger {
            event,
            user_allowlist,
        })),
    }
}

fn is_github_ci_event(event: &str) -> bool {
    matches!(event, "ci-passed" | "ci-failed")
}

fn ci_condition(events: &[String]) -> Option<i32> {
    let passed = events.iter().any(|event| event == "ci-passed");
    let failed = events.iter().any(|event| event == "ci-failed");
    match (passed, failed) {
        (true, true) => Some(3),
        (true, false) => Some(2),
        (false, true) => Some(1),
        _ => None,
    }
}

fn github_ci_trigger(repo: &str, branch: &str, condition: i32) -> Trigger {
    Trigger {
        trigger: Some(trigger::Trigger::Git(GitTrigger {
            event: Some(git_trigger::Event::CiCompleted(GitCiCompletedEvent {
                repos: vec![format!("https://github.com/{repo}")],
                condition,
                branch: branch.to_owned(),
            })),
            user_allowlist: Vec::new(),
        })),
    }
}

fn github_listener_triggers(listener: &Value) -> Vec<Trigger> {
    let events = strings(listener.get("events"));
    let mut result = events
        .iter()
        .filter(|event| !is_github_ci_event(event))
        .map(|event| github_trigger(listener, event, None))
        .collect::<Vec<_>>();
    if let (Some(branch), Some(condition)) = (
        listener.get("ciBranch").and_then(Value::as_str).filter(|branch| !branch.is_empty()),
        ci_condition(&events),
    ) {
        result.push(github_ci_trigger(&string(listener, "repo"), branch, condition));
    }
    result
}

fn teams_trigger(listener: &Value) -> Trigger {
    Trigger {
        trigger: Some(trigger::Trigger::MicrosoftTeamsTrigger(MicrosoftTeamsTrigger {
            tenant_id: string(listener, "tenantId"),
            team_id: string(listener, "teamId"),
            team_ids: strings(listener.get("teamIds")),
            channel_ids: strings(listener.get("channelIds")),
            message_contains: string(listener, "messageContains"),
            message_contains_is_regex: bool_value(listener, "messageContainsIsRegex"),
            block_unauthenticated_teams_users: bool_value(listener, "blockUnauthenticatedTeamsUsers"),
        })),
    }
}

fn linear_trigger(listener: &Value) -> Trigger {
    let event_value = listener.get("event").unwrap_or(&Value::Null);
    let event = match event_value.get("case").and_then(Value::as_str).unwrap_or_default() {
        "issueCreated" => Some(linear_trigger::Event::IssueCreated(LinearIssueCreatedEvent {})),
        "statusChanged" => Some(linear_trigger::Event::StatusChanged(
            LinearStatusChangedEvent {
                status_ids: strings(event_value.get("statusIds")),
            },
        )),
        "endOfCycle" => Some(linear_trigger::Event::EndOfCycle(LinearEndOfCycleEvent {
            cycle_ids: strings(event_value.get("cycleIds")),
        })),
        _ => None,
    };
    Trigger {
        trigger: Some(trigger::Trigger::Linear(LinearTrigger {
            event,
            project_ids: strings(listener.get("projectIds")),
            team_ids: strings(listener.get("teamIds")),
        })),
    }
}

fn sentry_trigger(listener: &Value) -> Trigger {
    let event = match listener.get("event").and_then(|event| event.get("case")).and_then(Value::as_str).unwrap_or_default() {
        "issueCreated" => Some(sentry_trigger::Event::IssueCreated(SentryIssueCreatedEvent {})),
        "issueResolved" => Some(sentry_trigger::Event::IssueResolved(SentryIssueResolvedEvent {})),
        "issueAssigned" => Some(sentry_trigger::Event::IssueAssigned(SentryIssueAssignedEvent {})),
        "issueArchived" => Some(sentry_trigger::Event::IssueArchived(SentryIssueArchivedEvent {})),
        "issueUnresolved" => Some(sentry_trigger::Event::IssueUnresolved(SentryIssueUnresolvedEvent {})),
        "issueAny" => Some(sentry_trigger::Event::IssueAny(SentryIssueAnyEvent {})),
        _ => None,
    };
    Trigger {
        trigger: Some(trigger::Trigger::Sentry(SentryTrigger {
            event,
            project_ids: strings(listener.get("projectIds")),
        })),
    }
}

fn pagerduty_trigger(listener: &Value) -> Trigger {
    let event = match listener.get("event").and_then(|event| event.get("case")).and_then(Value::as_str).unwrap_or_default() {
        "incidentTriggered" => Some(pager_duty_trigger::Event::IncidentTriggered(PagerDutyIncidentTriggeredEvent {})),
        "incidentAcknowledged" => Some(pager_duty_trigger::Event::IncidentAcknowledged(PagerDutyIncidentAcknowledgedEvent {})),
        "incidentResolved" => Some(pager_duty_trigger::Event::IncidentResolved(PagerDutyIncidentResolvedEvent {})),
        "incidentEscalated" => Some(pager_duty_trigger::Event::IncidentEscalated(PagerDutyIncidentEscalatedEvent {})),
        "incidentAny" => Some(pager_duty_trigger::Event::IncidentAny(PagerDutyIncidentAnyEvent {})),
        _ => None,
    };
    Trigger {
        trigger: Some(trigger::Trigger::Pagerduty(PagerDutyTrigger {
            event,
            service_ids: strings(listener.get("serviceIds")),
        })),
    }
}

fn listener_triggers(listener: &Value) -> Vec<Trigger> {
    match listener.get("type").and_then(Value::as_str).unwrap_or_default() {
        "slack" => slack_triggers(listener),
        "github" => github_listener_triggers(listener),
        "microsoftTeams" => vec![teams_trigger(listener)],
        "linear" => vec![linear_trigger(listener)],
        "sentry" => vec![sentry_trigger(listener)],
        "pagerduty" => vec![pagerduty_trigger(listener)],
        _ => Vec::new(),
    }
}

fn union_allowlists(existing: Option<Vec<String>>, addition: Option<Vec<String>>) -> Option<Vec<String>> {
    let (Some(mut existing), Some(addition)) = (existing, addition) else {
        return None;
    };
    for login in addition {
        if !existing.iter().any(|entry| entry.eq_ignore_ascii_case(&login)) {
            existing.push(login);
        }
    }
    Some(existing)
}

fn github_restriction(listener: &Value) -> Option<Vec<String>> {
    let allowlist = strings(listener.get("userAllowlist"));
    (!allowlist.is_empty()).then_some(allowlist)
}

fn group_listener_triggers(listeners: &[Value]) -> Vec<Trigger> {
    struct FirstGithub {
        listener: Value,
        non_ci_events: Vec<String>,
    }
    struct CiEntry {
        repo: String,
        branch: String,
        events: Vec<String>,
    }

    let mut merged = Vec::<Result<Value, FirstGithub>>::new();
    let mut seen_repos = Vec::<String>::new();
    let mut allowlists = BTreeMap::<String, BTreeMap<String, Option<Vec<String>>>>::new();
    let mut ci_entries = Vec::<(String, CiEntry)>::new();

    for listener in listeners {
        if listener.get("type").and_then(Value::as_str) != Some("github") {
            merged.push(Ok(listener.clone()));
            continue;
        }
        let repo = string(listener, "repo");
        let repo_key = repo.to_ascii_lowercase();
        let events = strings(listener.get("events"));
        let ci_events = events.iter().filter(|event| is_github_ci_event(event)).cloned().collect::<Vec<_>>();
        if let Some(branch) = listener.get("ciBranch").and_then(Value::as_str).filter(|branch| !branch.is_empty()) {
            if !ci_events.is_empty() {
                let key = format!("{repo_key}\0{branch}");
                if let Some((_, entry)) = ci_entries.iter_mut().find(|(entry_key, _)| entry_key == &key) {
                    for event in ci_events {
                        if !entry.events.contains(&event) {
                            entry.events.push(event);
                        }
                    }
                } else {
                    ci_entries.push((key, CiEntry {
                        repo: repo.clone(),
                        branch: branch.to_owned(),
                        events: ci_events,
                    }));
                }
            }
        }

        let restriction = github_restriction(listener);
        let non_ci_events = events.into_iter().filter(|event| !is_github_ci_event(event)).collect::<Vec<_>>();
        let by_event = allowlists.entry(repo_key.clone()).or_default();
        for event in &non_ci_events {
            let next = if by_event.contains_key(event) {
                union_allowlists(by_event.get(event).cloned().flatten(), restriction.clone())
            } else {
                restriction.clone()
            };
            by_event.insert(event.clone(), next);
        }

        if seen_repos.iter().any(|seen| seen == &repo_key) {
            continue;
        }
        seen_repos.push(repo_key);
        merged.push(Err(FirstGithub {
            listener: listener.clone(),
            non_ci_events,
        }));
    }

    let mut result = Vec::new();
    for item in merged {
        match item {
            Ok(listener) => result.extend(listener_triggers(&listener)),
            Err(first) => {
                let repo_key = string(&first.listener, "repo").to_ascii_lowercase();
                let by_event = allowlists.get(&repo_key);
                for event in first.non_ci_events {
                    let override_allowlist = by_event
                        .and_then(|map| map.get(&event))
                        .cloned()
                        .flatten()
                        .unwrap_or_default();
                    result.push(github_trigger(&first.listener, &event, Some(override_allowlist)));
                }
            }
        }
    }
    for (_, entry) in ci_entries {
        if let Some(condition) = ci_condition(&entry.events) {
            result.push(github_ci_trigger(&entry.repo, &entry.branch, condition));
        }
    }
    result
}

pub fn cloud_triggers(trigger_value: &Value, time_zone: Option<&str>) -> Option<Vec<Trigger>> {
    let members = trigger_members(trigger_value);
    if members.iter().any(|listener| {
        listener.get("type").and_then(Value::as_str) == Some("slack")
            && listener.get("channel").and_then(Value::as_str).is_some_and(|channel| channel.starts_with('@'))
    }) {
        return None;
    }
    let mut triggers = members
        .iter()
        .filter(|member| member.get("type").and_then(Value::as_str) == Some("cron"))
        .filter_map(|member| member.get("schedule").and_then(Value::as_str))
        .map(|schedule| cron_cloud_trigger(schedule, time_zone))
        .collect::<Vec<_>>();
    let listeners = members
        .into_iter()
        .filter(|member| member.get("type").and_then(Value::as_str) != Some("cron"))
        .collect::<Vec<_>>();
    if listeners.len() == 1 {
        triggers.extend(listener_triggers(&listeners[0]));
    } else if !listeners.is_empty() {
        triggers.extend(group_listener_triggers(&listeners));
    }
    Some(triggers)
}

pub fn sand_cloud_definition(
    agent_id: &str,
    automation: &ScheduledCloudAutomation,
    time_zone: Option<&str>,
) -> Option<CloudDefinition> {
    let triggers = cloud_triggers(&automation.trigger, time_zone)?;
    let workflow = Workflow {
        triggers,
        prompts: vec![Prompt {
            prompt: automation.prompt.clone(),
        }],
    };
    let mut hasher = Sha256::new();
    if workflow.triggers.iter().any(|wire_trigger| matches!(wire_trigger.trigger.as_ref(), Some(trigger::Trigger::Git(_)))) {
        hasher.update(GITHUB_SUBSCRIPTION_VERSION);
    }
    hasher.update(automation.id.as_bytes());
    hasher.update([0]);
    hasher.update(automation.name.as_bytes());
    hasher.update([0]);
    hasher.update(if automation.is_enabled { b"true".as_slice() } else { b"false".as_slice() });
    hasher.update([0]);
    hasher.update(workflow.encode_to_vec());
    let hash = format!("{:x}", hasher.finalize());
    Some(CloudDefinition {
        automation_id: stable_automation_id(agent_id, &automation.id),
        enabled: automation.is_enabled && is_server_schedulable(&automation.trigger),
        local_id: automation.id.clone(),
        marker: format!("{SAND_SHADOW_MARKER_PREFIX}{hash}"),
        name: automation.name.clone(),
        workflow,
        hash,
    })
}

pub trait CloudSyncClient: Send + Sync {
    fn list_sand_automations(&self, agent_id: &str) -> Result<ListAutomationsResponse, SandConnectError>;
    fn create_sand_automation(&self, request: CreateAutomationRequest) -> Result<(), SandConnectError>;
    fn update_sand_automation(&self, request: UpdateAutomationRequest) -> Result<(), SandConnectError>;
    fn delete_sand_automation(&self, automation_id: &str) -> Result<(), SandConnectError>;
}

pub struct ProductionCloudSyncClient {
    connect: Arc<SandConnectUnaryClient>,
}

impl ProductionCloudSyncClient {
    pub fn new(connect: Arc<SandConnectUnaryClient>) -> Self {
        Self { connect }
    }
}

impl CloudSyncClient for ProductionCloudSyncClient {
    fn list_sand_automations(&self, agent_id: &str) -> Result<ListAutomationsResponse, SandConnectError> {
        self.connect.unary(
            AUTOMATIONS_SERVICE,
            "ListSandAutomations",
            &ListSandAutomationsRequest {
                sand_agent_id: agent_id.to_owned(),
            },
        )
    }

    fn create_sand_automation(&self, request: CreateAutomationRequest) -> Result<(), SandConnectError> {
        let _: CreateAutomationResponse =
            self.connect.unary(AUTOMATIONS_SERVICE, "CreateSandAutomation", &request)?;
        Ok(())
    }

    fn update_sand_automation(&self, request: UpdateAutomationRequest) -> Result<(), SandConnectError> {
        let _: UpdateAutomationResponse =
            self.connect.unary(AUTOMATIONS_SERVICE, "UpdateSandAutomation", &request)?;
        Ok(())
    }

    fn delete_sand_automation(&self, automation_id: &str) -> Result<(), SandConnectError> {
        let _: DeleteAutomationResponse = self.connect.unary(
            AUTOMATIONS_SERVICE,
            "DeleteSandAutomation",
            &DeleteAutomationRequest {
                automation_id: automation_id.to_owned(),
            },
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SchedulingEvidence {
    Known(BTreeSet<String>),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulingAuthority {
    pub desired_cloud_automation_ids: BTreeSet<String>,
    pub enabled_remote_automation_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileOutcome {
    pub converged: bool,
    pub scheduling_authority_changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowPruneReport {
    pub agent_id: String,
    pub automation_id: String,
    pub outcome: String,
    pub local_definition_state: String,
    pub local_definition_count: usize,
    pub desired_count: usize,
    pub remote_shadow_count: usize,
}

pub struct SandAutomationCloudSync {
    client: Arc<dyn CloudSyncClient>,
    scheduling_evidence_by_agent: BTreeMap<String, SchedulingEvidence>,
    last_authority_by_agent: BTreeMap<String, SchedulingAuthority>,
    last_successful_fingerprint_by_agent: BTreeMap<String, String>,
    failed_agent_ids: BTreeSet<String>,
    pending_agent_deletions: BTreeSet<String>,
    shadow_prune_reports: Vec<ShadowPruneReport>,
}

impl SandAutomationCloudSync {
    pub fn new(client: Arc<dyn CloudSyncClient>) -> Self {
        Self {
            client,
            scheduling_evidence_by_agent: BTreeMap::new(),
            last_authority_by_agent: BTreeMap::new(),
            last_successful_fingerprint_by_agent: BTreeMap::new(),
            failed_agent_ids: BTreeSet::new(),
            pending_agent_deletions: BTreeSet::new(),
            shadow_prune_reports: Vec::new(),
        }
    }

    pub fn should_schedule_locally(&self, agent_id: &str, automation: &ScheduledCloudAutomation) -> bool {
        if !is_server_schedulable(&automation.trigger) {
            return true;
        }
        let has_listener = trigger_members(&automation.trigger)
            .iter()
            .any(|member| member.get("type").and_then(Value::as_str) != Some("cron"));
        if !has_listener {
            return false;
        }
        match self.scheduling_evidence_by_agent.get(agent_id) {
            Some(SchedulingEvidence::Known(enabled)) => {
                !enabled.contains(&stable_automation_id(agent_id, &automation.id))
            }
            _ => false,
        }
    }

    pub fn reconcile_agent(
        &mut self,
        agent_id: &str,
        desired: impl IntoIterator<Item = CloudDefinition>,
    ) -> Result<ReconcileOutcome, SandConnectError> {
        let desired_by_id = desired
            .into_iter()
            .map(|definition| (definition.automation_id.clone(), definition))
            .collect::<BTreeMap<_, _>>();
        let fingerprint = desired_by_id
            .values()
            .map(|definition| format!("{}:{}", definition.automation_id, definition.marker))
            .collect::<Vec<_>>()
            .join("\0");
        if self
            .last_successful_fingerprint_by_agent
            .get(agent_id)
            .is_some_and(|previous| previous == &fingerprint)
        {
            return Ok(ReconcileOutcome {
                converged: true,
                scheduling_authority_changed: false,
            });
        }
        let desired_ids = desired_by_id.keys().cloned().collect::<BTreeSet<_>>();
        let initial = match self.client.list_sand_automations(agent_id) {
            Ok(initial) => initial,
            Err(error) => {
                self.record_failure(agent_id);
                return Err(error);
            }
        };
        let remote = remote_shadow_automations_by_id(&initial);
        if is_converged(&remote, &desired_by_id) {
            let changed = self.publish_known(agent_id, &initial, desired_ids);
            self.record_recovery(agent_id);
            self.last_successful_fingerprint_by_agent
                .insert(agent_id.to_owned(), fingerprint.clone());
            return Ok(ReconcileOutcome {
                converged: true,
                scheduling_authority_changed: changed,
            });
        }

        let mut mutation_failed = false;
        let remote_shadow_count = remote.len();
        for (automation_id, remote_automation) in &remote {
            if desired_by_id.contains_key(automation_id) {
                continue;
            }
            self.scheduling_evidence_by_agent
                .insert(agent_id.to_owned(), SchedulingEvidence::Unknown);
            let succeeded = match self
                .client
                .delete_sand_automation(&remote_automation.automation_id)
            {
                Ok(()) => true,
                Err(_) => {
                    mutation_failed = true;
                    self.record_failure(agent_id);
                    false
                }
            };
            self.shadow_prune_reports.push(ShadowPruneReport {
                agent_id: agent_id.to_owned(),
                automation_id: automation_id.clone(),
                outcome: if succeeded { "deleted".into() } else { "failed".into() },
                local_definition_state: "loaded".into(),
                local_definition_count: desired_by_id.len(),
                desired_count: desired_by_id.len(),
                remote_shadow_count,
            });
        }
        for (automation_id, definition) in &desired_by_id {
            match remote.get(automation_id) {
                None => {
                    self.scheduling_evidence_by_agent
                        .insert(agent_id.to_owned(), SchedulingEvidence::Unknown);
                    if self.client.create_sand_automation(CreateAutomationRequest {
                        name: definition.name.clone(),
                        workflow: Some(definition.workflow.clone()),
                        description: Some(definition.marker.clone()),
                        enabled: Some(definition.enabled),
                        sand_agent_id: Some(agent_id.to_owned()),
                        sand_automation_id: Some(automation_id.clone()),
                    }).is_err() {
                        mutation_failed = true;
                        self.record_failure(agent_id);
                    }
                }
                Some(remote)
                    if remote.description.as_deref() != Some(definition.marker.as_str())
                        || remote.enabled != definition.enabled =>
                {
                    self.scheduling_evidence_by_agent
                        .insert(agent_id.to_owned(), SchedulingEvidence::Unknown);
                    if self.client.update_sand_automation(UpdateAutomationRequest {
                        name: Some(definition.name.clone()),
                        workflow: Some(definition.workflow.clone()),
                        enabled: Some(definition.enabled),
                        description: Some(definition.marker.clone()),
                        automation_id: remote.automation_id.clone(),
                    }).is_err() {
                        mutation_failed = true;
                        self.record_failure(agent_id);
                    }
                }
                _ => {}
            }
        }

        let readback = match self.client.list_sand_automations(agent_id) {
            Ok(readback) => readback,
            Err(error) => {
                self.record_failure(agent_id);
                return Err(error);
            }
        };
        let changed = self.publish_known(agent_id, &readback, desired_ids);
        let converged = is_converged(&remote_shadow_automations_by_id(&readback), &desired_by_id);
        if converged {
            self.record_recovery(agent_id);
            self.last_successful_fingerprint_by_agent
                .insert(agent_id.to_owned(), fingerprint);
        } else if !mutation_failed {
            self.record_failure(agent_id);
        }
        Ok(ReconcileOutcome {
            converged,
            scheduling_authority_changed: changed,
        })
    }

    pub fn delete_agent(&mut self, agent_id: &str) -> Result<ReconcileOutcome, SandConnectError> {
        self.pending_agent_deletions.insert(agent_id.to_owned());
        let initial = match self.client.list_sand_automations(agent_id) {
            Ok(initial) => initial,
            Err(error) => {
                self.record_failure(agent_id);
                return Err(error);
            }
        };
        let remote = initial
            .workflows
            .iter()
            .filter_map(|entry| entry.workflow.as_ref())
            .cloned()
            .collect::<Vec<_>>();
        for automation in remote {
            self.scheduling_evidence_by_agent
                .insert(agent_id.to_owned(), SchedulingEvidence::Unknown);
            if self.client.delete_sand_automation(&automation.automation_id).is_err() {
                self.record_failure(agent_id);
            }
        }
        let readback = match self.client.list_sand_automations(agent_id) {
            Ok(readback) => readback,
            Err(error) => {
                self.record_failure(agent_id);
                return Err(error);
            }
        };
        let changed = self.publish_known(agent_id, &readback, BTreeSet::new());
        let converged = readback.workflows.iter().all(|entry| entry.workflow.is_none());
        if converged {
            self.pending_agent_deletions.remove(agent_id);
            self.record_recovery(agent_id);
            self.scheduling_evidence_by_agent.remove(agent_id);
            self.last_authority_by_agent.remove(agent_id);
            self.last_successful_fingerprint_by_agent.remove(agent_id);
        } else {
            self.record_failure(agent_id);
        }
        Ok(ReconcileOutcome {
            converged,
            scheduling_authority_changed: changed,
        })
    }

    pub fn retry_pending_agent_deletions(
        &mut self,
    ) -> Vec<(String, Result<ReconcileOutcome, SandConnectError>)> {
        let pending = self.pending_agent_deletions.iter().cloned().collect::<Vec<_>>();
        pending
            .into_iter()
            .map(|agent_id| {
                let outcome = self.delete_agent(&agent_id);
                (agent_id, outcome)
            })
            .collect()
    }

    pub fn pending_agent_deletions(&self) -> &BTreeSet<String> {
        &self.pending_agent_deletions
    }

    pub fn failed_agent_ids(&self) -> &BTreeSet<String> {
        &self.failed_agent_ids
    }

    pub fn drain_shadow_prune_reports(&mut self) -> Vec<ShadowPruneReport> {
        std::mem::take(&mut self.shadow_prune_reports)
    }

    fn record_failure(&mut self, agent_id: &str) {
        self.failed_agent_ids.insert(agent_id.to_owned());
        self.last_successful_fingerprint_by_agent.remove(agent_id);
    }

    fn record_recovery(&mut self, agent_id: &str) {
        self.failed_agent_ids.remove(agent_id);
    }

    fn publish_known(
        &mut self,
        agent_id: &str,
        response: &ListAutomationsResponse,
        desired_cloud_automation_ids: BTreeSet<String>,
    ) -> bool {
        let enabled_remote_automation_ids = enabled_remote_automation_ids(response);
        self.scheduling_evidence_by_agent.insert(
            agent_id.to_owned(),
            SchedulingEvidence::Known(enabled_remote_automation_ids.clone()),
        );
        let authority = SchedulingAuthority {
            desired_cloud_automation_ids,
            enabled_remote_automation_ids,
        };
        let changed = self.last_authority_by_agent.get(agent_id) != Some(&authority);
        if changed {
            self.last_authority_by_agent.insert(agent_id.to_owned(), authority);
        }
        changed
    }
}

fn remote_shadow_automations_by_id(
    response: &ListAutomationsResponse,
) -> BTreeMap<String, Automation> {
    response
        .workflows
        .iter()
        .filter_map(|entry: &AutomationWithOwner| entry.workflow.as_ref())
        .filter(|automation| {
            automation
                .description
                .as_deref()
                .is_some_and(|description| description.starts_with(SAND_SHADOW_MARKER_PREFIX))
        })
        .map(|automation| (automation.automation_id.clone(), automation.clone()))
        .collect()
}

fn enabled_remote_automation_ids(response: &ListAutomationsResponse) -> BTreeSet<String> {
    response
        .workflows
        .iter()
        .filter_map(|entry| entry.workflow.as_ref())
        .filter(|automation| automation.enabled)
        .map(|automation| automation.automation_id.clone())
        .collect()
}

fn is_converged(
    remote: &BTreeMap<String, Automation>,
    desired: &BTreeMap<String, CloudDefinition>,
) -> bool {
    remote.len() == desired.len()
        && desired.iter().all(|(automation_id, definition)| {
            remote.get(automation_id).is_some_and(|automation| {
                automation.description.as_deref() == Some(definition.marker.as_str())
                    && automation.enabled == definition.enabled
            })
        })
}
