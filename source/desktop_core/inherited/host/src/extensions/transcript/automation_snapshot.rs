use std::collections::BTreeMap;

use crate::automations::automation::AutomationRecord;
use crate::automations::automation_trigger::trigger_identity;

#[derive(Debug, Clone, PartialEq)]
pub struct AutomationSnapshot {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub trigger: String,
    pub trigger_type: String,
    pub schedule: String,
    pub is_enabled: bool,
    pub created_at: f64,
    pub recorded_run_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomationAction {
    Updated,
    Enabled,
    Disabled,
}

pub fn snapshot_automations(
    automations: &[AutomationRecord],
) -> BTreeMap<String, AutomationSnapshot> {
    automations
        .iter()
        .map(|automation| {
            let trigger_type = automation
                .trigger
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            (
                automation.id.clone(),
                AutomationSnapshot {
                    id: automation.id.clone(),
                    name: automation.name.clone(),
                    prompt: automation.prompt.clone(),
                    trigger: trigger_identity(&automation.trigger),
                    trigger_type,
                    schedule: automation.schedule.clone(),
                    is_enabled: automation.is_enabled,
                    created_at: automation.created_at,
                    recorded_run_count: automation.runs.len(),
                },
            )
        })
        .collect()
}

pub fn diff_automation_action(
    before: &AutomationSnapshot,
    after: &AutomationSnapshot,
) -> Option<AutomationAction> {
    if before.name != after.name
        || before.prompt != after.prompt
        || before.trigger != after.trigger
    {
        return Some(AutomationAction::Updated);
    }
    if before.is_enabled != after.is_enabled {
        return Some(if after.is_enabled {
            AutomationAction::Enabled
        } else {
            AutomationAction::Disabled
        });
    }
    None
}
