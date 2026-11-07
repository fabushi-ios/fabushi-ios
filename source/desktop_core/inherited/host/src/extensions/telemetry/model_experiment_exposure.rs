use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub const HYDRATION_WAIT_MS: u64 = 60_000;
pub const SAND_MODEL_EXPERIMENT_OVERRIDE: &str = "SAND_MODEL_EXPERIMENT_OVERRIDE";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandModelExperimentState {
    pub active: bool,
    pub arm: String,
}

pub fn read_sand_model_experiment_env_override(
    env: &BTreeMap<String, String>,
) -> Option<SandModelExperimentState> {
    let raw = env.get(SAND_MODEL_EXPERIMENT_OVERRIDE)?.trim().to_ascii_lowercase();
    match raw.as_str() {
        "control" => Some(SandModelExperimentState { active: true, arm: "control".into() }),
        "treatment" | "test" => Some(SandModelExperimentState { active: true, arm: "treatment".into() }),
        _ => None,
    }
}

pub trait ModelExperimentExposureExperiments: Send + Sync {
    fn has_hydrated_statsig_user_id(&self) -> bool;
    fn wait_for_hydrated_statsig_user_id(&self, timeout_ms: u64) -> bool;
    fn get_sand_model_experiment_state(&self) -> Option<SandModelExperimentState>;
    fn log_sand_model_experiment_exposure(&self) -> bool;
}

pub trait ModelExperimentExposureAnalytics: Send + Sync {
    fn can_record_events(&self) -> bool;
    fn track_event(&self, name: &str, arm: &str);
}

pub struct ModelExperimentExposureLatch {
    experiments: Arc<dyn ModelExperimentExposureExperiments>,
    analytics: Arc<dyn ModelExperimentExposureAnalytics>,
    env: BTreeMap<String, String>,
    logged: Mutex<bool>,
}

impl ModelExperimentExposureLatch {
    pub fn new(
        experiments: Arc<dyn ModelExperimentExposureExperiments>,
        analytics: Arc<dyn ModelExperimentExposureAnalytics>,
        env: BTreeMap<String, String>,
    ) -> Self {
        Self { experiments, analytics, env, logged: Mutex::new(false) }
    }

    pub fn note(&self) {
        let mut logged = self.logged.lock().unwrap_or_else(|p| p.into_inner());
        if *logged { return; }

        let env_override = read_sand_model_experiment_env_override(&self.env);
        if env_override.is_none() && !self.experiments.has_hydrated_statsig_user_id() {
            let ready = self.experiments.wait_for_hydrated_statsig_user_id(HYDRATION_WAIT_MS);
            if *logged || !ready { return; }
        }

        let state = env_override.or_else(|| self.experiments.get_sand_model_experiment_state());
        let Some(state) = state.filter(|state| state.active) else { return; };

        let sdk_exposed = self.experiments.log_sand_model_experiment_exposure();
        if !sdk_exposed && (read_sand_model_experiment_env_override(&self.env).is_none()
            || !self.analytics.can_record_events())
        {
            return;
        }

        *logged = true;
        if self.analytics.can_record_events() {
            self.analytics.track_event("sand.model_experiment.exposure", &state.arm);
        }
    }

    pub fn is_logged(&self) -> bool {
        *self.logged.lock().unwrap_or_else(|p| p.into_inner())
    }
}
