use mahayana_host_runtime::sand_multitask::*;

#[test]
fn env_override_precedes_gate_and_matches_frozen_truthiness() {
    assert!(!resolve_multitask_enabled(Some("0"), || true));
    assert!(!resolve_multitask_enabled(Some("FALSE"), || true));
    assert!(resolve_multitask_enabled(Some("no"), || false));
    assert!(resolve_multitask_enabled(None, || true));
    assert!(!resolve_multitask_enabled(None, || false));
}

#[test]
fn executor_config_uses_the_single_builtin_executor_type() {
    let config = create_sand_executor_subagent_config();
    assert_eq!(config.subagent_type.r#type.case, "custom");
    assert_eq!(
        config.subagent_type.r#type.value.name,
        EXECUTOR_SUBAGENT_TYPE
    );
    assert!(!config.preserve_task_tool);
    assert_eq!(config.subagent_source, "builtin");
    assert!(config.description.contains("background subagent"));
    assert!(SAND_MULTITASK_TODO_DESCRIPTION.contains("SEVERAL todos"));
    assert!(SAND_MULTITASK_PROMPT_SECTION.starts_with("## Multitasking"));
    assert!(SAND_MULTITASK_PROMPT_SECTION.contains("this holds exactly as hard for the small jobs you do inline as for delegated ones"));
    assert!(SAND_MULTITASK_PROMPT_SECTION.contains("call Task with subagent_type \"executor\""));
    assert!(SAND_MULTITASK_PROMPT_SECTION.contains("The same goes for resuming one: resume does not carry over its context"));
    assert!(SAND_MULTITASK_PROMPT_SECTION.contains("never tell the user you are \"dispatching\""));
    assert!(SAND_MULTITASK_PROMPT_SECTION.ends_with("follow the room's instructions and do the work inline in your turn."));
}
