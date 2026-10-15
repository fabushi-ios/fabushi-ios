// Desktop automation policy parity for the existing account-scoped FeatureHost owner.
// This file defines normalization/limits and renders Host-owned guidance only;
// it does not create a second automation store or scheduler.

const AUTOMATION_MAX_NAME_LENGTH: usize = 80;
const AUTOMATION_MAX_PER_AGENT: usize = 50;
const AUTOMATION_MAX_RUN_HISTORY: usize = 20;
const AUTOMATION_MAX_RUN_DETAIL_LENGTH: usize = 300;
const MAX_EVENTS_IN_AUTOMATION_WAKE: usize = 25;
const AUTOMATION_PROMPT_GUIDANCE_VERSION: &str = "backend_triggers_v4";

fn normalize_automation_name(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(AUTOMATION_MAX_NAME_LENGTH)
        .collect()
}

fn normalize_automation_prompt(prompt: &str) -> String {
    prompt.trim().to_string()
}

fn clamp_automation_run_detail(detail: Option<&str>) -> Option<String> {
    let value = detail?.trim();
    (!value.is_empty()).then(|| {
        value
            .chars()
            .take(AUTOMATION_MAX_RUN_DETAIL_LENGTH)
            .collect()
    })
}

fn clamp_automation_coalesced_ids(values: &[String]) -> Vec<String> {
    values
        .iter()
        .filter(|value| !value.trim().is_empty())
        .take(MAX_EVENTS_IN_AUTOMATION_WAKE)
        .cloned()
        .collect()
}

fn automation_trigger_guidance_description(trigger: &AutomationTrigger) -> String {
    match trigger {
        AutomationTrigger::Schedule { schedule } => format!("Scheduled: {schedule}"),
        AutomationTrigger::Event {
            source,
            event,
            filter,
            filters,
        } => {
            let mut parts = vec![
                format!("{source:?}").to_ascii_lowercase(),
                event.trim().to_string(),
            ];
            if let Some(filter) = filter.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
                parts.push(filter.to_string());
            }
            if let Some(filters) = filters {
                for (key, value) in filters {
                    let rendered = match value {
                        Value::Null => String::new(),
                        Value::String(value) => value.trim().to_string(),
                        Value::Array(values) => values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .collect::<Vec<_>>()
                            .join(", "),
                        other => other.to_string(),
                    };
                    if !rendered.is_empty() && rendered != "{}" && rendered != "[]" {
                        parts.push(format!("{key}={rendered}"));
                    }
                }
            }
            parts.join(" · ")
        }
        AutomationTrigger::Group { listeners } => {
            format!("{} event listeners", listeners.len())
        }
    }
}

fn render_automations_system_guidance(
    automations: &[AutomationSummary],
    time_zone: Option<&str>,
) -> String {
    let zone_note = time_zone
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|zone| format!("the user's local time ({zone})"))
        .unwrap_or_else(|| "the user's local time".to_string());
    let mut lines = vec![
        "## Routines".to_string(),
        format!("Routine policy version: {AUTOMATION_PROMPT_GUIDANCE_VERSION}."),
        "Routines are durable standing orders: a saved prompt plus either a schedule or an outside-event listener. They can run while the user is away.".to_string(),
        "Definitions are owned by the account-scoped Host automation store. Inspect and mutate them through the existing routine APIs; never create or edit a second scheduler/store.".to_string(),
        "Treat recurring, time-based, monitoring, reminder, digest, and \"tell me when\" requests as routine candidates. Create one when intent is unambiguous; otherwise propose it briefly instead of pretending to remain awake.".to_string(),
        "Write saved prompts as durable intent, not frozen tool-call arguments. Connector/MCP schemas may change between runs, so discover the current tool contract when a run fires.".to_string(),
        format!("Cron schedules use five fields (minute hour day-of-month month day-of-week) interpreted in {zone_note}. @hourly/@daily/@weekly/@monthly and @every intervals are supported; CRON_TZ=<IANA zone> pins a schedule to a fixed zone."),
        "When the user gives an hour but no minute, preserve the current local minute; use minute 0 only when they explicitly ask for the top of the hour. Prefer the least-frequent cadence that still makes the result useful.".to_string(),
        "For vague recurring work, default to weekday daytime hours in the user's zone rather than nights and weekends unless the request genuinely requires seven-day or overnight execution.".to_string(),
        "Use event listeners instead of polling when the platform event exists. Supported listener families include Slack, GitHub, Microsoft Teams, Linear, Sentry, PagerDuty, and OR-groups.".to_string(),
        "Slack listeners may target a channel, DM, or wildcard and can match messages, mentions, keywords, or reactions. Surface channel access/invite requirements when relevant.".to_string(),
        "GitHub listeners target one repository and explicit event kinds. CI passed/failed listeners require a concrete branch; user allowlists filter human-driven events but never CI. Confirm actual GitHub logins instead of guessing display names.".to_string(),
        "A listener and a schedule are alternatives, not two triggers on one routine. Make finite watches self-expiring and pause repeated authentication failures rather than repeatedly waking and failing.".to_string(),
        "Routine writes can require approval because they act while the user is away. Respect the existing approval flow and never retry a denied write with different wording.".to_string(),
        format!("Canonical limits: name={AUTOMATION_MAX_NAME_LENGTH}, per-agent={AUTOMATION_MAX_PER_AGENT}, history={AUTOMATION_MAX_RUN_HISTORY}, detail={AUTOMATION_MAX_RUN_DETAIL_LENGTH}, coalesced-events={MAX_EVENTS_IN_AUTOMATION_WAKE}."),
    ];
    if automations.is_empty() {
        lines.push("No routines yet.".into());
    } else {
        lines.push("Current routines:".into());
        for automation in automations.iter().take(AUTOMATION_MAX_PER_AGENT) {
            let state = if automation.enabled { "enabled" } else { "paused" };
            let description = automation
                .trigger
                .as_ref()
                .map(automation_trigger_guidance_description)
                .unwrap_or_else(|| automation.schedule.clone());
            lines.push(format!(
                "- {} [{}] — {}; folder {}",
                automation.name, state, description, automation.id
            ));
        }
    }
    lines.join("\n")
}

impl FeatureHostController {
    fn create_automations_system_guidance(
        &self,
        agent_id: Option<&str>,
    ) -> Result<String, FeatureHostError> {
        let owner = agent_id.unwrap_or("mahayana-assistant");
        let time_zone = self.host_settings_user_timezone_direct()?;
        let state = self.state()?;
        let automations = state
            .automations
            .values()
            .filter(|automation| {
                automation
                    .agent_id
                    .as_deref()
                    .unwrap_or("mahayana-assistant")
                    == owner
            })
            .take(AUTOMATION_MAX_PER_AGENT)
            .cloned()
            .collect::<Vec<_>>();
        Ok(render_automations_system_guidance(
            &automations,
            time_zone.as_deref(),
        ))
    }
}

#[cfg(test)]
mod automation_policy_tests {
    use super::*;

    fn routine(id: &str, agent_id: Option<&str>) -> AutomationSummary {
        AutomationSummary {
            id: id.into(),
            agent_id: agent_id.map(str::to_string),
            name: format!("Routine {id}"),
            prompt: " durable prompt ".into(),
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
        }
    }

    #[test]
    fn normalization_and_limits_match_desktop_contract() {
        assert_eq!(
            normalize_automation_name("  a   b  "),
            "a b"
        );
        assert_eq!(
            normalize_automation_name(&"x".repeat(AUTOMATION_MAX_NAME_LENGTH + 5)).chars().count(),
            AUTOMATION_MAX_NAME_LENGTH
        );
        assert_eq!(normalize_automation_prompt("  hello  "), "hello");
        assert_eq!(
            clamp_automation_run_detail(Some(&"d".repeat(AUTOMATION_MAX_RUN_DETAIL_LENGTH + 5)))
                .unwrap()
                .chars()
                .count(),
            AUTOMATION_MAX_RUN_DETAIL_LENGTH
        );
        let ids = (0..(MAX_EVENTS_IN_AUTOMATION_WAKE + 3))
            .map(|index| format!("run-{index}"))
            .collect::<Vec<_>>();
        assert_eq!(
            clamp_automation_coalesced_ids(&ids).len(),
            MAX_EVENTS_IN_AUTOMATION_WAKE
        );
        assert_eq!(AUTOMATION_MAX_PER_AGENT, 50);
        assert_eq!(AUTOMATION_MAX_RUN_HISTORY, 20);
    }

    #[test]
    fn guidance_is_host_owned_and_describes_current_routines() {
        let guidance = render_automations_system_guidance(
            &[routine("daily", Some("research"))],
            Some("America/Los_Angeles"),
        );
        assert!(guidance.contains("account-scoped Host automation store"));
        assert!(guidance.contains("Routine daily"));
        assert!(guidance.contains("Scheduled: @daily"));
        assert!(guidance.contains("per-agent=50"));
        assert!(guidance.contains("history=20"));
        assert!(guidance.contains("coalesced-events=25"));
        assert!(guidance.contains("America/Los_Angeles"));
    }

    #[test]
    fn guidance_caps_visible_routines_at_per_agent_limit() {
        let routines = (0..(AUTOMATION_MAX_PER_AGENT + 3))
            .map(|index| routine(&format!("id-{index:02}"), None))
            .collect::<Vec<_>>();
        let guidance = render_automations_system_guidance(&routines, Some("UTC"));
        assert!(guidance.contains("folder id-49"));
        assert!(!guidance.contains("folder id-50"));
    }
}
