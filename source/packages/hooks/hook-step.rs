pub const HOOK_STEPS: &[&str] = &[
    "beforeShellExecution",
    "beforeMCPExecution",
    "afterShellExecution",
    "afterMCPExecution",
    "beforeReadFile",
    "afterFileEdit",
    "beforeTabFileRead",
    "afterTabFileEdit",
    "stop",
    "beforeSubmitPrompt",
    "afterAgentResponse",
    "afterAgentThought",
    "sessionStart",
    "sessionEnd",
    "preCompact",
    "subagentStart",
    "subagentStop",
    "preToolUse",
    "postToolUse",
    "postToolUseFailure",
    "workspaceOpen",
];

pub fn is_hook_step(value: &str) -> bool {
    HOOK_STEPS.iter().any(|step| *step == value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_exact_pinned_hook_step_set() {
        assert_eq!(HOOK_STEPS.len(), 21);
        assert!(is_hook_step("beforeShellExecution"));
        assert!(is_hook_step("preToolUse"));
        assert!(is_hook_step("workspaceOpen"));
        assert!(!is_hook_step("beforeUnknownExecution"));
    }
}
