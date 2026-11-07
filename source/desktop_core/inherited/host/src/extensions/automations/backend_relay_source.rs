use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::automations::automation_trigger::{
    GITHUB_EVENT_KINDS, TRIGGER_ANY_SCOPE, listener_matches_event,
};
use super::backend_transport::{AutomationsBackendError, AutomationsBackendTransport};
use super::listener_integrations::{ListenerScopeIssue, describe_scope_issues};

pub const DEFAULT_REGISTER_INTERVAL_MS: u64 = 5 * 60_000;
pub const DEGRADED_REGISTER_INTERVAL_MS: u64 = 60_000;
pub const ERROR_BACKOFF_MS: u64 = 30_000;

#[derive(Debug, Clone, PartialEq)]
pub struct BackendRelaySource {
    kind: String,
    listeners: Vec<Value>,
    started: bool,
}

impl BackendRelaySource {
    pub fn new(kind: impl Into<String>) -> Self {
        Self { kind: kind.into(), listeners: Vec::new(), started: false }
    }

    pub fn kind(&self) -> &str { &self.kind }
    pub fn is_started(&self) -> bool { self.started }
    pub fn listeners(&self) -> &[Value] { &self.listeners }

    pub fn set_listeners(&mut self, listeners: Vec<Value>) {
        self.listeners = listeners;
    }

    pub fn start(&mut self) { self.started = true; }
    pub fn stop(&mut self) { self.started = false; }

    pub fn accepts(
        &self,
        event: &Value,
        platform_matched: bool,
        admit_missing_subject: bool,
    ) -> bool {
        self.started && self.listeners.iter().any(|listener| {
            listener.get("type").and_then(Value::as_str) == Some(self.kind.as_str())
                && listener_matches_event(
                    listener,
                    event,
                    platform_matched,
                    admit_missing_subject,
                )
        })
    }
}

pub fn relay_subscription_request(slack: &[Value], github: &[Value]) -> Value {
    let mut slack_channels = BTreeSet::new();
    for listener in slack {
        let Some(channel) = listener.get("channel").and_then(Value::as_str) else { continue };
        slack_channels.insert(if channel == TRIGGER_ANY_SCOPE {
            TRIGGER_ANY_SCOPE.to_string()
        } else {
            channel.to_string()
        });
    }

    let mut github_repos = BTreeSet::new();
    let mut github_kinds = BTreeSet::new();
    for listener in github {
        if let Some(repo) = listener.get("repo").and_then(Value::as_str) {
            github_repos.insert(repo.to_ascii_lowercase());
        }
        for kind in listener
            .get("events")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            github_kinds.insert(kind.to_string());
        }
    }

    json!({
        "slackChannels": slack_channels.into_iter().collect::<Vec<_>>(),
        "githubRepos": github_repos.into_iter().collect::<Vec<_>>(),
        "githubKinds": github_kinds.into_iter().collect::<Vec<_>>(),
    })
}

pub fn map_relay_wire_event(event: &Value, now_ms: u64) -> Option<Value> {
    let object = event.as_object()?;
    let source = object.get("source")?.as_str()?;
    let timestamp_ms = object
        .get("timestampMs")
        .and_then(Value::as_u64)
        .unwrap_or(now_ms);

    if source == "slack" {
        let channel_name = object
            .get("channelName")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .or_else(|| object.get("channelId").and_then(Value::as_str))?;
        if channel_name.is_empty() {
            return None;
        }
        let channel_id = object.get("channelId").and_then(Value::as_str);
        let channel = if Some(channel_name) == channel_id || channel_name.starts_with('#') {
            channel_name.to_string()
        } else {
            format!("#{channel_name}")
        };
        let sender = object
            .get("senderSlackUserId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(|value| format!("@{value}"))
            .unwrap_or_else(|| "someone".into());

        let mut mapped = serde_json::Map::new();
        mapped.insert("source".into(), Value::String("slack".into()));
        mapped.insert("channel".into(), Value::String(channel));
        mapped.insert("sender".into(), Value::String(sender));
        mapped.insert(
            "text".into(),
            Value::String(object.get("text").and_then(Value::as_str).unwrap_or_default().into()),
        );
        mapped.insert("timestampMs".into(), Value::Number(timestamp_ms.into()));
        if let Some(ts) = object.get("ts").and_then(Value::as_str) {
            mapped.insert("ts".into(), Value::String(ts.into()));
        }
        if let Some(thread_ts) = object.get("threadTs").and_then(Value::as_str) {
            mapped.insert("threadTs".into(), Value::String(thread_ts.into()));
        }

        if object.get("kind").and_then(Value::as_str) == Some("reaction") {
            let emoji = object.get("reactionEmoji").and_then(Value::as_str)?;
            if emoji.is_empty() {
                return None;
            }
            mapped.insert("isMention".into(), Value::Bool(false));
            mapped.insert(
                "isSelf".into(),
                Value::Bool(object.get("isSelf").and_then(Value::as_bool).unwrap_or(false)),
            );
            mapped.insert(
                "reactionEmoji".into(),
                Value::String(format!(":{}:", emoji.replace(':', ""))),
            );
        } else {
            mapped.insert(
                "isMention".into(),
                Value::Bool(object.get("isMention").and_then(Value::as_bool).unwrap_or(false)),
            );
        }
        return Some(Value::Object(mapped));
    }

    if source != "github" {
        return None;
    }
    let repo = object.get("repo").and_then(Value::as_str)?;
    let kind = object.get("kind").and_then(Value::as_str)?;
    if repo.is_empty() || !GITHUB_EVENT_KINDS.contains(&kind) {
        return None;
    }
    let mut mapped = serde_json::Map::new();
    mapped.insert("source".into(), Value::String("github".into()));
    mapped.insert("repo".into(), Value::String(repo.into()));
    mapped.insert("kind".into(), Value::String(kind.into()));
    mapped.insert(
        "title".into(),
        Value::String(object.get("title").and_then(Value::as_str).unwrap_or_default().into()),
    );
    mapped.insert(
        "actor".into(),
        Value::String(object.get("actor").and_then(Value::as_str).unwrap_or("someone").into()),
    );
    mapped.insert("timestampMs".into(), Value::Number(timestamp_ms.into()));
    for field in ["url", "detail", "prOwner", "branch"] {
        if let Some(value) = object.get(field).and_then(Value::as_str).filter(|value| !value.is_empty()) {
            mapped.insert(field.into(), Value::String(value.into()));
        }
    }
    Some(Value::Object(mapped))
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RelayStatus {
    pub state: String,
    pub detail: Option<String>,
    pub scope_issues: Vec<ListenerScopeIssue>,
}

pub struct BackendRelayRuntime {
    transport: Arc<dyn AutomationsBackendTransport>,
    slack_listeners: Vec<Value>,
    github_listeners: Vec<Value>,
    registered_key: String,
    registered_at_ms: u64,
    degraded_streak: u32,
    backoff_until_ms: u64,
    pending_ack_ids: Vec<String>,
    pub slack_status: RelayStatus,
    pub github_status: RelayStatus,
}

impl BackendRelayRuntime {
    pub fn new(transport: Arc<dyn AutomationsBackendTransport>) -> Self {
        Self {
            transport,
            slack_listeners: Vec::new(),
            github_listeners: Vec::new(),
            registered_key: String::new(),
            registered_at_ms: 0,
            degraded_streak: 0,
            backoff_until_ms: 0,
            pending_ack_ids: Vec::new(),
            slack_status: RelayStatus { state: "idle".into(), detail: None, scope_issues: Vec::new() },
            github_status: RelayStatus { state: "idle".into(), detail: None, scope_issues: Vec::new() },
        }
    }

    pub fn set_listeners(&mut self, slack: Vec<Value>, github: Vec<Value>) {
        if self.slack_listeners == slack && self.github_listeners == github {
            return;
        }
        self.slack_listeners = slack;
        self.github_listeners = github;
        self.registered_key.clear();
    }

    pub fn pending_ack_ids(&self) -> &[String] {
        &self.pending_ack_ids
    }

    pub fn request_drain(&mut self) {
        self.backoff_until_ms = 0;
    }

    pub fn tick(
        &mut self,
        now_ms: u64,
        notify_connected: bool,
        safety_poll_enabled: bool,
        mut sink: impl FnMut(&Value) -> bool,
    ) -> Result<usize, AutomationsBackendError> {
        if now_ms < self.backoff_until_ms {
            return Ok(0);
        }

        let result: Result<usize, AutomationsBackendError> = (|| {
            self.ensure_registered(now_ms)?;
            if !notify_connected && !safety_poll_enabled && self.pending_ack_ids.is_empty() {
                return Ok(0);
            }

            let response = self.transport.post_json(
                "/sand/listener-events/poll",
                &json!({ "ackIds": self.pending_ack_ids }),
            )?;
            let events = response
                .get("events")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let previous_acks = self.pending_ack_ids.iter().cloned().collect::<BTreeSet<_>>();
            let mut next_acks = Vec::new();
            let mut delivered = 0usize;

            for wire in events {
                let Some(id) = wire.get("id").and_then(Value::as_str).map(str::to_string) else {
                    continue;
                };
                if previous_acks.contains(&id) {
                    next_acks.push(id);
                    continue;
                }
                match map_relay_wire_event(&wire, now_ms) {
                    None => next_acks.push(id),
                    Some(mapped) if sink(&mapped) => {
                        next_acks.push(id);
                        delivered += 1;
                    }
                    Some(_) => {}
                }
            }
            self.pending_ack_ids = next_acks;
            Ok(delivered)
        })();

        if let Err(error) = &result {
            self.registered_key.clear();
            self.backoff_until_ms = now_ms.saturating_add(ERROR_BACKOFF_MS);
            let detail = error.to_string();
            if !self.slack_listeners.is_empty() {
                self.slack_status = RelayStatus { state: "error".into(), detail: Some(detail.clone()), scope_issues: Vec::new() };
            }
            if !self.github_listeners.is_empty() {
                self.github_status = RelayStatus { state: "error".into(), detail: Some(detail), scope_issues: Vec::new() };
            }
        }
        result
    }

    fn ensure_registered(&mut self, now_ms: u64) -> Result<(), AutomationsBackendError> {
        let request = relay_subscription_request(&self.slack_listeners, &self.github_listeners);
        let key = serde_json::to_string(&request).unwrap_or_default();
        let multiplier = 1u64 << self.degraded_streak.saturating_sub(1).min(10);
        let stale_after = if self.degraded_streak == 0 {
            DEFAULT_REGISTER_INTERVAL_MS
        } else {
            DEFAULT_REGISTER_INTERVAL_MS.min(DEGRADED_REGISTER_INTERVAL_MS.saturating_mul(multiplier))
        };
        if key == self.registered_key
            && now_ms.saturating_sub(self.registered_at_ms) <= stale_after
        {
            return Ok(());
        }

        let response = self.transport.post_json("/sand/listener-subscriptions", &request)?;
        self.registered_key = key;
        self.registered_at_ms = now_ms;

        let slack_state = response.get("slack").and_then(Value::as_object);
        let github_state = response.get("github").and_then(Value::as_object);
        let mut slack_issues = Vec::new();
        for team in slack_state
            .and_then(|value| value.get("teams"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            for channel in team
                .get("channels")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if channel.get("isBotMember").and_then(Value::as_bool) == Some(false) {
                    if let Some(scope) = channel.get("input").and_then(Value::as_str) {
                        slack_issues.push(ListenerScopeIssue {
                            kind: "bot-not-in-channel".into(),
                            scope: scope.into(),
                        });
                    }
                }
            }
            for scope in team
                .get("unresolvedChannels")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                slack_issues.push(ListenerScopeIssue {
                    kind: "not-found".into(),
                    scope: scope.into(),
                });
            }
        }
        self.slack_status = if self.slack_listeners.is_empty() {
            RelayStatus { state: "idle".into(), detail: None, scope_issues: Vec::new() }
        } else if slack_state.and_then(|value| value.get("status")).and_then(Value::as_str) == Some("not-linked") {
            RelayStatus {
                state: "error".into(),
                detail: Some("Slack isn't connected to your Cursor account. Connect it to start listening.".into()),
                scope_issues: Vec::new(),
            }
        } else if slack_issues.is_empty() {
            RelayStatus { state: "listening".into(), detail: None, scope_issues: Vec::new() }
        } else {
            RelayStatus {
                state: "listening".into(),
                detail: Some(describe_scope_issues(&slack_issues)),
                scope_issues: slack_issues,
            }
        };

        let github_repos = github_state
            .and_then(|value| value.get("repos"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let failed_repos = github_repos
            .iter()
            .filter(|repo| repo.get("isSubscribed").and_then(Value::as_bool) == Some(false))
            .collect::<Vec<_>>();
        self.github_status = if self.github_listeners.is_empty() {
            RelayStatus { state: "idle".into(), detail: None, scope_issues: Vec::new() }
        } else if github_state.and_then(|value| value.get("status")).and_then(Value::as_str) == Some("not-connected") {
            RelayStatus {
                state: "error".into(),
                detail: Some("GitHub isn't connected to your Cursor account. Connect it to start listening.".into()),
                scope_issues: Vec::new(),
            }
        } else if failed_repos.is_empty() {
            RelayStatus { state: "listening".into(), detail: None, scope_issues: Vec::new() }
        } else {
            let detail = failed_repos
                .iter()
                .map(|repo| {
                    repo.get("detail")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!(
                            "Can't watch {}.",
                            repo.get("repo").and_then(Value::as_str).unwrap_or_default()
                        ))
                })
                .collect::<Vec<_>>()
                .join(" ");
            RelayStatus {
                state: if failed_repos.len() == github_repos.len() { "error".into() } else { "listening".into() },
                detail: Some(detail),
                scope_issues: Vec::new(),
            }
        };
        let degraded = self.slack_status.state == "error"
            || !self.slack_status.scope_issues.is_empty()
            || self.github_status.state == "error"
            || self.github_status.detail.is_some();
        self.degraded_streak = if degraded {
            self.degraded_streak.saturating_add(1)
        } else {
            0
        };
        Ok(())
    }
}

