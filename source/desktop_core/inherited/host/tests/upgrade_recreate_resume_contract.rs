use mahayana_host_runtime::extensions::transcript::upgrade_recreate_resume::{
    UpgradeRecreateResume, build_upgrade_resume_prompt,
};

#[test]
fn upgrade_quiesce_state_matches_frozen_lifecycle_toggle() {
    let resume = UpgradeRecreateResume::default();
    assert!(!resume.is_quiescing_for_upgrade());

    let summary = resume.quiesce_for_upgrade(3);
    assert!(summary.quiescing);
    assert_eq!(summary.running_turns, 3);
    assert!(resume.is_quiescing_for_upgrade());

    resume.resume_after_recreate();
    assert!(!resume.is_quiescing_for_upgrade());
}

#[test]
fn upgrade_resume_prompts_preserve_source_specific_silence_semantics() {
    let turn = build_upgrade_resume_prompt("turn");
    assert!(turn.contains("interrupted you mid-task"));
    assert!(turn.contains("nothing reaches the user unless it's inside a SendMessage"));

    let automation = build_upgrade_resume_prompt("automation");
    assert!(automation.contains("scheduled routine run"));
    assert!(automation.contains("ending with no SendMessage remains a valid outcome"));

    let revival = build_upgrade_resume_prompt("background-revival");
    assert!(revival.contains("background-work follow-ups"));
    assert!(revival.contains("ending with no SendMessage remains a valid outcome"));
}
