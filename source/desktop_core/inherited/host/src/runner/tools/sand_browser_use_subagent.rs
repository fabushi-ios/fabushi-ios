pub const BROWSER_USE_SUBAGENT_TYPE: &str = "browserUse";

pub const BROWSER_USE_SUBAGENT_DESCRIPTION: &str = concat!(
    "Delegate a self-contained web task to a background subagent that drives your box's browser at the page level — navigating, reading structured page snapshots, clicking elements by reference, filling forms, and taking screenshots — without touching the desktop's mouse or keyboard. ",
    "Prefer it over computerUse for browser-only work: page snapshots give it exact element targets, so it is faster and more reliable than pixel clicking, and it shares the box browser's persistent logins. ",
    "Use computerUse instead when the task needs the desktop itself (GUI apps, file dialogs, drag interactions) or a site that defeats DOM automation. ",
    "It runs in the background like any Task: you are notified when it finishes, so do not poll or await it. ",
    "It runs headless and cannot ask follow-ups, so give it a tightly-scoped, self-contained task with the specifics it needs (site, exact values), explicit success criteria, and what to report back. ",
    "It cannot act as the user: if a step needs a human (a password, 2FA, a captcha, a payment) it stops and reports back, so you can hand the user the box with request_box_help and dispatch it again to continue."
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandSubagentTypeValue {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandCustomSubagentType {
    pub case: &'static str,
    pub value: SandSubagentTypeValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandSubagentType {
    pub r#type: SandCustomSubagentType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandBrowserUseSubagentConfig {
    pub subagent_type: SandSubagentType,
    pub description: &'static str,
    pub preserve_task_tool: bool,
    pub subagent_source: &'static str,
}

fn normalized_subagent_type(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !matches!(ch, '-' | '_' | ' '))
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn is_browser_use_subagent_type(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        normalized_subagent_type(value) == normalized_subagent_type(BROWSER_USE_SUBAGENT_TYPE)
    })
}

pub fn create_sand_browser_use_subagent_config() -> SandBrowserUseSubagentConfig {
    SandBrowserUseSubagentConfig {
        subagent_type: SandSubagentType {
            r#type: SandCustomSubagentType {
                case: "custom",
                value: SandSubagentTypeValue {
                    name: BROWSER_USE_SUBAGENT_TYPE.to_string(),
                },
            },
        },
        description: BROWSER_USE_SUBAGENT_DESCRIPTION,
        preserve_task_tool: false,
        subagent_source: "builtin",
    }
}
