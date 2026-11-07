#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandAgentModelParameter {
    pub id: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandAgentModelSelection {
    pub model_id: String,
    pub max_mode: bool,
    pub parameters: Vec<SandAgentModelParameter>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandModelExperimentArm {
    Control,
    Treatment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandModelExperimentState {
    pub active: bool,
    pub arm: SandModelExperimentArm,
}

pub const SAND_AUTOMATION_REQUEST_SOURCE: &str = "automation";

pub fn read_sand_model_experiment_env_override(
    raw: Option<&str>,
) -> Option<SandModelExperimentState> {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("control") => Some(SandModelExperimentState {
            active: true,
            arm: SandModelExperimentArm::Control,
        }),
        Some("treatment") | Some("test") => Some(SandModelExperimentState {
            active: true,
            arm: SandModelExperimentArm::Treatment,
        }),
        _ => None,
    }
}

pub fn read_sand_model_experiment_process_override() -> Option<SandModelExperimentState> {
    let raw = std::env::var("SAND_MODEL_EXPERIMENT_OVERRIDE").ok();
    read_sand_model_experiment_env_override(raw.as_deref())
}


pub fn sand_model_experiment_opus_medium_selection() -> SandAgentModelSelection {
    SandAgentModelSelection {
        model_id: "claude-opus-4-8".into(),
        max_mode: true,
        parameters: vec![
            SandAgentModelParameter {
                id: "thinking".into(),
                value: "true".into(),
            },
            SandAgentModelParameter {
                id: "context".into(),
                value: "1m".into(),
            },
            SandAgentModelParameter {
                id: "effort".into(),
                value: "medium".into(),
            },
            SandAgentModelParameter {
                id: "fast".into(),
                value: "false".into(),
            },
        ],
    }
}

pub fn select_sand_model_experiment_model(
    state: Option<&SandModelExperimentState>,
    _request_source: Option<&str>,
    configured_model: Option<SandAgentModelSelection>,
) -> Option<SandAgentModelSelection> {
    let state = state?;
    if !state.active {
        return None;
    }

    if state.arm == SandModelExperimentArm::Control {
        return Some(sand_model_experiment_opus_medium_selection());
    }

    Some(
        configured_model
            .unwrap_or_else(sand_model_experiment_opus_medium_selection),
    )
}

pub fn select_sand_experiment_turn_model<DefaultModel, AutomationsModel>(
    state: Option<&SandModelExperimentState>,
    request_source: Option<&str>,
    mut read_configured_default_model: DefaultModel,
    mut read_configured_automations_model: AutomationsModel,
) -> Option<SandAgentModelSelection>
where
    DefaultModel: FnMut() -> Option<SandAgentModelSelection>,
    AutomationsModel: FnMut() -> Option<SandAgentModelSelection>,
{
    let Some(state) = state else {
        return None;
    };

    if !state.active || state.arm == SandModelExperimentArm::Control {
        return select_sand_model_experiment_model(
            Some(state),
            request_source,
            None,
        );
    }

    let configured = if request_source == Some(SAND_AUTOMATION_REQUEST_SOURCE) {
        read_configured_automations_model()
            .or_else(&mut read_configured_default_model)
    } else {
        read_configured_default_model()
    };

    select_sand_model_experiment_model(
        Some(state),
        request_source,
        configured,
    )
}
