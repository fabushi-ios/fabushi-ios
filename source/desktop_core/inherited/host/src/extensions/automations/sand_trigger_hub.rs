use std::collections::BTreeMap;

use serde_json::Value;

use crate::automations::automation_trigger::{trigger_matches_event, trigger_members};

#[derive(Debug, Clone, PartialEq)]
pub struct ScheduledAutomation {
    pub agent_id: String,
    pub automation_id: String,
    pub is_enabled: bool,
    pub trigger: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerFire {
    pub agent_id: String,
    pub automation_id: String,
}

pub fn desired_listeners_by_kind(
    scheduled: &[ScheduledAutomation],
    should_schedule_locally: impl Fn(&str, &ScheduledAutomation) -> bool,
) -> BTreeMap<String, Vec<Value>> {
    let mut by_kind = BTreeMap::<String, Vec<Value>>::new();
    for entry in scheduled {
        if !entry.is_enabled || !should_schedule_locally(&entry.agent_id, entry) {
            continue;
        }
        for listener in trigger_members(&entry.trigger) {
            let Some(kind) = listener.get("type").and_then(Value::as_str) else {
                continue;
            };
            if kind != "cron" {
                by_kind.entry(kind.to_string()).or_default().push(listener);
            }
        }
    }
    by_kind
}

pub fn matching_fires(
    scheduled: &[ScheduledAutomation],
    event: &Value,
    is_ready: bool,
    is_stopped: bool,
    platform_matched: bool,
    should_schedule_locally: impl Fn(&str, &ScheduledAutomation) -> bool,
) -> Vec<TriggerFire> {
    if is_stopped || !is_ready {
        return Vec::new();
    }

    scheduled
        .iter()
        .filter(|entry| entry.is_enabled)
        .filter(|entry| should_schedule_locally(&entry.agent_id, entry))
        .filter(|entry| trigger_matches_event(&entry.trigger, event, platform_matched, false))
        .map(|entry| TriggerFire {
            agent_id: entry.agent_id.clone(),
            automation_id: entry.automation_id.clone(),
        })
        .collect()
}
