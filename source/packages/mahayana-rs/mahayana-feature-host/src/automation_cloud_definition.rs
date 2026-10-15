use crate::automation_cloud_proto::{
    CronTrigger, GitCiCompletedEvent, GitIssueAssignedEvent, GitPullRequestEvent,
    GitPullRequestReviewCommentEvent, GitPullRequestReviewEvent,
    GitPullRequestReviewRequestedEvent, GitReviewThreadEvent, GitTrigger,
    LinearEndOfCycleEvent, LinearIssueCreatedEvent, LinearStatusChangedEvent, LinearTrigger,
    MicrosoftTeamsTrigger, PagerDutyIncidentAcknowledgedEvent, PagerDutyIncidentAnyEvent,
    PagerDutyIncidentEscalatedEvent, PagerDutyIncidentResolvedEvent,
    PagerDutyIncidentTriggeredEvent, PagerDutyTrigger, Prompt, SentryIssueAnyEvent,
    SentryIssueArchivedEvent, SentryIssueAssignedEvent, SentryIssueCreatedEvent,
    SentryIssueResolvedEvent, SentryIssueUnresolvedEvent, SentryTrigger,
    SlackAnyReactionAddedTrigger, SlackMentionTrigger, SlackReactionAddedTrigger, SlackTrigger,
    Trigger, Workflow,
};
use crate::automation_cloud_proto::{
    git_trigger, linear_trigger, pager_duty_trigger, sentry_trigger, trigger,
};
use mahayana_host_protocol::{AutomationSummary, AutomationTrigger, ListenerPlatform};
use prost::Message;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const GITHUB_SUBSCRIPTION_VERSION: &[u8] = b"github-subscriptions-v1\0";

fn filter_value<'a>(
    filters: Option<&'a BTreeMap<String, Value>>,
    key: &str,
) -> Option<&'a Value> {
    filters.and_then(|filters| filters.get(key))
}

fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => vec![value.clone()],
        _ => Vec::new(),
    }
}

fn string(filters: Option<&BTreeMap<String, Value>>, key: &str) -> String {
    filter_value(filters, key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn bool_value(filters: Option<&BTreeMap<String, Value>>, key: &str) -> bool {
    filter_value(filters, key)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn trigger_members(trigger: &AutomationTrigger) -> Vec<Value> {
    match trigger {
        AutomationTrigger::Schedule { schedule } => {
            vec![json!({"type": "cron", "schedule": schedule})]
        }
        AutomationTrigger::Event {
            source,
            event,
            filter,
            filters,
        } => {
            let filters = filters.as_ref();
            let member = match source {
                ListenerPlatform::Slack => {
                    let channel = string(filters, "channel");
                    let kind = match event.as_str() {
                        "mention" => "mention",
                        "reaction" => "reaction",
                        "keyword" => "keyword",
                        _ => "message",
                    };
                    let mut rule = json!({"kind": kind});
                    if kind == "reaction" {
                        rule["emoji"] = filter_value(filters, "emoji")
                            .cloned()
                            .unwrap_or_else(|| json!([]));
                        rule["bySelf"] = json!(bool_value(filters, "bySelf"));
                    } else if kind == "keyword" {
                        rule["keyword"] = json!(
                            filter_value(filters, "keyword")
                                .and_then(Value::as_str)
                                .or(filter.as_deref())
                                .unwrap_or_default()
                        );
                    }
                    json!({"type": "slack", "channel": channel, "match": rule})
                }
                ListenerPlatform::Github => {
                    let mut events = strings(filter_value(filters, "events"));
                    if events.is_empty() && event != "*" {
                        events.push(event.clone());
                    }
                    json!({
                        "type": "github",
                        "repo": string(filters, "repo"),
                        "events": events,
                        "userAllowlist": strings(filter_value(filters, "userAllowlist")),
                        "ciBranch": string(filters, "ciBranch"),
                    })
                }
                ListenerPlatform::Teams => json!({
                    "type": "microsoftTeams",
                    "tenantId": string(filters, "tenantId"),
                    "teamId": string(filters, "teamId"),
                    "teamIds": strings(filter_value(filters, "teamIds")),
                    "channelIds": strings(filter_value(filters, "channelIds")),
                    "messageContains": string(filters, "messageContains"),
                    "messageContainsIsRegex": bool_value(filters, "messageContainsIsRegex"),
                    "blockUnauthenticatedTeamsUsers": bool_value(filters, "blockUnauthenticatedTeamsUsers"),
                }),
                ListenerPlatform::Linear => {
                    let mut event_value = json!({"case": event});
                    if event == "statusChanged" {
                        event_value["statusIds"] = json!(strings(filter_value(filters, "statusIds")));
                    } else if event == "endOfCycle" {
                        event_value["cycleIds"] = json!(strings(filter_value(filters, "cycleIds")));
                    }
                    json!({
                        "type": "linear",
                        "event": event_value,
                        "projectIds": strings(filter_value(filters, "projectIds")),
                        "teamIds": strings(filter_value(filters, "teamIds")),
                    })
                }
                ListenerPlatform::Sentry => json!({
                    "type": "sentry",
                    "event": {"case": event},
                    "projectIds": strings(filter_value(filters, "projectIds")),
                }),
                ListenerPlatform::Pagerduty => json!({
                    "type": "pagerduty",
                    "event": {"case": event},
                    "serviceIds": strings(filter_value(filters, "serviceIds")),
                }),
                ListenerPlatform::Git => return Vec::new(),
            };
            vec![member]
        }
        AutomationTrigger::Group { listeners } => listeners
            .iter()
            .flat_map(trigger_members)
            .collect(),
    }
}

pub(crate) fn is_server_schedulable(trigger: &AutomationTrigger) -> bool {
    !trigger_members(trigger).iter().any(|listener| {
        listener.get("type").and_then(Value::as_str) == Some("slack")
            && listener
                .get("channel")
                .and_then(Value::as_str)
                .is_some_and(|channel| channel.starts_with('@'))
    })
}

fn split_schedule_time_zone(schedule: &str) -> (String, Option<String>) {
    let normalized = schedule.split_whitespace().collect::<Vec<_>>().join(" ");
    let Some((prefix, rest)) = normalized.split_once(' ') else {
        return (normalized, None);
    };
    let zone = prefix
        .strip_prefix("CRON_TZ=")
        .or_else(|| prefix.strip_prefix("TZ="));
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
        Some(zone) if !expanded.starts_with("@every") => {
            format!("CRON_TZ={zone} {expanded}")
        }
        _ => expanded,
    };
    Trigger {
        trigger: Some(trigger::Trigger::Cron(CronTrigger { cron })),
    }
}

fn slack_triggers(listener: &Value) -> Vec<Trigger> {
    let channel = listener
        .get("channel")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let channels = vec![channel];
    let rule = listener.get("match").unwrap_or(&Value::Null);
    match rule
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "mention" => vec![Trigger {
            trigger: Some(trigger::Trigger::SlackMention(SlackMentionTrigger {
                channel: String::new(),
                channels,
                block_unauthenticated_slack_users: false,
            })),
        }],
        "reaction" => {
            let only_owner_reactions = rule
                .get("bySelf")
                .and_then(Value::as_bool)
                .unwrap_or(false);
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
                message_contains: rule
                    .get("keyword")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
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

fn github_trigger(
    listener: &Value,
    event: &str,
    allowlist_override: Option<Vec<String>>,
) -> Trigger {
    let repo = format!(
        "https://github.com/{}",
        listener
            .get("repo")
            .and_then(Value::as_str)
            .unwrap_or_default()
    );
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
        "issue-assigned" => Some(git_trigger::Event::IssueAssigned(
            GitIssueAssignedEvent { repos },
        )),
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
        listener
            .get("ciBranch")
            .and_then(Value::as_str)
            .filter(|branch| !branch.is_empty()),
        ci_condition(&events),
    ) {
        result.push(github_ci_trigger(
            listener
                .get("repo")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            branch,
            condition,
        ));
    }
    result
}

fn teams_trigger(listener: &Value) -> Trigger {
    Trigger {
        trigger: Some(trigger::Trigger::MicrosoftTeamsTrigger(
            MicrosoftTeamsTrigger {
                tenant_id: listener
                    .get("tenantId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                team_id: listener
                    .get("teamId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                team_ids: strings(listener.get("teamIds")),
                channel_ids: strings(listener.get("channelIds")),
                message_contains: listener
                    .get("messageContains")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                message_contains_is_regex: listener
                    .get("messageContainsIsRegex")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                block_unauthenticated_teams_users: listener
                    .get("blockUnauthenticatedTeamsUsers")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
        )),
    }
}

fn linear_trigger(listener: &Value) -> Trigger {
    let event_value = listener.get("event").unwrap_or(&Value::Null);
    let event = match event_value
        .get("case")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "issueCreated" => Some(linear_trigger::Event::IssueCreated(
            LinearIssueCreatedEvent {},
        )),
        "statusChanged" => Some(linear_trigger::Event::StatusChanged(
            LinearStatusChangedEvent {
                status_ids: strings(event_value.get("statusIds")),
            },
        )),
        "endOfCycle" => Some(linear_trigger::Event::EndOfCycle(
            LinearEndOfCycleEvent {
                cycle_ids: strings(event_value.get("cycleIds")),
            },
        )),
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
    let event = match listener
        .get("event")
        .and_then(|event| event.get("case"))
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
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
    let event = match listener
        .get("event")
        .and_then(|event| event.get("case"))
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "incidentTriggered" => Some(pager_duty_trigger::Event::IncidentTriggered(
            PagerDutyIncidentTriggeredEvent {},
        )),
        "incidentAcknowledged" => Some(pager_duty_trigger::Event::IncidentAcknowledged(
            PagerDutyIncidentAcknowledgedEvent {},
        )),
        "incidentResolved" => Some(pager_duty_trigger::Event::IncidentResolved(
            PagerDutyIncidentResolvedEvent {},
        )),
        "incidentEscalated" => Some(pager_duty_trigger::Event::IncidentEscalated(
            PagerDutyIncidentEscalatedEvent {},
        )),
        "incidentAny" => Some(pager_duty_trigger::Event::IncidentAny(
            PagerDutyIncidentAnyEvent {},
        )),
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
    match listener
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "slack" => slack_triggers(listener),
        "github" => github_listener_triggers(listener),
        "microsoftTeams" => vec![teams_trigger(listener)],
        "linear" => vec![linear_trigger(listener)],
        "sentry" => vec![sentry_trigger(listener)],
        "pagerduty" => vec![pagerduty_trigger(listener)],
        _ => Vec::new(),
    }
}

fn union_allowlists(
    existing: Option<Vec<String>>,
    addition: Option<Vec<String>>,
) -> Option<Vec<String>> {
    let (Some(mut existing), Some(addition)) = (existing, addition) else {
        return None;
    };
    for login in addition {
        if !existing
            .iter()
            .any(|entry| entry.eq_ignore_ascii_case(&login))
        {
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
        let repo = listener
            .get("repo")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let repo_key = repo.to_ascii_lowercase();
        let events = strings(listener.get("events"));
        let ci_events = events
            .iter()
            .filter(|event| is_github_ci_event(event))
            .cloned()
            .collect::<Vec<_>>();
        if let Some(branch) = listener
            .get("ciBranch")
            .and_then(Value::as_str)
            .filter(|branch| !branch.is_empty())
        {
            if !ci_events.is_empty() {
                let key = format!("{repo_key}\0{branch}");
                if let Some((_, entry)) = ci_entries
                    .iter_mut()
                    .find(|(entry_key, _)| entry_key == &key)
                {
                    for event in ci_events {
                        if !entry.events.contains(&event) {
                            entry.events.push(event);
                        }
                    }
                } else {
                    ci_entries.push((
                        key,
                        CiEntry {
                            repo: repo.clone(),
                            branch: branch.to_owned(),
                            events: ci_events,
                        },
                    ));
                }
            }
        }

        let restriction = github_restriction(listener);
        let non_ci_events = events
            .into_iter()
            .filter(|event| !is_github_ci_event(event))
            .collect::<Vec<_>>();
        let by_event = allowlists.entry(repo_key.clone()).or_default();
        for event in &non_ci_events {
            let next = if by_event.contains_key(event) {
                union_allowlists(
                    by_event.get(event).cloned().flatten(),
                    restriction.clone(),
                )
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
                let repo_key = first
                    .listener
                    .get("repo")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let by_event = allowlists.get(&repo_key);
                for event in first.non_ci_events {
                    let override_allowlist = by_event
                        .and_then(|map| map.get(&event))
                        .cloned()
                        .flatten()
                        .unwrap_or_default();
                    result.push(github_trigger(
                        &first.listener,
                        &event,
                        Some(override_allowlist),
                    ));
                }
            }
        }
    }
    for (_, entry) in ci_entries {
        if let Some(condition) = ci_condition(&entry.events) {
            result.push(github_ci_trigger(
                &entry.repo,
                &entry.branch,
                condition,
            ));
        }
    }
    result
}

fn cloud_triggers(
    trigger_value: &AutomationTrigger,
    time_zone: Option<&str>,
) -> Option<Vec<Trigger>> {
    let members = trigger_members(trigger_value);
    if members.iter().any(|listener| {
        listener.get("type").and_then(Value::as_str) == Some("slack")
            && listener
                .get("channel")
                .and_then(Value::as_str)
                .is_some_and(|channel| channel.starts_with('@'))
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

pub(crate) const SAND_SHADOW_MARKER_PREFIX: &str = "sand-shadow:";

#[derive(Debug, Clone)]
pub(crate) struct AutomationCloudDefinition {
    pub automation_id: String,
    pub enabled: bool,
    pub hash: String,
    pub local_id: String,
    pub marker: String,
    pub name: String,
    pub workflow: Workflow,
}

pub(crate) fn stable_automation_id(agent_id: &str, local_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(agent_id.as_bytes());
    hasher.update([0]);
    hasher.update(local_id.as_bytes());
    let hex = format!("{:x}", hasher.finalize());
    let variant_value =
        (u8::from_str_radix(&hex[16..17], 16).expect("sha256 hex") & 3) | 8;
    format!(
        "{}-{}-5{}-{:x}{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        variant_value,
        &hex[17..20],
        &hex[20..32]
    )
}

pub(crate) fn automation_cloud_definition(
    agent_id: &str,
    automation: &AutomationSummary,
    time_zone: Option<&str>,
) -> Option<AutomationCloudDefinition> {
    let trigger = automation.trigger.clone().unwrap_or_else(|| {
        AutomationTrigger::Schedule {
            schedule: automation.schedule.clone(),
        }
    });
    let triggers = cloud_triggers(&trigger, time_zone)?;
    let workflow = Workflow {
        triggers,
        prompts: vec![Prompt {
            prompt: automation.prompt.clone(),
        }],
    };
    let mut hasher = Sha256::new();
    if workflow.triggers.iter().any(|wire_trigger| {
        matches!(
            wire_trigger.trigger.as_ref(),
            Some(trigger::Trigger::Git(_))
        )
    }) {
        hasher.update(GITHUB_SUBSCRIPTION_VERSION);
    }
    hasher.update(automation.id.as_bytes());
    hasher.update([0]);
    hasher.update(automation.name.as_bytes());
    hasher.update([0]);
    hasher.update(if automation.enabled {
        b"true".as_slice()
    } else {
        b"false".as_slice()
    });
    hasher.update([0]);
    hasher.update(workflow.encode_to_vec());
    let hash = format!("{:x}", hasher.finalize());
    Some(AutomationCloudDefinition {
        automation_id: stable_automation_id(agent_id, &automation.id),
        enabled: automation.enabled,
        marker: format!("{SAND_SHADOW_MARKER_PREFIX}{hash}"),
        hash,
        local_id: automation.id.clone(),
        name: automation.name.clone(),
        workflow,
    })
}

pub(crate) fn automation_cloud_definition_revision(
    automation: &AutomationSummary,
    time_zone: Option<&str>,
) -> Option<String> {
    let agent_id = automation
        .agent_id
        .as_deref()
        .unwrap_or("mahayana-assistant");
    automation_cloud_definition(agent_id, automation, time_zone)
        .map(|definition| definition.hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_desktop_schedule_revision_fixture() {
        let automation = AutomationSummary {
            id: "daily".into(),
            agent_id: None,
            name: "Daily".into(),
            prompt: "Review".into(),
            schedule: "@daily".into(),
            trigger: Some(AutomationTrigger::Schedule {
                schedule: "@daily".into(),
            }),
            enabled: true,
            created_at_ms: 1,
            raised_notices: Vec::new(),
            runs: Vec::new(),
            last_run_at_ms: None,
            next_run_at_ms: None,
        };
        let first = automation_cloud_definition_revision(&automation, Some("UTC"))
            .expect("revision");
        let second = automation_cloud_definition_revision(&automation, Some("UTC"))
            .expect("revision");
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn slack_direct_message_is_not_server_schedulable() {
        let trigger = AutomationTrigger::Event {
            source: ListenerPlatform::Slack,
            event: "message".into(),
            filter: None,
            filters: Some(BTreeMap::from([(
                "channel".into(),
                json!("@someone"),
            )])),
        };
        assert!(!is_server_schedulable(&trigger));
    }
}
