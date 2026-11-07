use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};

use crate::extensions::auth::credential_renewer::RenewalOutcome;
use crate::extensions::auth::extension::HostAuthExtension;
use crate::extensions::browser_ua::extension::StopSubscription;
use crate::extensions::experiments::HostExperimentsExtension;

use super::codebase_telemetry_adapter::{
    CsnapsAdapterError, CsnapsCodebaseTelemetryAdapter,
};
use super::privacy_mode::{
    CodebaseTelemetryPrivacyMode, TelemetryAuth,
};

pub const SAND_CODEBASE_TELEMETRY_FEATURE_GATE: &str = "sand_codebase_telemetry";
pub const FEATURE_GATE_MAIN: &str = "main";
pub const FEATURE_GATE_GIT_HISTORY: &str = "git-history";
pub const FEATURE_GATE_AGENT_DOT_DIRS: &str = "agent-dot-dirs";
pub const CODEBASE_KIND_WORKSPACE_ROOT: u64 = 1;
pub const CODEBASE_KIND_HOME_DIRECTORY: u64 = 2;
pub const SAND_BOX_WORKSPACE_ROOT: &str = "/workspace";
pub const SAND_BOX_HOME_DIR: &str = "/home/box";

pub type AuthListener = Arc<dyn Fn(Option<TelemetryAuth>) + Send + Sync>;
pub type FeatureGateListener = Arc<dyn Fn(Vec<String>) + Send + Sync>;
pub type AdapterFactory = Arc<
    dyn Fn(TelemetryAuth) -> Result<Arc<CsnapsCodebaseTelemetryAdapter>, CsnapsAdapterError>
        + Send
        + Sync,
>;

struct AuthSourceState {
    value: Option<TelemetryAuth>,
    listeners: BTreeMap<u64, AuthListener>,
    next_id: AtomicU64,
}

pub struct AuthSubscription {
    state: Weak<Mutex<AuthSourceState>>,
    id: u64,
}

impl Drop for AuthSubscription {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            state
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .listeners
                .remove(&self.id);
        }
    }
}

struct GateSourceState {
    listeners: BTreeMap<u64, FeatureGateListener>,
    next_id: AtomicU64,
}

pub struct FeatureGateSubscription {
    state: Weak<Mutex<GateSourceState>>,
    id: u64,
}

impl Drop for FeatureGateSubscription {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            state
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .listeners
                .remove(&self.id);
        }
    }
}

pub struct SandCodebaseTelemetryHost {
    auth: Arc<HostAuthExtension>,
    experiments: Arc<HostExperimentsExtension>,
    privacy_mode: Arc<CodebaseTelemetryPrivacyMode>,
    adapter_factory: AdapterFactory,
    auth_source: Arc<Mutex<AuthSourceState>>,
    gate_source: Arc<Mutex<GateSourceState>>,
    auth_renewal_subscription: Mutex<Option<u64>>,
    experiments_stop: Mutex<Option<StopSubscription>>,
}

impl SandCodebaseTelemetryHost {
    pub fn new(
        auth: Arc<HostAuthExtension>,
        experiments: Arc<HostExperimentsExtension>,
        privacy_mode: Arc<CodebaseTelemetryPrivacyMode>,
        adapter_factory: AdapterFactory,
    ) -> Arc<Self> {
        let host = Arc::new(Self {
            auth,
            experiments,
            privacy_mode,
            adapter_factory,
            auth_source: Arc::new(Mutex::new(AuthSourceState {
                value: None,
                listeners: BTreeMap::new(),
                next_id: AtomicU64::new(1),
            })),
            gate_source: Arc::new(Mutex::new(GateSourceState {
                listeners: BTreeMap::new(),
                next_id: AtomicU64::new(1),
            })),
            auth_renewal_subscription: Mutex::new(None),
            experiments_stop: Mutex::new(None),
        });
        host.install_sources();
        host
    }

    pub fn auth(&self) -> Option<TelemetryAuth> {
        self.auth_source
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .value
            .clone()
    }

    pub fn subscribe_auth(&self, listener: AuthListener) -> AuthSubscription {
        let mut state = self
            .auth_source
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let id = state.next_id.fetch_add(1, Ordering::Relaxed);
        state.listeners.insert(id, listener);
        AuthSubscription {
            state: Arc::downgrade(&self.auth_source),
            id,
        }
    }

    pub fn check_feature_gate(&self, gate: &str) -> bool {
        if gate != FEATURE_GATE_MAIN {
            return false;
        }
        self.experiments
            .check_feature_gate(SAND_CODEBASE_TELEMETRY_FEATURE_GATE)
    }

    pub fn subscribe_feature_gate_changes(
        &self,
        listener: FeatureGateListener,
    ) -> FeatureGateSubscription {
        let mut state = self
            .gate_source
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let id = state.next_id.fetch_add(1, Ordering::Relaxed);
        state.listeners.insert(id, listener);
        FeatureGateSubscription {
            state: Arc::downgrade(&self.gate_source),
            id,
        }
    }

    pub fn desired_codebases(&self) -> Vec<Value> {
        desired_codebases()
    }

    pub fn create_adapter(
        &self,
        auth: TelemetryAuth,
    ) -> Result<Arc<CsnapsCodebaseTelemetryAdapter>, CsnapsAdapterError> {
        (self.adapter_factory)(auth)
    }

    pub fn privacy_mode(&self) -> &Arc<CodebaseTelemetryPrivacyMode> {
        &self.privacy_mode
    }

    pub fn dispose(&self) {
        if let Some(id) = self
            .auth_renewal_subscription
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take()
        {
            self.auth.service().unsubscribe_from_renewal(id);
        }
        if let Some(stop) = self
            .experiments_stop
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take()
        {
            stop();
        }
        self.auth_source
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .listeners
            .clear();
        self.gate_source
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .listeners
            .clear();
    }

    fn install_sources(self: &Arc<Self>) {
        self.sync_auth();

        let weak = Arc::downgrade(self);
        let subscription = self.auth.service().subscribe_to_renewal(Arc::new(move |event| {
            let Some(host) = weak.upgrade() else {
                return;
            };
            if event.result.outcome == RenewalOutcome::Renewed
                || host.auth.peek_access_token().is_none()
            {
                host.sync_auth();
            }
        }));
        *self
            .auth_renewal_subscription
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(subscription);

        let last = Arc::new(Mutex::new(
            self.experiments
                .check_feature_gate(SAND_CODEBASE_TELEMETRY_FEATURE_GATE),
        ));
        let weak = Arc::downgrade(self);
        let last_for_listener = Arc::clone(&last);
        let stop = self.experiments.subscribe(Arc::new(move || {
            let Some(host) = weak.upgrade() else {
                return;
            };
            let next = host
                .experiments
                .check_feature_gate(SAND_CODEBASE_TELEMETRY_FEATURE_GATE);
            let mut previous = last_for_listener
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if *previous == next {
                return;
            }
            *previous = next;
            drop(previous);
            let listeners = host
                .gate_source
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .listeners
                .values()
                .cloned()
                .collect::<Vec<_>>();
            for listener in listeners {
                listener(vec![FEATURE_GATE_MAIN.to_string()]);
            }
        }));
        *self
            .experiments_stop
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(stop);
    }

    fn sync_auth(&self) {
        let next = self
            .auth
            .peek_access_token()
            .and_then(|token| telemetry_auth_from_token(&token).map(|auth_id| TelemetryAuth {
                auth_id,
                auth_token: token,
            }));

        let (changed, listeners) = {
            let mut state = self
                .auth_source
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if state.value == next {
                return;
            }
            let changed = state
                .value
                .as_ref()
                .map(|value| value.auth_id.as_str())
                != next.as_ref().map(|value| value.auth_id.as_str());
            state.value = next.clone();
            (
                changed,
                state.listeners.values().cloned().collect::<Vec<_>>(),
            )
        };

        match next.clone() {
            Some(auth) => self.privacy_mode.set_auth(auth),
            None if changed => self.privacy_mode.clear_auth(),
            None => {}
        }
        for listener in listeners {
            listener(next.clone());
        }
    }
}

impl Drop for SandCodebaseTelemetryHost {
    fn drop(&mut self) {
        self.dispose();
    }
}

pub fn telemetry_auth_from_token(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let parsed: Value = serde_json::from_slice(&bytes).ok()?;
    parsed
        .get("sub")
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub fn desired_codebases() -> Vec<Value> {
    vec![
        json!({
            "path": SAND_BOX_WORKSPACE_ROOT,
            "kind": CODEBASE_KIND_WORKSPACE_ROOT,
        }),
        json!({
            "path": SAND_BOX_HOME_DIR,
            "kind": CODEBASE_KIND_HOME_DIRECTORY,
        }),
    ]
}
