use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::blocking::Client;
use serde_json::Value;
use url::Url;
use uuid::Uuid;

use crate::extensions::auth::credential_renewer::{
    RenewalOutcome, SAND_CLIENT_TYPE, sand_box_namespace, sand_client_version,
};
use crate::extensions::auth::extension::HostAuthExtension;
use crate::extensions::browser_ua::extension::{
    BrowserUaExperimentsApi, StopSubscription,
};
use crate::extensions::extension_ids_generated::HostExtensionId;
use crate::extensions::inference::sand_model_experiment::{
    SandAgentModelParameter, SandAgentModelSelection, SandModelExperimentArm,
    SandModelExperimentState, read_sand_model_experiment_process_override,
};

pub const EXPERIMENTS_DEPENDENCIES: &[HostExtensionId] =
    &[HostExtensionId::Auth, HostExtensionId::Settings];
pub const STATSIG_BOOTSTRAP_CACHE_FILENAME: &str = "sand-statsig-bootstrap.json";
pub const STATSIG_BOOTSTRAP_PATH: &str = "aiserver.v1.AnalyticsService/BootstrapStatsig";
pub const STATSIG_BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(30);
pub const STATSIG_BASE_POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
pub const STATSIG_MIN_POLL_INTERVAL: Duration = Duration::from_secs(30);
pub const STATSIG_POLL_JITTER_PERCENT: u64 = 30;
pub const GATE_READY_TIMEOUT: Duration = Duration::from_secs(10);
pub const HYDRATED_USER_TIMEOUT: Duration = Duration::from_secs(60);
pub const SAND_MODEL_EXPERIMENT_NAME: &str = "sand_model_selection";
pub const SAND_DEFAULT_MODEL_CONFIG_NAME: &str = "sand_default_model";
pub const SAND_AUTOMATIONS_MODEL_CONFIG_NAME: &str = "sand_automations_model";
pub const SAND_COMPUTER_USE_CONFIG_NAME: &str = "sand_computer_use_playwright_config";
pub const SAND_BROWSER_USE_CONFIG_NAME: &str = "sand_browser_use_model";
pub const STATSIG_CLIENT_KEY: &str =
    "client-Bm4HJ0aDjXHQVsoACMREyLNxm5p6zzuzhO50MgtoT5D";
pub const STATSIG_LOG_EVENT_PROXY_URL: &str = "https://api3.cursor.sh/tev1/v1";
pub const STATSIG_JS_SDK_VERSION: &str = "3.31.0";
pub const SAND_PRODUCT_FEATURE_GATE_DEFAULTS_ENV: &str = "SAND_PRODUCT_FEATURE_GATE_DEFAULTS";

pub fn experiments_extension_id() -> HostExtensionId {
    HostExtensionId::Experiments
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostExperimentsOptions {
    pub is_dev_build: bool,
    pub env_gate_overrides: Option<String>,
}

impl HostExperimentsOptions {
    pub fn from_process_env() -> Self {
        let is_dev_build = std::env::var("SAND_PACKAGED").as_deref() != Ok("1")
            || std::env::var("SAND_HOST_DEV_ERROR_DETAIL").as_deref() == Ok("1");
        Self {
            is_dev_build,
            env_gate_overrides: std::env::var("SAND_FEATURE_GATE_OVERRIDES")
                .ok()
                .filter(|value| !value.trim().is_empty()),
        }
    }
}

type AuthenticatedGatePin = Arc<dyn Fn(bool) + Send + Sync>;

pub const PRE_PIN_DIAGNOSTICS_BUFFER_CAP: usize = 64;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExperimentDiagnostic {
    pub kind: String,
    pub stage: Option<String>,
    pub reason: Option<String>,
    pub error_class: Option<String>,
    pub gates_on_count: Option<i64>,
    pub authenticated: Option<bool>,
}

pub type ExperimentDiagnosticReporter =
    Arc<dyn Fn(ExperimentDiagnostic) + Send + Sync>;

#[derive(Default)]
struct ExperimentDiagnosticReporterState {
    reporter: Option<ExperimentDiagnosticReporter>,
    buffered: Vec<ExperimentDiagnostic>,
}

fn experiment_diagnostic_reporter_state() -> &'static Mutex<ExperimentDiagnosticReporterState> {
    static STATE: OnceLock<Mutex<ExperimentDiagnosticReporterState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ExperimentDiagnosticReporterState::default()))
}

pub fn pin_experiments_diagnostics_reporter(
    reporter: Option<ExperimentDiagnosticReporter>,
) {
    let backlog = {
        let mut state = experiment_diagnostic_reporter_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.reporter = reporter.clone();
        std::mem::take(&mut state.buffered)
    };
    if let Some(reporter) = reporter {
        for diagnostic in backlog {
            reporter(diagnostic);
        }
    }
}

pub fn report_experiments_diagnostic(diagnostic: ExperimentDiagnostic) {
    let reporter = {
        let mut state = experiment_diagnostic_reporter_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(reporter) = state.reporter.as_ref().cloned() {
            Some(reporter)
        } else {
            if state.buffered.len() < PRE_PIN_DIAGNOSTICS_BUFFER_CAP {
                state.buffered.push(diagnostic.clone());
            }
            None
        }
    };
    if let Some(reporter) = reporter {
        reporter(diagnostic);
    }
}

#[derive(Debug, Clone, Default)]
struct StatsigExperimentAssignment {
    value: BTreeMap<String, Value>,
    group_name: Option<String>,
    rule_id: Option<String>,
    secondary_exposures: Vec<Value>,
}

#[derive(Debug, Default)]
struct ExperimentsReadiness {
    has_live_statsig_bootstrap: bool,
    is_refreshing: bool,
    is_disposed: bool,
    hydrated_user_id: Option<String>,
}

struct MutableGatePropertyState {
    value: bool,
    listeners: BTreeMap<u64, Arc<dyn Fn(bool) + Send + Sync>>,
    next_listener_id: u64,
}

pub struct MutableGateProperty {
    state: Arc<Mutex<MutableGatePropertyState>>,
}

impl MutableGateProperty {
    fn new(value: bool) -> Self {
        Self {
            state: Arc::new(Mutex::new(MutableGatePropertyState {
                value,
                listeners: BTreeMap::new(),
                next_listener_id: 0,
            })),
        }
    }

    pub fn get(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .value
    }

    pub fn subscribe(
        &self,
        listener: Arc<dyn Fn(bool) + Send + Sync>,
    ) -> StopSubscription {
        let id = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.next_listener_id = state.next_listener_id.saturating_add(1);
            let id = state.next_listener_id;
            state.listeners.insert(id, listener);
            id
        };
        let state = Arc::downgrade(&self.state);
        Box::new(move || {
            if let Some(state) = state.upgrade() {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .listeners
                    .remove(&id);
            }
        })
    }

    fn set(&self, value: bool) {
        let listeners = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.value == value {
                return;
            }
            state.value = value;
            state.listeners.values().cloned().collect::<Vec<_>>()
        };
        for listener in listeners {
            listener(value);
        }
    }

    fn clear_listeners(&self) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .listeners
            .clear();
    }
}

struct ExperimentsState {
    options: HostExperimentsOptions,
    settings_overrides: Mutex<BTreeMap<String, bool>>,
    network_feature_gates: Mutex<BTreeMap<String, bool>>,
    network_dynamic_configs: Mutex<BTreeMap<String, BTreeMap<String, Value>>>,
    dynamic_config_overrides: Mutex<BTreeMap<String, BTreeMap<String, Value>>>,
    network_experiments: Mutex<BTreeMap<String, StatsigExperimentAssignment>>,
    statsig_user: Mutex<Option<Value>>,
    listeners: Mutex<BTreeMap<u64, Arc<dyn Fn() + Send + Sync>>>,
    next_listener_id: AtomicU64,
    gate_properties: Mutex<BTreeMap<String, Arc<MutableGateProperty>>>,
    readiness: Mutex<ExperimentsReadiness>,
    readiness_changed: Condvar,
    has_authenticated_statsig_bootstrap: AtomicBool,
    model_experiment_exposure_logged: AtomicBool,
    authenticated_gate_pins: Mutex<Vec<(String, AuthenticatedGatePin)>>,
}

#[derive(Clone)]
pub struct HostExperimentsExtension {
    state: Arc<ExperimentsState>,
}

impl HostExperimentsExtension {
    pub fn new(options: HostExperimentsOptions) -> Self {
        Self {
            state: Arc::new(ExperimentsState {
                options,
                settings_overrides: Mutex::new(BTreeMap::new()),
                network_feature_gates: Mutex::new(BTreeMap::new()),
                network_dynamic_configs: Mutex::new(BTreeMap::new()),
                dynamic_config_overrides: Mutex::new(BTreeMap::new()),
                network_experiments: Mutex::new(BTreeMap::new()),
                statsig_user: Mutex::new(None),
                listeners: Mutex::new(BTreeMap::new()),
                next_listener_id: AtomicU64::new(0),
                gate_properties: Mutex::new(BTreeMap::new()),
                readiness: Mutex::new(ExperimentsReadiness::default()),
                readiness_changed: Condvar::new(),
                has_authenticated_statsig_bootstrap: AtomicBool::new(false),
                model_experiment_exposure_logged: AtomicBool::new(false),
                authenticated_gate_pins: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn check_feature_gate(&self, name: &str) -> bool {
        if let Some(value) = self
            .state
            .settings_overrides
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(name)
            .copied()
        {
            return value;
        }
        if self.state.options.is_dev_build {
            if let Some(value) = env_gate_override(
                name,
                self.state.options.env_gate_overrides.as_deref(),
            ) {
                return value;
            }
        }
        if let Some(value) = self
            .state
            .network_feature_gates
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(name)
            .copied()
        {
            return value;
        }
        if let Some(value) = std::env::var(SAND_PRODUCT_FEATURE_GATE_DEFAULTS_ENV)
            .ok()
            .as_deref()
            .and_then(|raw| env_gate_override(name, Some(raw)))
        {
            return value;
        }
        bundled_feature_gate_default(name)
    }

    pub fn check_gate(&self, name: &str, timeout: Option<Duration>) -> bool {
        let timeout = timeout.unwrap_or(GATE_READY_TIMEOUT);
        let started = Instant::now();
        let mut readiness = self
            .state
            .readiness
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while !readiness.has_live_statsig_bootstrap
            && readiness.is_refreshing
            && !readiness.is_disposed
        {
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break;
            }
            let (next, result) = self
                .state
                .readiness_changed
                .wait_timeout(readiness, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            readiness = next;
            if result.timed_out() {
                break;
            }
        }
        drop(readiness);
        self.check_feature_gate(name)
    }

    pub fn get_feature_gate_property(
        &self,
        name: impl Into<String>,
    ) -> Arc<MutableGateProperty> {
        let name = name.into();
        let current = self.check_feature_gate(&name);
        let mut properties = self
            .state
            .gate_properties
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(
            properties
                .entry(name)
                .or_insert_with(|| Arc::new(MutableGateProperty::new(current))),
        )
    }

    pub fn has_authenticated_statsig_bootstrap(&self) -> bool {
        self.state
            .has_authenticated_statsig_bootstrap
            .load(Ordering::Acquire)
    }

    pub fn has_live_statsig_bootstrap(&self) -> bool {
        self.state
            .readiness
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .has_live_statsig_bootstrap
    }

    pub fn has_hydrated_statsig_user_id(&self) -> bool {
        self.state
            .readiness
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .hydrated_user_id
            .as_deref()
            .is_some_and(|value| !value.is_empty())
    }

    pub fn wait_for_hydrated_statsig_user_id(&self, timeout: Duration) -> bool {
        let started = Instant::now();
        let mut readiness = self
            .state
            .readiness
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if readiness
                .hydrated_user_id
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            {
                return true;
            }
            if readiness.is_disposed {
                return false;
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return false;
            }
            let (next, result) = self
                .state
                .readiness_changed
                .wait_timeout(readiness, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            readiness = next;
            if result.timed_out() {
                return readiness
                    .hydrated_user_id
                    .as_deref()
                    .is_some_and(|value| !value.is_empty());
            }
        }
    }

    pub fn pin_gate_on_authenticated_bootstrap(
        &self,
        name: impl Into<String>,
        pin: Arc<dyn Fn(bool) + Send + Sync>,
    ) {
        let name = name.into();
        if self.has_authenticated_statsig_bootstrap() {
            pin(self.check_feature_gate(&name));
            return;
        }
        {
            let mut pins = self
                .state
                .authenticated_gate_pins
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !self.has_authenticated_statsig_bootstrap() {
                pins.push((name.clone(), Arc::clone(&pin)));
                return;
            }
        }
        pin(self.check_feature_gate(&name));
    }

    pub fn hydrate_statsig_bootstrap(
        &self,
        config: &str,
        authenticated_network: bool,
    ) -> Result<bool, String> {
        self.hydrate_statsig_bootstrap_source(config, true, authenticated_network)
    }

    fn hydrate_cached_statsig_bootstrap(&self, config: &str) -> Result<(), String> {
        self.hydrate_statsig_bootstrap_source(config, false, false)
            .map(|_| ())
    }

    fn hydrate_statsig_bootstrap_source(
        &self,
        config: &str,
        live_network: bool,
        authenticated_network: bool,
    ) -> Result<bool, String> {
        let parsed = parse_statsig_bootstrap(config)?;
        *self
            .state
            .network_feature_gates
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = parsed.feature_gates;
        *self
            .state
            .network_dynamic_configs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = parsed.dynamic_configs;
        *self
            .state
            .network_experiments
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = parsed.experiments;
        *self
            .state
            .statsig_user
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = parsed.user;

        let hydrated_user_id = parsed.user_id.filter(|value| !value.is_empty());
        let user_changed = {
            let mut readiness = self
                .state
                .readiness
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let changed = readiness.hydrated_user_id != hydrated_user_id;
            readiness.hydrated_user_id = hydrated_user_id.clone();
            if live_network {
                readiness.has_live_statsig_bootstrap = true;
            }
            changed
        };
        if user_changed {
            self.state
                .model_experiment_exposure_logged
                .store(false, Ordering::Release);
        }

        let authenticated = live_network
            && authenticated_network
            && hydrated_user_id.is_some();
        self.state
            .has_authenticated_statsig_bootstrap
            .store(authenticated, Ordering::Release);

        self.state.readiness_changed.notify_all();
        self.notify_listeners();
        if authenticated {
            let pins = std::mem::take(
                &mut *self
                    .state
                    .authenticated_gate_pins
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            );
            for (name, pin) in pins {
                pin(self.check_feature_gate(&name));
            }
        }
        Ok(authenticated)
    }

    fn begin_refresh(&self) -> bool {
        let mut readiness = self
            .state
            .readiness
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if readiness.is_disposed {
            return false;
        }
        readiness.is_refreshing = true;
        self.state.readiness_changed.notify_all();
        true
    }

    fn finish_refresh(&self) {
        let mut readiness = self
            .state
            .readiness
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        readiness.is_refreshing = false;
        self.state.readiness_changed.notify_all();
    }

    pub fn dispose(&self) {
        self.state
            .has_authenticated_statsig_bootstrap
            .store(false, Ordering::Release);
        self.state
            .model_experiment_exposure_logged
            .store(false, Ordering::Release);
        {
            let mut readiness = self
                .state
                .readiness
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            readiness.is_disposed = true;
            readiness.is_refreshing = false;
            readiness.has_live_statsig_bootstrap = false;
            readiness.hydrated_user_id = None;
        }
        self.state.readiness_changed.notify_all();

        self.state
            .listeners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        let properties = self
            .state
            .gate_properties
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for property in properties {
            property.clear_listeners();
        }
        self.state
            .authenticated_gate_pins
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.state
            .network_feature_gates
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.state
            .network_dynamic_configs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.state
            .network_experiments
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        *self
            .state
            .statsig_user
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    pub fn is_agent_network_enabled(&self) -> bool {
        self.check_feature_gate("sand_agent_network")
    }

    pub fn is_mcp_multi_account_enabled(&self) -> bool {
        self.check_feature_gate("mcp_multi_account")
    }

    pub fn is_sparse_plugin_clones_enabled(&self) -> bool {
        self.check_feature_gate("enable_sparse_plugin_clones")
    }

    pub fn is_multitask_enabled(&self) -> bool {
        resolve_boolean_env_override(
            std::env::var("SAND_MULTITASK").ok().as_deref(),
            self.check_feature_gate("sand_multitask"),
        )
    }

    pub fn is_send_message_delivery_owed_enabled(&self) -> bool {
        self.check_feature_gate("sand_send_message_delivery_owed")
    }

    pub fn is_dynamic_tools_enabled(&self) -> bool {
        self.check_feature_gate("grok_bot_dynamic_tools")
    }

    pub fn is_shared_room_box_tools_enabled(&self) -> bool {
        let env_enabled = resolve_boolean_env_override(
            std::env::var("SAND_SHARED_ROOM_BOX_TOOLS").ok().as_deref(),
            true,
        );
        env_enabled && !self.check_feature_gate("sand_shared_room_box_tools_kill_switch")
    }

    pub fn is_browser_use_subagent_enabled(&self) -> bool {
        self.check_feature_gate("sand_browser_use_subagent")
    }

    pub fn is_spotlight_enabled(&self) -> bool {
        resolve_boolean_env_override(
            std::env::var("SAND_SPOTLIGHT").ok().as_deref(),
            self.check_feature_gate("sand_spotlight"),
        )
    }

    pub fn is_unicode_typing_enabled(&self) -> bool {
        self.check_feature_gate("sand_computer_use_unicode_typing")
    }

    pub fn is_ua_token_kill_switch_enabled(&self) -> bool {
        self.check_feature_gate("sand_browser_ua_token_kill_switch")
    }

    pub fn get_sand_model_experiment_state(&self) -> Option<SandModelExperimentState> {
        if let Some(override_state) = read_sand_model_experiment_process_override() {
            return Some(override_state);
        }
        if !self.has_hydrated_statsig_user_id() {
            return None;
        }
        let experiments = self
            .state
            .network_experiments
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let assignment = experiments.get(SAND_MODEL_EXPERIMENT_NAME)?;
        assignment.group_name.as_deref()?;
        let enabled = assignment
            .value
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Some(SandModelExperimentState {
            active: true,
            arm: if enabled {
                SandModelExperimentArm::Treatment
            } else {
                SandModelExperimentArm::Control
            },
        })
    }

    pub fn log_sand_model_experiment_exposure(&self) -> bool {
        if !self.has_hydrated_statsig_user_id() {
            return false;
        }
        let assignment = self
            .state
            .network_experiments
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(SAND_MODEL_EXPERIMENT_NAME)
            .cloned();
        let Some(assignment) = assignment.filter(|value| value.group_name.is_some()) else {
            return false;
        };
        let user = self
            .state
            .statsig_user
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(user) = user else {
            return false;
        };

        if self
            .state
            .model_experiment_exposure_logged
            .swap(true, Ordering::AcqRel)
        {
            return true;
        }
        let scheduled = spawn_statsig_model_exposure(user, assignment);
        if !scheduled {
            self.state
                .model_experiment_exposure_logged
                .store(false, Ordering::Release);
        }
        scheduled
    }

    pub fn get_dynamic_config(&self, name: &str) -> BTreeMap<String, Value> {
        let mut config = bundled_dynamic_config_default(name);
        if let Some(values) = self
            .state
            .network_dynamic_configs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(name)
            .cloned()
        {
            for (key, value) in values {
                config.insert(key, value);
            }
        }
        if let Some(values) = self
            .state
            .dynamic_config_overrides
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(name)
            .cloned()
        {
            for (key, value) in values {
                config.insert(key, value);
            }
        }
        config
    }

    pub fn get_configured_default_model(&self) -> Option<SandAgentModelSelection> {
        configured_model_from_config(
            self.get_dynamic_config(SAND_DEFAULT_MODEL_CONFIG_NAME),
            self.has_hydrated_statsig_user_id(),
        )
    }

    pub fn get_configured_automations_model(&self) -> Option<SandAgentModelSelection> {
        configured_model_from_config(
            self.get_dynamic_config(SAND_AUTOMATIONS_MODEL_CONFIG_NAME),
            self.has_hydrated_statsig_user_id(),
        )
    }

    pub fn get_computer_use_model_override(&self) -> Option<BTreeMap<String, Value>> {
        self.check_feature_gate("sand_computer_use_playwright")
            .then(|| self.get_dynamic_config(SAND_COMPUTER_USE_CONFIG_NAME))
    }

    pub fn get_browser_use_model_override(&self) -> Option<BTreeMap<String, Value>> {
        self.check_feature_gate("sand_browser_use_subagent")
            .then(|| self.get_dynamic_config(SAND_BROWSER_USE_CONFIG_NAME))
    }

    pub fn replace_feature_flag_overrides(&self, overrides: BTreeMap<String, bool>) {
        *self
            .state
            .settings_overrides
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = overrides;
        self.notify_listeners();
    }

    pub fn replace_dynamic_config_overrides(
        &self,
        overrides: BTreeMap<String, BTreeMap<String, Value>>,
    ) {
        *self
            .state
            .dynamic_config_overrides
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = overrides;
        self.notify_listeners();
    }

    fn notify_listeners(&self) {
        let properties = self
            .state
            .gate_properties
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(name, property)| (name.clone(), Arc::clone(property)))
            .collect::<Vec<_>>();
        for (name, property) in properties {
            property.set(self.check_feature_gate(&name));
        }

        let listeners = self
            .state
            .listeners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for listener in listeners {
            listener();
        }
    }

    pub fn subscribe(&self, listener: Arc<dyn Fn() + Send + Sync>) -> StopSubscription {
        let id = self.state.next_listener_id.fetch_add(1, Ordering::SeqCst) + 1;
        self.state
            .listeners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id, listener);
        let state: Weak<ExperimentsState> = Arc::downgrade(&self.state);
        Box::new(move || {
            if let Some(state) = state.upgrade() {
                state
                    .listeners
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&id);
            }
        })
    }
}

impl BrowserUaExperimentsApi for HostExperimentsExtension {
    fn is_ua_token_kill_switch_enabled(&self) -> bool {
        HostExperimentsExtension::is_ua_token_kill_switch_enabled(self)
    }

    fn subscribe(&self, listener: Arc<dyn Fn() + Send + Sync>) -> StopSubscription {
        HostExperimentsExtension::subscribe(self, listener)
    }
}

pub fn start_host_experiments_extension() -> HostExperimentsExtension {
    HostExperimentsExtension::new(HostExperimentsOptions::from_process_env())
}

enum StatsigBootstrapCommand {
    Refresh,
    Stop,
}

pub struct ProductionStatsigBootstrapRuntime {
    stop: mpsc::Sender<StatsigBootstrapCommand>,
    worker: Mutex<Option<JoinHandle<()>>>,
    auth_stop: Mutex<Option<StopSubscription>>,
    experiments: Arc<HostExperimentsExtension>,
}

impl Drop for ProductionStatsigBootstrapRuntime {
    fn drop(&mut self) {
        if let Ok(stop) = self.auth_stop.get_mut() {
            if let Some(stop) = stop.take() {
                stop();
            }
        }
        let _ = self.stop.send(StatsigBootstrapCommand::Stop);
        if let Ok(worker) = self.worker.get_mut() {
            if let Some(worker) = worker.take() {
                let _ = worker.join();
            }
        }
        self.experiments.dispose();
    }
}

pub fn start_authenticated_statsig_bootstrap(
    experiments: Arc<HostExperimentsExtension>,
    auth: Arc<HostAuthExtension>,
    backend_url: String,
    cache_dir: PathBuf,
) -> Result<ProductionStatsigBootstrapRuntime, String> {
    let cache_path = cache_dir.join(STATSIG_BOOTSTRAP_CACHE_FILENAME);
    if let Some(config) = load_cached_statsig_bootstrap(&cache_path) {
        let _ = experiments.hydrate_cached_statsig_bootstrap(&config);
    }

    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(STATSIG_BOOTSTRAP_TIMEOUT)
        .build()
        .map_err(|error| format!("build Statsig bootstrap client: {error}"))?;
    let (tx, rx) = mpsc::channel();
    let auth_revision = Arc::new(AtomicU64::new(0));
    let worker_revision = Arc::clone(&auth_revision);
    let worker_experiments = Arc::clone(&experiments);
    let worker_auth = Arc::clone(&auth);
    let worker_cache = cache_path.clone();
    let poll_interval = statsig_poll_interval(experiments.state.options.is_dev_build);
    let worker = thread::Builder::new()
        .name("sand-experiments-statsig".into())
        .spawn(move || {
            let refresh = || -> Option<Duration> {
                if !worker_experiments.begin_refresh() {
                    return None;
                }
                let revision = worker_revision.load(Ordering::Acquire);
                let result = fetch_statsig_bootstrap(
                    &client,
                    &backend_url,
                    &worker_auth,
                );
                let retry_after = match result {
                    Ok(result) => {
                        if revision != worker_revision.load(Ordering::Acquire) {
                            None
                        } else if let Some(config) = result.config {
                            if worker_experiments
                                .hydrate_statsig_bootstrap_source(&config, true, true)
                                .is_ok()
                            {
                                let _ = save_cached_statsig_bootstrap(&worker_cache, &config);
                            }
                            None
                        } else {
                            result.retry_after
                        }
                    }
                    Err(error) => {
                        eprintln!("mahayana-host-experiments Statsig bootstrap failed: {error}");
                        None
                    }
                };
                worker_experiments.finish_refresh();
                retry_after
            };

            let mut next_delay = refresh()
                .map(|delay| delay.max(poll_interval))
                .unwrap_or(poll_interval);
            loop {
                match rx.recv_timeout(next_delay) {
                    Ok(StatsigBootstrapCommand::Refresh) => {
                        let mut stop = false;
                        while let Ok(command) = rx.try_recv() {
                            if matches!(command, StatsigBootstrapCommand::Stop) {
                                stop = true;
                                break;
                            }
                        }
                        if stop {
                            break;
                        }
                        next_delay = refresh()
                            .map(|delay| delay.max(poll_interval))
                            .unwrap_or(poll_interval);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        next_delay = refresh()
                            .map(|delay| delay.max(poll_interval))
                            .unwrap_or(poll_interval);
                    }
                    Ok(StatsigBootstrapCommand::Stop)
                    | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .map_err(|error| format!("start Statsig bootstrap worker: {error}"))?;

    let renewal_tx = tx.clone();
    let renewal_experiments = Arc::clone(&experiments);
    let renewal_revision = Arc::clone(&auth_revision);
    let auth_service = Arc::clone(auth.service());
    let subscription_id = auth_service.subscribe_to_renewal(Arc::new(move |event| {
        if event.result.outcome == RenewalOutcome::Renewed
            && (event.is_first_credential
                || !renewal_experiments.has_authenticated_statsig_bootstrap())
        {
            renewal_revision.fetch_add(1, Ordering::AcqRel);
            let _ = renewal_tx.send(StatsigBootstrapCommand::Refresh);
        }
    }));
    let auth_service_for_stop = Arc::clone(&auth_service);
    let auth_stop: StopSubscription = Box::new(move || {
        auth_service_for_stop.unsubscribe_from_renewal(subscription_id);
    });

    Ok(ProductionStatsigBootstrapRuntime {
        stop: tx,
        worker: Mutex::new(Some(worker)),
        auth_stop: Mutex::new(Some(auth_stop)),
        experiments,
    })
}

#[derive(Debug)]
struct StatsigBootstrapFetchResult {
    config: Option<String>,
    retry_after: Option<Duration>,
}

fn fetch_statsig_bootstrap(
    client: &Client,
    backend_url: &str,
    auth: &HostAuthExtension,
) -> Result<StatsigBootstrapFetchResult, String> {
    let base = Url::parse(backend_url)
        .map_err(|error| format!("invalid Statsig backend URL: {error}"))?;
    let url = base
        .join(STATSIG_BOOTSTRAP_PATH)
        .map_err(|error| format!("join Statsig bootstrap URL: {error}"))?;
    let machine_id = auth
        .get_machine_id()
        .map_err(|error| format!("read machine id for Statsig bootstrap: {error}"))?;
    let access_token = auth.get_access_token().ok();
    let mut request = client
        .post(url)
        .header("content-type", "application/json")
        .header("x-cursor-checksum", create_cursor_checksum(&machine_id, wall_clock_now_ms()))
        .header("x-cursor-client-type", SAND_CLIENT_TYPE)
        .header("x-cursor-client-version", sand_client_version())
        .header("x-sand-box-namespace", sand_box_namespace())
        .header("x-ghost-mode", "true")
        .header("x-request-id", Uuid::new_v4().to_string())
        .body("{}");
    if std::env::var("CURSOR_AGENT_CLI_LOCAL_MODE").as_deref() == Ok("true") {
        request = request.header("local-cli-mode", "true");
    }
    if let Some(token) = access_token.as_deref() {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .map_err(|error| format!("Statsig bootstrap transport: {error}"))?;
    if !response.status().is_success() {
        return Ok(StatsigBootstrapFetchResult {
            config: None,
            retry_after: parse_retry_after_header(
                response.headers().get(reqwest::header::RETRY_AFTER),
            ),
        });
    }
    let payload = response
        .json::<Value>()
        .map_err(|error| format!("parse Statsig bootstrap response: {error}"))?;
    Ok(StatsigBootstrapFetchResult {
        config: payload
            .get("config")
            .and_then(Value::as_str)
            .map(str::to_string),
        retry_after: None,
    })
}

fn parse_retry_after_header(
    value: Option<&reqwest::header::HeaderValue>,
) -> Option<Duration> {
    let raw = value?.to_str().ok()?.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(seconds) = raw.parse::<f64>() {
        if seconds.is_finite() {
            let millis = (seconds.max(0.0) * 1_000.0)
                .round()
                .min(u64::MAX as f64) as u64;
            return Some(Duration::from_millis(millis));
        }
    }
    let when = chrono::DateTime::parse_from_rfc2822(raw)
        .ok()?
        .with_timezone(&chrono::Utc);
    let delta_ms = when
        .signed_duration_since(chrono::Utc::now())
        .num_milliseconds()
        .max(0) as u64;
    Some(Duration::from_millis(delta_ms))
}

fn wall_clock_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

pub fn create_cursor_checksum(machine_id: &str, now_ms: u64) -> String {
    let unix_kilo_seconds = now_ms / 1_000_000;
    let shifts = [40u32, 32, 24, 16, 8, 0];
    let mut bytes = [0u8; 6];
    for (index, shift) in shifts.into_iter().enumerate() {
        bytes[index] = ((unix_kilo_seconds >> shift) & 0xff) as u8;
    }
    let mut last_byte = 165u8;
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = (*byte ^ last_byte).wrapping_add(index as u8);
        last_byte = *byte;
    }
    format!("{}{}", URL_SAFE_NO_PAD.encode(bytes), machine_id)
}

fn load_cached_statsig_bootstrap(path: &Path) -> Option<String> {
    let parsed = serde_json::from_str::<Value>(&fs::read_to_string(path).ok()?).ok()?;
    parsed
        .get("config")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn save_cached_statsig_bootstrap(path: &Path, config: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create Statsig cache directory: {error}"))?;
    }
    let parsed = parse_statsig_bootstrap(config)?;
    let payload = serde_json::to_vec(&serde_json::json!({
        "config": config,
        "userId": parsed.user_id,
        "fetchedAtMs": wall_clock_now_ms(),
    }))
    .map_err(|error| format!("serialize Statsig cache: {error}"))?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temp, payload)
        .map_err(|error| format!("write Statsig cache: {error}"))?;
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(&temp, path)
        .map_err(|error| format!("commit Statsig cache: {error}"))
}

#[derive(Debug)]
struct ParsedStatsigBootstrap {
    user_id: Option<String>,
    user: Option<Value>,
    feature_gates: BTreeMap<String, bool>,
    dynamic_configs: BTreeMap<String, BTreeMap<String, Value>>,
    experiments: BTreeMap<String, StatsigExperimentAssignment>,
}

fn parse_statsig_bootstrap(config: &str) -> Result<ParsedStatsigBootstrap, String> {
    let parsed = serde_json::from_str::<Value>(config)
        .map_err(|error| format!("parse Statsig config: {error}"))?;
    let user = parsed
        .get("user")
        .and_then(Value::as_object)
        .map(|value| Value::Object(value.clone()));
    let user_id = user
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|user| user.get("userID"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let feature_gates = parsed
        .get("feature_gates")
        .and_then(Value::as_object)
        .map(|gates| {
            gates
                .iter()
                .filter_map(|(name, value)| {
                    statsig_gate_value(value).map(|enabled| (name.clone(), enabled))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();

    let mut dynamic_configs = BTreeMap::new();
    let mut experiments = BTreeMap::new();
    if let Some(configs) = parsed.get("dynamic_configs").and_then(Value::as_object) {
        for (name, raw) in configs {
            let values = raw
                .get("value")
                .and_then(Value::as_object)
                .map(|values| {
                    values
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect::<BTreeMap<_, _>>()
                })
                .unwrap_or_default();
            dynamic_configs.insert(name.clone(), values.clone());
            experiments.insert(
                name.clone(),
                StatsigExperimentAssignment {
                    value: values,
                    group_name: raw
                        .get("group_name")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    rule_id: raw
                        .get("rule_id")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    secondary_exposures: raw
                        .get("secondary_exposures")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                },
            );
        }
    }

    Ok(ParsedStatsigBootstrap {
        user_id,
        user,
        feature_gates,
        dynamic_configs,
        experiments,
    })
}

fn statsig_gate_value(value: &Value) -> Option<bool> {
    value.as_bool().or_else(|| {
        value
            .get("value")
            .and_then(|value| value.as_bool().or_else(|| value.get("value").and_then(Value::as_bool)))
    })
}

fn env_gate_override(name: &str, raw: Option<&str>) -> Option<bool> {
    let raw = raw?;
    for pair in raw.split(',') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("").trim();
        let value = parts.next().unwrap_or("").trim();
        if key == name {
            return Some(matches!(value, "1" | "true" | "on"));
        }
    }
    None
}

fn resolve_boolean_env_override(raw: Option<&str>, fallback: bool) -> bool {
    let Some(raw) = raw.filter(|value| !value.is_empty()) else {
        return fallback;
    };
    raw != "0" && !raw.eq_ignore_ascii_case("false")
}

fn bundled_feature_gate_default(name: &str) -> bool {
    matches!(
        name,
        "enable_sparse_plugin_clones"
            | "sand_multitask"
            | "sand_spotlight"
            | "sand_computer_use_playwright"
    )
}

fn bundled_dynamic_config_default(name: &str) -> BTreeMap<String, Value> {
    match name {
        "grok_bot_conversation_size_limits" => BTreeMap::from([
            ("soft_limit_mb".into(), serde_json::json!(256)),
            ("hard_limit_mb".into(), serde_json::json!(1024)),
        ]),
        SAND_DEFAULT_MODEL_CONFIG_NAME | SAND_AUTOMATIONS_MODEL_CONFIG_NAME => {
            BTreeMap::from([
                ("modelId".into(), serde_json::json!("default")),
                ("maxMode".into(), serde_json::json!(false)),
                ("parameters".into(), serde_json::json!([])),
            ])
        }
        SAND_COMPUTER_USE_CONFIG_NAME | SAND_BROWSER_USE_CONFIG_NAME => {
            BTreeMap::from([
                ("modelId".into(), serde_json::json!("claude-opus-4-8")),
                ("maxMode".into(), serde_json::json!(false)),
                (
                    "parameters".into(),
                    serde_json::json!([
                        { "id": "thinking", "value": "false" },
                        { "id": "effort", "value": "low" }
                    ]),
                ),
            ])
        }
        "sand_model_filter" => BTreeMap::from([
            ("allowedModelIds".into(), serde_json::json!([])),
            ("defaultParameters".into(), serde_json::json!({})),
        ]),
        _ => BTreeMap::new(),
    }
}

fn configured_model_from_config(
    config: BTreeMap<String, Value>,
    has_hydrated_statsig_user_id: bool,
) -> Option<SandAgentModelSelection> {
    if !has_hydrated_statsig_user_id {
        return None;
    }
    let model_id = config.get("modelId")?.as_str()?.to_string();
    let max_mode = config.get("maxMode")?.as_bool()?;
    let raw_parameters = config.get("parameters")?.as_array()?;
    if model_id.is_empty() {
        return None;
    }
    if model_id.len() > 128
        || model_id
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        || raw_parameters.len() > 16
    {
        return None;
    }
    let mut parameter_ids = BTreeSet::new();
    let mut parameters = Vec::with_capacity(raw_parameters.len());
    for raw in raw_parameters {
        let raw = raw.as_object()?;
        let id = raw.get("id")?.as_str()?;
        let value = raw.get("value")?.as_str()?;
        if id.is_empty()
            || value.len() > 64
            || !parameter_ids.insert(id.to_string())
        {
            return None;
        }
        parameters.push(SandAgentModelParameter {
            id: id.to_string(),
            value: value.to_string(),
        });
    }
    let routed_model = matches!(
        model_id.as_str(),
        "default" | "premium" | "auto-low" | "auto-medium" | "auto-high" | "auto-smart"
    );
    if routed_model && (max_mode || !parameters.is_empty()) {
        return None;
    }
    Some(SandAgentModelSelection {
        model_id,
        max_mode,
        parameters,
    })
}

fn statsig_poll_interval(is_dev_build: bool) -> Duration {
    let base_ms = if is_dev_build {
        STATSIG_MIN_POLL_INTERVAL.as_millis() as u64
    } else {
        STATSIG_BASE_POLL_INTERVAL
            .max(STATSIG_MIN_POLL_INTERVAL)
            .as_millis() as u64
    };
    let jitter_cap = base_ms.saturating_mul(STATSIG_POLL_JITTER_PERCENT) / 100;
    let jitter = if jitter_cap == 0 {
        0
    } else {
        (Uuid::new_v4().as_u128() % jitter_cap as u128) as u64
    };
    Duration::from_millis(base_ms.saturating_add(jitter))
}

fn spawn_statsig_model_exposure(
    user: Value,
    assignment: StatsigExperimentAssignment,
) -> bool {
    let rule_id = assignment.rule_id.unwrap_or_default();
    let secondary_exposures = assignment.secondary_exposures;
    let event = serde_json::json!({
        "eventName": "statsig::config_exposure",
        "user": user,
        "time": wall_clock_now_ms(),
        "value": Value::Null,
        "metadata": {
            "config": SAND_MODEL_EXPERIMENT_NAME,
            "ruleID": rule_id,
            "reason": "Network:Recognized"
        },
        "secondaryExposures": secondary_exposures
    });
    let payload = serde_json::json!({
        "events": [event],
        "statsigMetadata": {
            "sdkType": "js-client",
            "sdkVersion": STATSIG_JS_SDK_VERSION
        }
    });
    let now = wall_clock_now_ms().to_string();
    let session_id = Uuid::new_v4().to_string();
    let url = format!("{}/rgstr", STATSIG_LOG_EVENT_PROXY_URL.trim_end_matches('/'));

    thread::Builder::new()
        .name("sand-experiments-exposure".into())
        .spawn(move || {
            let Ok(client) = Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(10))
                .build()
            else {
                return;
            };
            let _ = client
                .post(url)
                .query(&[
                    ("k", STATSIG_CLIENT_KEY),
                    ("st", "js-client"),
                    ("sv", STATSIG_JS_SDK_VERSION),
                    ("t", now.as_str()),
                    ("sid", session_id.as_str()),
                ])
                .header("statsig-event-count", "1")
                .header("statsig-retry-count", "0")
                .header("statsig-sdk-type", "js-client")
                .header("statsig-sdk-version", STATSIG_JS_SDK_VERSION)
                .json(&payload)
                .send();
        })
        .is_ok()
}
