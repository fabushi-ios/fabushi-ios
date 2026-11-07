use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub struct BackendCloudTrigger {
    pub case: String,
    pub value: Value,
}

pub fn backend_cloud_trigger(trigger: &Value) -> Option<BackendCloudTrigger> {
    match trigger.get("type").and_then(Value::as_str)? {
        "microsoftTeams" => Some(BackendCloudTrigger {
            case: "microsoftTeamsTrigger".into(),
            value: json!({
                "tenantId": trigger.get("tenantId").and_then(Value::as_str).unwrap_or_default(),
                "teamId": trigger.get("teamId").and_then(Value::as_str).unwrap_or_default(),
                "teamIds": trigger.get("teamIds").and_then(Value::as_array).cloned().unwrap_or_default(),
                "channelIds": trigger.get("channelIds").and_then(Value::as_array).cloned().unwrap_or_default(),
                "messageContains": trigger.get("messageContains").and_then(Value::as_str).unwrap_or_default(),
                "messageContainsIsRegex": trigger.get("messageContainsIsRegex").and_then(Value::as_bool).unwrap_or(false),
                "blockUnauthenticatedTeamsUsers": trigger.get("blockUnauthenticatedTeamsUsers").and_then(Value::as_bool).unwrap_or(false),
            }),
        }),
        "linear" => Some(BackendCloudTrigger {
            case: "linear".into(),
            value: json!({
                "event": trigger.get("event").cloned().unwrap_or(Value::Null),
                "projectIds": trigger.get("projectIds").and_then(Value::as_array).cloned().unwrap_or_default(),
                "teamIds": trigger.get("teamIds").and_then(Value::as_array).cloned().unwrap_or_default(),
            }),
        }),
        "sentry" => Some(BackendCloudTrigger {
            case: "sentry".into(),
            value: json!({
                "event": trigger.get("event").cloned().unwrap_or(Value::Null),
                "projectIds": trigger.get("projectIds").and_then(Value::as_array).cloned().unwrap_or_default(),
            }),
        }),
        "pagerduty" => Some(BackendCloudTrigger {
            case: "pagerduty".into(),
            value: json!({
                "event": trigger.get("event").cloned().unwrap_or(Value::Null),
                "serviceIds": trigger.get("serviceIds").and_then(Value::as_array).cloned().unwrap_or_default(),
            }),
        }),
        _ => None,
    }
}
