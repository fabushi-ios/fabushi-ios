const AUTOMATION_STATUS_PROMPT_MARKER: &str = "<automation_status>";
const AUTOMATION_UI_LIMIT: usize = 100;

fn format_automation_status_timestamp(timestamp_ms: i64, time_zone: Option<&str>) -> String {
    let Ok(timestamp) = JiffTimestamp::from_millisecond(timestamp_ms) else {
        return "never".into();
    };
    let Some(zone) = resolved_automation_time_zone(time_zone, None)
        .or_else(|| JiffTimeZone::get("UTC").ok())
    else {
        return "never".into();
    };
    let date = timestamp.to_zoned(zone);
    let hour24 = date.hour() as u32;
    let hour12 = match hour24 % 12 {
        0 => 12,
        value => value,
    };
    let meridiem = if hour24 < 12 { "AM" } else { "PM" };
    format!(
        "{:02}/{:02}/{:04}, {:02}:{:02}:{:02} {}",
        date.month(),
        date.day(),
        date.year(),
        hour12,
        date.minute(),
        date.second(),
        meridiem
    )
}

fn summarize_automation_last_visible_run(
    automation: &AutomationSummary,
    time_zone: Option<&str>,
    firing_automation_id: Option<&str>,
) -> String {
    let last = automation.runs.iter().rev().find(|run| {
        firing_automation_id != Some(automation.id.as_str())
            || run.status != AutomationRunStatus::Running
    });
    let Some(last) = last else {
        return "never run".into();
    };
    match last.status {
        AutomationRunStatus::Running => format!(
            "running now (started {})",
            format_automation_status_timestamp(last.started_at, time_zone)
        ),
        AutomationRunStatus::Ok => format!(
            "last run {} (succeeded)",
            format_automation_status_timestamp(last.started_at, time_zone)
        ),
        AutomationRunStatus::Error => format!(
            "last run {} (failed)",
            format_automation_status_timestamp(last.started_at, time_zone)
        ),
    }
}

fn render_automation_cleared_status_reminder() -> String {
    [
        "<system_reminder>",
        AUTOMATION_STATUS_PROMPT_MARKER,
        "Current routine runtime status. This snapshot is authoritative for this turn and supersedes earlier routine status reminders.",
        "No current routines.",
        "</automation_status>",
        "</system_reminder>",
    ]
    .join("\n")
}

fn render_automation_runtime_status_reminder(
    automations: &[AutomationSummary],
    time_zone: Option<&str>,
    firing_automation_id: Option<&str>,
) -> Option<String> {
    if automations.is_empty() {
        return None;
    }
    let mut lines = vec![
        "<system_reminder>".to_string(),
        AUTOMATION_STATUS_PROMPT_MARKER.to_string(),
        "Current routine runtime status. This snapshot is authoritative for this turn and supersedes earlier routine status reminders.".to_string(),
    ];
    for automation in automations.iter().take(AUTOMATION_UI_LIMIT) {
        let next = if automation.enabled {
            automation
                .next_run_at_ms
                .map(|value| {
                    format!(
                        "next run {}; ",
                        format_automation_status_timestamp(value, time_zone)
                    )
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        lines.push(format!(
            "- {} (folder {}): {}{}",
            automation.name,
            automation.id,
            next,
            summarize_automation_last_visible_run(
                automation,
                time_zone,
                firing_automation_id,
            )
        ));
    }
    lines.push("</automation_status>".into());
    lines.push("</system_reminder>".into());
    Some(lines.join("\n"))
}

impl FeatureHostController {
    fn create_automation_status_reminder(
        &self,
        firing_automation_id: Option<&str>,
    ) -> Result<String, FeatureHostError> {
        let time_zone = self.host_settings_user_timezone_direct()?;
        let mut automations = self.list_all_automations()?;
        automations.truncate(AUTOMATION_UI_LIMIT);
        Ok(render_automation_runtime_status_reminder(
            &automations,
            time_zone.as_deref(),
            firing_automation_id,
        )
        .unwrap_or_else(render_automation_cleared_status_reminder))
    }
}

#[cfg(test)]
mod automation_status_reminder_tests {
    use super::*;

    fn automation(
        id: &str,
        enabled: bool,
        next_run_at_ms: Option<i64>,
        runs: Vec<AutomationRunSummary>,
    ) -> AutomationSummary {
        AutomationSummary {
            id: id.into(),
            agent_id: None,
            name: format!("Routine {id}"),
            prompt: "Do the thing".into(),
            schedule: "@daily".into(),
            trigger: None,
            enabled,
            created_at_ms: 1,
            raised_notices: Vec::new(),
            runs,
            last_run_at_ms: None,
            next_run_at_ms,
        }
    }

    fn run(id: &str, status: AutomationRunStatus, started_at: i64) -> AutomationRunSummary {
        AutomationRunSummary {
            id: id.into(),
            status,
            started_at,
            detail: None,
            event: None,
        }
    }

    #[test]
    fn cleared_snapshot_supersedes_older_status_when_no_routines_remain() {
        assert!(render_automation_runtime_status_reminder(&[], Some("UTC"), None).is_none());
        let cleared = render_automation_cleared_status_reminder();
        assert!(cleared.contains(AUTOMATION_STATUS_PROMPT_MARKER));
        assert!(cleared.contains("authoritative for this turn"));
        assert!(cleared.contains("No current routines."));
        assert_eq!(cleared.matches(AUTOMATION_STATUS_PROMPT_MARKER).count(), 1);
    }

    #[test]
    fn status_snapshot_reports_next_run_and_never_running_success_failed_states() {
        let rows = vec![
            automation("never", true, Some(3_000), Vec::new()),
            automation(
                "running",
                true,
                Some(4_000),
                vec![run("run-running", AutomationRunStatus::Running, 2_000)],
            ),
            automation(
                "succeeded",
                true,
                None,
                vec![run("run-ok", AutomationRunStatus::Ok, 1_000)],
            ),
            automation(
                "failed-disabled",
                false,
                Some(9_000),
                vec![run("run-error", AutomationRunStatus::Error, 5_000)],
            ),
        ];
        let reminder =
            render_automation_runtime_status_reminder(&rows, Some("UTC"), None).unwrap();
        assert!(reminder.contains("next run 01/01/1970, 12:00:03 AM; never run"));
        assert!(reminder.contains("running now (started 01/01/1970, 12:00:02 AM)"));
        assert!(reminder.contains("last run 01/01/1970, 12:00:01 AM (succeeded)"));
        assert!(reminder.contains("last run 01/01/1970, 12:00:05 AM (failed)"));
        let disabled = reminder
            .lines()
            .find(|line| line.contains("folder failed-disabled"))
            .unwrap();
        assert!(!disabled.contains("next run"));
    }

    #[test]
    fn firing_routine_filters_current_running_run_before_selecting_recent_visible_run() {
        let rows = vec![automation(
            "daily",
            true,
            None,
            vec![
                run("previous", AutomationRunStatus::Ok, 1_000),
                run("current", AutomationRunStatus::Running, 2_000),
            ],
        )];
        let reminder = render_automation_runtime_status_reminder(
            &rows,
            Some("UTC"),
            Some("daily"),
        )
        .unwrap();
        assert!(reminder.contains("last run 01/01/1970, 12:00:01 AM (succeeded)"));
        assert!(!reminder.contains("running now"));
    }

    #[test]
    fn status_snapshot_uses_user_timezone_and_enforces_ui_limit() {
        let utc = vec![automation(
            "daily",
            true,
            Some(0),
            vec![run("ok", AutomationRunStatus::Ok, 0)],
        )];
        let pacific =
            render_automation_runtime_status_reminder(&utc, Some("America/Los_Angeles"), None)
                .unwrap();
        assert!(pacific.contains("12/31/1969, 04:00:00 PM"));

        let many = (0..=AUTOMATION_UI_LIMIT)
            .map(|index| automation(&format!("id-{index:03}"), true, None, Vec::new()))
            .collect::<Vec<_>>();
        let capped =
            render_automation_runtime_status_reminder(&many, Some("UTC"), None).unwrap();
        assert!(capped.contains("folder id-099"));
        assert!(!capped.contains("folder id-100"));
    }
}
