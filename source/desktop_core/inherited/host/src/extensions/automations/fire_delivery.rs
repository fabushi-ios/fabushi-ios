use serde_json::{Map, Value};

use crate::automations::automation_id::stable_automation_id;
use crate::automations::automation_trigger::{
    GITHUB_EVENT_KINDS, trigger_matches_event, trigger_members, trigger_schedule,
};
use crate::extensions::session::agent_session::AgentAutomationEntry;

use super::sand_automation_fire_consumer::{BackendAutomationFire, FireCompletion};

const ID_MAX: usize = 200;
const TEXT_MAX: usize = 4_000;
const TITLE_MAX: usize = 400;
const URL_MAX: usize = 600;
const STATUS_MAX: usize = 200;

#[derive(Debug, Clone, PartialEq)]
pub enum PreparedBackendFire {
    Schedule {
        agent_id: String,
        automation_id: String,
        run_uuid: String,
        scheduled_for_ms: Option<u64>,
    },
    Event {
        agent_id: String,
        automation_id: String,
        run_uuid: String,
        event: Value,
    },
    Complete {
        completion: FireCompletion,
        reason: &'static str,
    },
    Abandon {
        reason: &'static str,
    },
}

pub fn prepare_backend_fire(
    entries: &[AgentAutomationEntry],
    fire: &BackendAutomationFire,
    expected_definition_revision: impl Fn(&AgentAutomationEntry) -> Option<String>,
) -> PreparedBackendFire {
    let target = entries.iter().find(|entry| {
        entry.agent_id == fire.sand_agent_id
            && stable_automation_id(&entry.agent_id, &entry.automation.id) == fire.automation_id
    });
    let Some(target) = target else {
        return rejected("automation_missing", "Automation not found on the Sand box");
    };

    if let Some(existing) = existing_run_completion(target, &fire.id) {
        return PreparedBackendFire::Complete {
            completion: existing,
            reason: "existing_run",
        };
    }
    if !target.automation.is_enabled {
        return rejected("automation_disabled", "Automation is disabled on the Sand box");
    }

    if fire.definition_revision.is_none()
        && fire.event.is_none()
        && admits_bare_fire(&target.automation.trigger, fire)
        && target
            .automation
            .last_run_at
            .is_some_and(|last_run_at| last_run_at >= fire.timestamp_ms as f64)
    {
        return rejected(
            "slot_already_covered",
            "Legacy automation fire was already covered on the Sand box",
        );
    }

    if let Some(revision) = fire.definition_revision.as_deref() {
        match expected_definition_revision(target) {
            Some(expected) if expected == revision => {}
            Some(_) => {
                return rejected(
                    "definition_changed",
                    "Automation definition changed on the Sand box",
                );
            }
            None => {
                return PreparedBackendFire::Abandon {
                    reason: "definition_revision_unavailable",
                };
            }
        }
    }

    if let Some(raw_event) = fire.event.as_ref() {
        let Some(parsed) = parse_fire_trigger_event(raw_event, fire.timestamp_ms) else {
            return rejected(
                "event_unrecognized",
                "Event context not recognized by the Sand box",
            );
        };
        if !fire_event_matches_trigger(&target.automation.trigger, &parsed, raw_event) {
            return rejected(
                "trigger_no_longer_matches",
                "Automation trigger no longer matches the event",
            );
        }
        return PreparedBackendFire::Event {
            agent_id: target.agent_id.clone(),
            automation_id: target.automation.id.clone(),
            run_uuid: fire.id.clone(),
            event: parsed,
        };
    }

    if !admits_bare_fire(&target.automation.trigger, fire) {
        return rejected(
            "missing_event_context",
            "Fire carried no event context on the Sand box",
        );
    }

    PreparedBackendFire::Schedule {
        agent_id: target.agent_id.clone(),
        automation_id: target.automation.id.clone(),
        run_uuid: fire.id.clone(),
        scheduled_for_ms: fire.scheduled_for_ms,
    }
}

fn rejected(reason: &'static str, message: &str) -> PreparedBackendFire {
    PreparedBackendFire::Complete {
        completion: FireCompletion::failed(message),
        reason,
    }
}

fn existing_run_completion(
    target: &AgentAutomationEntry,
    run_uuid: &str,
) -> Option<FireCompletion> {
    let run = target.automation.runs.iter().find(|run| {
        run.id == run_uuid
            || run
                .coalesced_run_ids
                .as_ref()
                .is_some_and(|ids| ids.iter().any(|id| id == run_uuid))
    })?;
    if run.status == "running" {
        return None;
    }
    if run.status == "ok" {
        Some(FireCompletion::succeeded())
    } else {
        Some(FireCompletion::failed(
            run.detail
                .clone()
                .unwrap_or_else(|| "Automation run failed on the Sand box".into()),
        ))
    }
}

pub fn admits_bare_fire(trigger: &Value, fire: &BackendAutomationFire) -> bool {
    let has_schedule = trigger_schedule(trigger).is_some();
    let has_event_listener = trigger_members(trigger)
        .iter()
        .any(|member| member.get("type").and_then(Value::as_str) != Some("cron"));
    has_schedule && (!has_event_listener || fire.scheduled_for_ms.is_some())
}

pub fn fire_event_matches_trigger(trigger: &Value, parsed: &Value, raw: &Value) -> bool {
    if trigger_matches_event(trigger, parsed, true, true) {
        return true;
    }
    if parsed.get("source").and_then(Value::as_str) != Some("slack") {
        return false;
    }
    let Some(channel_id) = bounded_string(raw.get("channelId"), ID_MAX) else {
        return false;
    };
    if parsed.get("channel").and_then(Value::as_str) == Some(channel_id.as_str()) {
        return false;
    }
    let mut alternate = parsed.clone();
    alternate["channel"] = Value::String(channel_id);
    trigger_matches_event(trigger, &alternate, true, true)
}

pub fn parse_fire_trigger_event(value: &Value, fallback_timestamp_ms: u64) -> Option<Value> {
    let object = value.as_object()?;
    let source = object.get("source")?.as_str()?;
    let timestamp_ms = object
        .get("timestampMs")
        .and_then(Value::as_u64)
        .unwrap_or(fallback_timestamp_ms);

    match source {
        "slack" => parse_slack(object, timestamp_ms),
        "github" => parse_github(object, timestamp_ms),
        "microsoftTeams" => parse_teams(object, timestamp_ms),
        "linear" => parse_linear(object, timestamp_ms),
        "sentry" => parse_sentry(object, timestamp_ms),
        "pagerduty" => parse_pagerduty(object, timestamp_ms),
        _ => None,
    }
}

fn parse_slack(object: &Map<String, Value>, timestamp_ms: u64) -> Option<Value> {
    let channel = object.get("channel")?.as_str()?;
    if channel.is_empty() {
        return None;
    }
    let mut out = Map::new();
    out.insert("source".into(), Value::String("slack".into()));
    out.insert("channel".into(), Value::String(channel.into()));
    out.insert(
        "sender".into(),
        Value::String(
            object
                .get("sender")
                .and_then(Value::as_str)
                .unwrap_or("someone")
                .into(),
        ),
    );
    out.insert(
        "text".into(),
        Value::String(
            object
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
        ),
    );
    out.insert(
        "isMention".into(),
        Value::Bool(
            object
                .get("isMention")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ),
    );
    if object.get("isSelf").and_then(Value::as_bool) == Some(true) {
        out.insert("isSelf".into(), Value::Bool(true));
    }
    for field in ["reactionEmoji", "ts", "threadTs"] {
        if let Some(value) = object
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            out.insert(field.into(), Value::String(value.into()));
        }
    }
    out.insert("timestampMs".into(), Value::Number(timestamp_ms.into()));
    Some(Value::Object(out))
}

fn parse_github(object: &Map<String, Value>, timestamp_ms: u64) -> Option<Value> {
    let repo = object.get("repo")?.as_str()?;
    let kind = object.get("kind")?.as_str()?;
    if repo.is_empty() || !GITHUB_EVENT_KINDS.contains(&kind) {
        return None;
    }
    let mut out = Map::new();
    out.insert("source".into(), Value::String("github".into()));
    out.insert("repo".into(), Value::String(repo.into()));
    out.insert("kind".into(), Value::String(kind.into()));
    out.insert(
        "title".into(),
        Value::String(
            object
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
        ),
    );
    out.insert(
        "actor".into(),
        Value::String(
            object
                .get("actor")
                .and_then(Value::as_str)
                .unwrap_or("someone")
                .into(),
        ),
    );
    for field in ["url", "detail", "prOwner", "branch"] {
        if let Some(value) = object
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            out.insert(field.into(), Value::String(value.into()));
        }
    }
    out.insert("timestampMs".into(), Value::Number(timestamp_ms.into()));
    Some(Value::Object(out))
}

fn parse_teams(object: &Map<String, Value>, timestamp_ms: u64) -> Option<Value> {
    if oversized(
        object,
        &[
            ("tenantId", ID_MAX),
            ("teamId", ID_MAX),
            ("channelId", ID_MAX),
            ("text", TEXT_MAX),
            ("aadObjectId", ID_MAX),
            ("activityId", ID_MAX),
            ("rootMessageId", ID_MAX),
        ],
    ) {
        return None;
    }
    let tenant_id = bounded_string(object.get("tenantId"), ID_MAX)?;
    let team_id = bounded_string(object.get("teamId"), ID_MAX)?;
    let channel_id = bounded_string(object.get("channelId"), ID_MAX)?;
    let mut out = Map::new();
    out.insert("source".into(), Value::String("microsoftTeams".into()));
    out.insert("tenantId".into(), Value::String(tenant_id));
    out.insert("teamId".into(), Value::String(team_id));
    out.insert("channelId".into(), Value::String(channel_id));
    out.insert(
        "text".into(),
        Value::String(
            object
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
        ),
    );
    out.insert(
        "aadObjectId".into(),
        Value::String(
            bounded_string(object.get("aadObjectId"), ID_MAX).unwrap_or_else(|| "unknown".into()),
        ),
    );
    out.insert(
        "activityId".into(),
        Value::String(
            bounded_string(object.get("activityId"), ID_MAX).unwrap_or_else(|| "unknown".into()),
        ),
    );
    if let Some(root_message_id) = bounded_string(object.get("rootMessageId"), ID_MAX) {
        out.insert("rootMessageId".into(), Value::String(root_message_id));
    }
    out.insert("timestampMs".into(), Value::Number(timestamp_ms.into()));
    Some(Value::Object(out))
}

fn parse_linear(object: &Map<String, Value>, timestamp_ms: u64) -> Option<Value> {
    let event = object.get("event")?.as_str()?;
    if !matches!(event, "issueCreated" | "statusChanged" | "endOfCycle")
        || oversized(
            object,
            &[
                ("issueIdentifier", ID_MAX),
                ("title", TITLE_MAX),
                ("url", URL_MAX),
                ("status", STATUS_MAX),
                ("projectId", ID_MAX),
                ("teamId", ID_MAX),
                ("statusId", ID_MAX),
                ("cycleId", ID_MAX),
                ("cycleName", TITLE_MAX),
            ],
        )
    {
        return None;
    }
    let mut out = Map::new();
    out.insert("source".into(), Value::String("linear".into()));
    out.insert("event".into(), Value::String(event.into()));
    copy_bounded_fields(
        object,
        &mut out,
        &[
            ("issueIdentifier", ID_MAX),
            ("title", TITLE_MAX),
            ("url", URL_MAX),
            ("status", STATUS_MAX),
            ("projectId", ID_MAX),
            ("teamId", ID_MAX),
            ("statusId", ID_MAX),
            ("cycleId", ID_MAX),
            ("cycleName", TITLE_MAX),
        ],
    );
    out.insert("timestampMs".into(), Value::Number(timestamp_ms.into()));
    Some(Value::Object(out))
}

fn parse_sentry(object: &Map<String, Value>, timestamp_ms: u64) -> Option<Value> {
    let event = object.get("event")?.as_str()?;
    if !matches!(
        event,
        "issueCreated" | "issueResolved" | "issueAssigned" | "issueArchived" | "issueUnresolved"
    ) || oversized(
        object,
        &[
            ("issueId", ID_MAX),
            ("shortId", ID_MAX),
            ("title", TITLE_MAX),
            ("projectId", ID_MAX),
            ("url", URL_MAX),
            ("projectSlug", ID_MAX),
            ("status", STATUS_MAX),
            ("substatus", STATUS_MAX),
        ],
    ) {
        return None;
    }
    let issue_id = bounded_string(object.get("issueId"), ID_MAX)?;
    let short_id = bounded_string(object.get("shortId"), ID_MAX)?;
    let title = bounded_string(object.get("title"), TITLE_MAX)?;
    let mut out = Map::new();
    out.insert("source".into(), Value::String("sentry".into()));
    out.insert("event".into(), Value::String(event.into()));
    out.insert("issueId".into(), Value::String(issue_id));
    out.insert("shortId".into(), Value::String(short_id));
    out.insert("title".into(), Value::String(title));
    copy_bounded_fields(
        object,
        &mut out,
        &[
            ("projectId", ID_MAX),
            ("url", URL_MAX),
            ("projectSlug", ID_MAX),
            ("status", STATUS_MAX),
            ("substatus", STATUS_MAX),
        ],
    );
    out.insert("timestampMs".into(), Value::Number(timestamp_ms.into()));
    Some(Value::Object(out))
}

fn parse_pagerduty(object: &Map<String, Value>, timestamp_ms: u64) -> Option<Value> {
    let event = object.get("event")?.as_str()?;
    if !matches!(
        event,
        "incidentTriggered" | "incidentAcknowledged" | "incidentResolved" | "incidentEscalated"
    ) || oversized(
        object,
        &[
            ("incidentId", ID_MAX),
            ("title", TITLE_MAX),
            ("status", STATUS_MAX),
            ("serviceId", ID_MAX),
            ("serviceName", ID_MAX),
            ("url", URL_MAX),
        ],
    ) {
        return None;
    }
    let incident_id = bounded_string(object.get("incidentId"), ID_MAX)?;
    let title = bounded_string(object.get("title"), TITLE_MAX)?;
    let status = bounded_string(object.get("status"), STATUS_MAX)?;
    let service_id = bounded_string(object.get("serviceId"), ID_MAX)?;
    let mut out = Map::new();
    out.insert("source".into(), Value::String("pagerduty".into()));
    out.insert("event".into(), Value::String(event.into()));
    out.insert("incidentId".into(), Value::String(incident_id));
    out.insert("title".into(), Value::String(title));
    out.insert("status".into(), Value::String(status));
    out.insert("serviceId".into(), Value::String(service_id));
    copy_bounded_fields(
        object,
        &mut out,
        &[("serviceName", ID_MAX), ("url", URL_MAX)],
    );
    out.insert("timestampMs".into(), Value::Number(timestamp_ms.into()));
    Some(Value::Object(out))
}

fn oversized(object: &Map<String, Value>, fields: &[(&str, usize)]) -> bool {
    fields.iter().any(|(name, max)| {
        object
            .get(*name)
            .and_then(Value::as_str)
            .is_some_and(|value| value.len() > *max)
    })
}

fn bounded_string(value: Option<&Value>, max: usize) -> Option<String> {
    let value = value?.as_str()?;
    (!value.is_empty() && value.len() <= max).then(|| value.to_string())
}

fn copy_bounded_fields(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    fields: &[(&str, usize)],
) {
    for (name, max) in fields {
        if let Some(value) = bounded_string(source.get(*name), *max) {
            target.insert((*name).into(), Value::String(value));
        }
    }
}
