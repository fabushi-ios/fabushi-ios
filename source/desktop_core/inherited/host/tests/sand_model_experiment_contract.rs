use std::cell::Cell;

use mahayana_host_runtime::extensions::inference::sand_model_experiment::{
    SAND_AUTOMATION_REQUEST_SOURCE, SandAgentModelParameter, SandAgentModelSelection,
    SandModelExperimentArm, SandModelExperimentState,
    read_sand_model_experiment_env_override, sand_model_experiment_opus_medium_selection,
    select_sand_experiment_turn_model, select_sand_model_experiment_model,
};

fn configured(id: &str) -> SandAgentModelSelection {
    SandAgentModelSelection {
        model_id: id.into(),
        max_mode: false,
        parameters: vec![SandAgentModelParameter {
            id: "effort".into(),
            value: "high".into(),
        }],
    }
}

#[test]
fn env_override_matches_frozen_control_and_treatment_aliases() {
    assert_eq!(
        read_sand_model_experiment_env_override(Some(" control ")),
        Some(SandModelExperimentState {
            active: true,
            arm: SandModelExperimentArm::Control,
        })
    );
    assert_eq!(
        read_sand_model_experiment_env_override(Some("TREATMENT")),
        Some(SandModelExperimentState {
            active: true,
            arm: SandModelExperimentArm::Treatment,
        })
    );
    assert_eq!(
        read_sand_model_experiment_env_override(Some("test")),
        Some(SandModelExperimentState {
            active: true,
            arm: SandModelExperimentArm::Treatment,
        })
    );
    assert_eq!(read_sand_model_experiment_env_override(Some("off")), None);
    assert_eq!(read_sand_model_experiment_env_override(None), None);
}

#[test]
fn control_arm_forces_frozen_opus_medium_selection() {
    let state = SandModelExperimentState {
        active: true,
        arm: SandModelExperimentArm::Control,
    };
    assert_eq!(
        select_sand_model_experiment_model(
            Some(&state),
            Some("turn"),
            Some(configured("configured-model")),
        ),
        Some(sand_model_experiment_opus_medium_selection())
    );
}

#[test]
fn inactive_and_missing_experiment_return_no_override() {
    let inactive = SandModelExperimentState {
        active: false,
        arm: SandModelExperimentArm::Treatment,
    };
    assert_eq!(
        select_sand_model_experiment_model(
            Some(&inactive),
            None,
            Some(configured("configured")),
        ),
        None
    );
    assert_eq!(
        select_sand_model_experiment_model(None, None, Some(configured("configured"))),
        None
    );
}

#[test]
fn treatment_uses_configured_default_for_normal_turns() {
    let state = SandModelExperimentState {
        active: true,
        arm: SandModelExperimentArm::Treatment,
    };
    let selected = select_sand_experiment_turn_model(
        Some(&state),
        Some("turn"),
        || Some(configured("default-model")),
        || Some(configured("automation-model")),
    );
    assert_eq!(selected, Some(configured("default-model")));
}

#[test]
fn automation_prefers_automation_model_then_default() {
    let state = SandModelExperimentState {
        active: true,
        arm: SandModelExperimentArm::Treatment,
    };
    assert_eq!(
        select_sand_experiment_turn_model(
            Some(&state),
            Some(SAND_AUTOMATION_REQUEST_SOURCE),
            || Some(configured("default-model")),
            || Some(configured("automation-model")),
        ),
        Some(configured("automation-model"))
    );
    assert_eq!(
        select_sand_experiment_turn_model(
            Some(&state),
            Some(SAND_AUTOMATION_REQUEST_SOURCE),
            || Some(configured("default-model")),
            || None,
        ),
        Some(configured("default-model"))
    );
}

#[test]
fn treatment_without_config_falls_back_to_frozen_selection() {
    let state = SandModelExperimentState {
        active: true,
        arm: SandModelExperimentArm::Treatment,
    };
    assert_eq!(
        select_sand_experiment_turn_model(
            Some(&state),
            Some("turn"),
            || None,
            || None,
        ),
        Some(sand_model_experiment_opus_medium_selection())
    );
}

#[test]
fn inactive_and_control_paths_do_not_read_config() {
    let reads = Cell::new(0_u32);
    let inactive = SandModelExperimentState {
        active: false,
        arm: SandModelExperimentArm::Treatment,
    };
    let selected = select_sand_experiment_turn_model(
        Some(&inactive),
        Some(SAND_AUTOMATION_REQUEST_SOURCE),
        || {
            reads.set(reads.get() + 1);
            Some(configured("default"))
        },
        || {
            reads.set(reads.get() + 1);
            Some(configured("automation"))
        },
    );
    assert_eq!(selected, None);
    assert_eq!(reads.get(), 0);

    let control = SandModelExperimentState {
        active: true,
        arm: SandModelExperimentArm::Control,
    };
    let selected = select_sand_experiment_turn_model(
        Some(&control),
        Some(SAND_AUTOMATION_REQUEST_SOURCE),
        || {
            reads.set(reads.get() + 1);
            Some(configured("default"))
        },
        || {
            reads.set(reads.get() + 1);
            Some(configured("automation"))
        },
    );
    assert_eq!(selected, Some(sand_model_experiment_opus_medium_selection()));
    assert_eq!(reads.get(), 0);
}
