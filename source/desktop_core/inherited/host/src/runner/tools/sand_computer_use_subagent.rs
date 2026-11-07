use crate::r#box::box_monitor_layout::display_space_sentence;

use super::sand_browser_use_subagent::{
    SandCustomSubagentType, SandSubagentType, SandSubagentTypeValue,
};

pub const COMPUTER_USE_SUBAGENT_TYPE: &str = "computerUse";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandComputerUseSubagentConfig {
    pub subagent_type: SandSubagentType,
    pub description: String,
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

pub fn is_computer_use_subagent_type(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        normalized_subagent_type(value) == normalized_subagent_type(COMPUTER_USE_SUBAGENT_TYPE)
    })
}

pub fn computer_use_subagent_description(browser_use_offered: bool) -> String {
    let mut parts = if browser_use_offered {
        vec![
            "Delegate a self-contained desktop task to a background subagent that drives your box's desktop — GUI apps, file dialogs, drag interactions, and sites that defeat page-level automation — by screenshot, click, drag, type, key, scroll, and wait.".to_string(),
            "For browser-only work, dispatch browserUse instead; use computerUse when the task needs the desktop itself or a browserUse dispatch reported a site it could not operate.".to_string(),
        ]
    } else {
        vec![
            "Delegate a self-contained GUI / desktop task to a background subagent that drives your box's desktop — browsing, signing in to sites, and using GUI apps — by screenshot, click, drag, type, key, scroll, and wait.".to_string(),
        ]
    };
    parts.extend([
        display_space_sentence(None, None),
        "It runs in the background like any Task: you are notified when it finishes, so do not poll or await it.".to_string(),
        "It runs headless and cannot ask follow-ups, so give it a tightly-scoped, self-contained task — the smallest concrete step rather than a sprawling goal — with the specifics it needs (site, account, exact values), explicit success criteria and stopping point, and what to report back; break a big GUI goal into several narrow dispatches, and if one runs long or loops, steer it with MessageSubagent or stop it with StopSubagent.".to_string(),
        "Only one computerUse subagent can run at a time, because they share your desktop's single screen — never dispatch a second while one is still running.".to_string(),
        "It cannot act as the user: if a step needs a human (entering a password, 2FA, a captcha, a payment) it stops and reports back, so you can hand the user the box with request_box_help and then dispatch it again to continue.".to_string(),
    ]);
    parts.join(" ")
}

pub fn create_sand_computer_use_subagent_config(
    browser_use_offered: bool,
) -> SandComputerUseSubagentConfig {
    SandComputerUseSubagentConfig {
        subagent_type: SandSubagentType {
            r#type: SandCustomSubagentType {
                case: "custom",
                value: SandSubagentTypeValue {
                    name: COMPUTER_USE_SUBAGENT_TYPE.to_string(),
                },
            },
        },
        description: computer_use_subagent_description(browser_use_offered),
        preserve_task_tool: false,
        subagent_source: "builtin",
    }
}
