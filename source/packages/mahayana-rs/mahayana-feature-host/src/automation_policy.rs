// Desktop automation policy parity for the existing account-scoped FeatureHost owner.
// This file defines normalization/limits and renders Host-owned guidance only;
// it does not create a second automation store or scheduler.

const AUTOMATION_MAX_NAME_LENGTH: usize = 80;
const AUTOMATION_MAX_PER_AGENT: usize = 50;
const AUTOMATION_MAX_RUN_HISTORY: usize = 20;
const AUTOMATION_MAX_RUN_DETAIL_LENGTH: usize = 300;
const MAX_EVENTS_IN_AUTOMATION_WAKE: usize = 25;
const AUTOMATION_PROMPT_GUIDANCE_VERSION: &str = "backend_triggers_v4";

const TRIGGER_MAX_REACTION_EMOJI: usize = 8;
const TRIGGER_MAX_CHANNEL_LENGTH: usize = 80;
const TRIGGER_MAX_KEYWORD_LENGTH: usize = 120;
const TRIGGER_MAX_REPO_LENGTH: usize = 140;
const TRIGGER_MAX_BRANCH_LENGTH: usize = 200;
const TRIGGER_MAX_ALLOWLIST_LOGINS: usize = 50;
const TRIGGER_MAX_ALLOWLIST_LOGIN_LENGTH: usize = 80;
const TRIGGER_MAX_FILTER_IDS: usize = 50;
const TRIGGER_MAX_ID_LENGTH: usize = 200;
const GITHUB_EVENT_KINDS: &[&str] = &[
    "pr-opened",
    "pr-pushed",
    "pr-merged",
    "review-requested",
    "review-approved",
    "review-changes-requested",
    "review-commented",
    "pr-comment",
    "inline-review-comment",
    "review-thread-resolved",
    "review-thread-unresolved",
    "issue-assigned",
    "ci-passed",
    "ci-failed",
];

fn normalize_trigger_token(value: &str, max: usize) -> String {
    value
        .replace(['\r', '\n'], " ")
        .trim()
        .chars()
        .take(max)
        .collect()
}

fn valid_github_repo(repo: &str) -> bool {
    let mut parts = repo.split('/');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(owner), Some(name), None)
            if !owner.is_empty()
                && !name.is_empty()
                && !owner.chars().any(char::is_whitespace)
                && !name.chars().any(char::is_whitespace)
    )
}

fn valid_git_branch(branch: &str) -> bool {
    !branch.is_empty()
        && !branch.chars().any(char::is_whitespace)
        && !branch.contains("..")
        && !branch.contains("@{")
        && !branch.starts_with('-')
        && !branch.starts_with('/')
        && !branch.ends_with('/')
        && !branch
            .chars()
            .any(|ch| matches!(ch, '~' | '^' | ':' | '?' | '*' | '[' | '\\' | '|'))
}

fn normalize_reaction_emoji(value: &str) -> Option<String> {
    let bare = value.trim().trim_matches(':');
    let normalized = bare
        .split("::")
        .next()
        .unwrap_or(bare)
        .trim()
        .to_ascii_lowercase();
    (!normalized.is_empty()
        && normalized.chars().all(|ch| {
            ch.is_ascii_lowercase()
                || ch.is_ascii_digit()
                || matches!(ch, '_' | '+' | '-')
        }))
    .then_some(normalized)
}

fn normalize_filter_string_list(
    value: Option<&Value>,
    max_count: usize,
    max_len: usize,
) -> Vec<String> {
    let Some(values) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut output = Vec::new();
    for value in values {
        let Some(raw) = value.as_str() else {
            continue;
        };
        let normalized = normalize_trigger_token(raw, max_len);
        if normalized.is_empty() || output.contains(&normalized) {
            continue;
        }
        output.push(normalized);
        if output.len() >= max_count {
            break;
        }
    }
    output
}

fn normalize_automation_event_filters(
    source: ListenerPlatform,
    event: &str,
    filters: Option<BTreeMap<String, Value>>,
) -> Result<Option<BTreeMap<String, Value>>, FeatureHostError> {
    let mut filters = filters.unwrap_or_default();
    match source {
        ListenerPlatform::Github => {
            let repo = filters
                .get("repo")
                .and_then(Value::as_str)
                .map(|value| normalize_trigger_token(value, TRIGGER_MAX_REPO_LENGTH))
                .filter(|value| valid_github_repo(value))
                .ok_or_else(|| {
                    FeatureHostError::Contract(
                        "GitHub routine listener requires a valid owner/repo".into(),
                    )
                })?;
            filters.insert("repo".into(), Value::String(repo));

            let mut events = normalize_filter_string_list(
                filters.get("events"),
                TRIGGER_MAX_FILTER_IDS,
                TRIGGER_MAX_ID_LENGTH,
            );
            if event != "*" {
                let event = normalize_trigger_token(event, TRIGGER_MAX_ID_LENGTH);
                if GITHUB_EVENT_KINDS.contains(&event.as_str()) && !events.contains(&event) {
                    events.push(event);
                }
            }
            events.retain(|kind| GITHUB_EVENT_KINDS.contains(&kind.as_str()));
            events.dedup();
            if events.is_empty() {
                return Err(FeatureHostError::Contract(
                    "GitHub routine listener requires at least one supported event".into(),
                ));
            }

            let ci_branch = filters
                .get("ciBranch")
                .and_then(Value::as_str)
                .map(|value| normalize_trigger_token(value, TRIGGER_MAX_BRANCH_LENGTH))
                .filter(|value| valid_git_branch(value));
            let watches_ci = events.iter().any(|kind| is_github_ci_event_kind(kind));
            if watches_ci && ci_branch.is_none() {
                return Err(FeatureHostError::Contract(
                    "GitHub CI routine listeners require a valid concrete branch".into(),
                ));
            }
            filters.insert("events".into(), json!(events));
            if let Some(branch) = ci_branch {
                filters.insert("ciBranch".into(), Value::String(branch));
            } else {
                filters.remove("ciBranch");
            }

            let mut logins = normalize_filter_string_list(
                filters.get("userAllowlist"),
                TRIGGER_MAX_ALLOWLIST_LOGINS,
                TRIGGER_MAX_ALLOWLIST_LOGIN_LENGTH,
            );
            let mut unique = Vec::new();
            for login in logins.drain(..) {
                let login = login.trim_start_matches('@').to_string();
                if login.is_empty()
                    || unique
                        .iter()
                        .any(|existing: &String| existing.eq_ignore_ascii_case(&login))
                {
                    continue;
                }
                unique.push(login);
            }
            if unique.is_empty() {
                filters.remove("userAllowlist");
            } else {
                filters.insert("userAllowlist".into(), json!(unique));
            }
        }
        ListenerPlatform::Slack => {
            let channel = filters
                .get("channel")
                .and_then(Value::as_str)
                .map(|value| normalize_trigger_token(value, TRIGGER_MAX_CHANNEL_LENGTH))
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    FeatureHostError::Contract(
                        "Slack routine listener requires a channel, DM, or wildcard scope".into(),
                    )
                })?;
            filters.insert("channel".into(), Value::String(channel));
            if event == "keyword" {
                let keyword = filters
                    .get("keyword")
                    .and_then(Value::as_str)
                    .map(|value| normalize_trigger_token(value, TRIGGER_MAX_KEYWORD_LENGTH))
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        FeatureHostError::Contract(
                            "Slack keyword routine listener requires a keyword".into(),
                        )
                    })?;
                filters.insert("keyword".into(), Value::String(keyword));
            } else if event == "reaction" {
                let emoji = filters
                    .get("emoji")
                    .and_then(Value::as_array)
                    .map(|values| {
                        let mut output = Vec::new();
                        for value in values {
                            if let Some(value) =
                                value.as_str().and_then(normalize_reaction_emoji)
                            {
                                if !output.contains(&value) {
                                    output.push(value);
                                }
                            }
                            if output.len() >= TRIGGER_MAX_REACTION_EMOJI {
                                break;
                            }
                        }
                        output
                    })
                    .unwrap_or_default();
                if !emoji.is_empty() {
                    filters.insert("emoji".into(), json!(emoji));
                } else {
                    filters.remove("emoji");
                }
            } else if !matches!(event, "mention" | "message") {
                return Err(FeatureHostError::Contract(
                    "unsupported Slack routine listener event".into(),
                ));
            }
        }
        _ => {
            let keys = filters.keys().cloned().collect::<Vec<_>>();
            for key in keys {
                match filters.get(&key).cloned() {
                    Some(Value::String(value)) => {
                        let normalized = normalize_trigger_token(&value, TRIGGER_MAX_ID_LENGTH);
                        if normalized.is_empty() {
                            filters.remove(&key);
                        } else {
                            filters.insert(key, Value::String(normalized));
                        }
                    }
                    Some(Value::Array(_)) => {
                        let values = normalize_filter_string_list(
                            filters.get(&key),
                            TRIGGER_MAX_FILTER_IDS,
                            TRIGGER_MAX_ID_LENGTH,
                        );
                        filters.insert(key, json!(values));
                    }
                    _ => {}
                }
            }
        }
    }
    Ok((!filters.is_empty()).then_some(filters))
}

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

fn slugify_automation_name(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in name.trim().to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "automation".into()
    } else {
        out
    }
}

fn unique_automation_id(
    automations: &BTreeMap<String, AutomationSummary>,
    name: &str,
) -> String {
    let base = slugify_automation_name(name);
    if !automations.contains_key(&base) {
        return base;
    }
    for suffix in 2..1000 {
        let candidate = format!("{base}-{suffix}");
        if !automations.contains_key(&candidate) {
            return candidate;
        }
    }
    format!("{base}-{}", Uuid::new_v4())
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
        let existing = BTreeMap::from([
            ("daily".to_string(), routine("daily", None)),
            ("daily-2".to_string(), routine("daily-2", None)),
        ]);
        assert_eq!(slugify_automation_name(" Daily !!! "), "daily");
        assert_eq!(unique_automation_id(&existing, "Daily"), "daily-3");
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
        assert!(valid_github_repo("owner/repo"));
        assert!(!valid_github_repo("owner/repo/extra"));
        assert!(valid_git_branch("release/main"));
        assert!(!valid_git_branch("release..main"));
        assert_eq!(normalize_reaction_emoji(":THUMBS_UP::skin-tone-2:").as_deref(), Some("thumbs_up"));
    }

    #[test]
    fn github_listener_filters_fail_closed_and_normalize_identity() {
        assert!(normalize_automation_event_filters(
            ListenerPlatform::Github,
            "pr-opened",
            Some(BTreeMap::from([("repo".into(), json!("not-a-repo"))])),
        )
        .is_err());

        assert!(normalize_automation_event_filters(
            ListenerPlatform::Github,
            "ci-failed",
            Some(BTreeMap::from([
                ("repo".into(), json!("owner/repo")),
                ("events".into(), json!(["ci-failed"])),
            ])),
        )
        .is_err());

        let normalized = normalize_automation_event_filters(
            ListenerPlatform::Github,
            "*",
            Some(BTreeMap::from([
                ("repo".into(), json!(" owner/repo ")),
                ("events".into(), json!(["pr-opened", "ci-failed", "unknown"])),
                ("ciBranch".into(), json!("main")),
                ("userAllowlist".into(), json!(["@Alice", "alice", "@Bob"])),
            ])),
        )
        .unwrap()
        .unwrap();
        assert_eq!(normalized["repo"], json!("owner/repo"));
        assert_eq!(normalized["events"], json!(["pr-opened", "ci-failed"]));
        assert_eq!(normalized["ciBranch"], json!("main"));
        assert_eq!(normalized["userAllowlist"], json!(["Alice", "Bob"]));
    }

    #[test]
    fn slack_listener_filters_normalize_scope_keyword_and_reaction() {
        let keyword = normalize_automation_event_filters(
            ListenerPlatform::Slack,
            "keyword",
            Some(BTreeMap::from([
                ("channel".into(), json!(" #alerts\n")),
                ("keyword".into(), json!("  deploy failed\n")),
            ])),
        )
        .unwrap()
        .unwrap();
        assert_eq!(keyword["channel"], json!("#alerts"));
        assert_eq!(keyword["keyword"], json!("deploy failed"));

        let reaction = normalize_automation_event_filters(
            ListenerPlatform::Slack,
            "reaction",
            Some(BTreeMap::from([
                ("channel".into(), json!("alerts")),
                ("emoji".into(), json!([":THUMBS_UP::skin:", "thumbs_up", "bad emoji"])),
            ])),
        )
        .unwrap()
        .unwrap();
        assert_eq!(reaction["emoji"], json!(["thumbs_up"]));
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
