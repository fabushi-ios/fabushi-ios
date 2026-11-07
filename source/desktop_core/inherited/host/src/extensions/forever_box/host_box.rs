use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::r#box::box_env::BoxEnvironmentUpdate;
use crate::r#box::generated_production::ProductionBoxResourceAccessor;
use crate::r#box::loopback_sand_box::{LoopbackReady, LoopbackSandBoxError};
use crate::r#box::production::ProductionBoxEnvironment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxWindowStatus {
    pub window_index: u32,
    pub vnc_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxStatus {
    pub agent_id: String,
    pub state: String,
    pub vnc_url: Option<String>,
    pub windows: Option<Vec<BoxWindowStatus>>,
    pub image_update_available: Option<bool>,
    pub pull_percent: Option<u8>,
}

pub type HostBoxStatusListener = Arc<dyn Fn(&BoxStatus) + Send + Sync + 'static>;

#[derive(Default)]
struct HostBoxState {
    vnc_urls: BTreeMap<String, String>,
    fork_vnc_urls: BTreeMap<String, BTreeMap<u32, String>>,
    last_reported: BTreeMap<String, BoxStatus>,
    connection_epochs: BTreeMap<String, u64>,
    image_update_available: Option<bool>,
    listeners: BTreeMap<u64, HostBoxStatusListener>,
    next_listener_id: u64,
}

#[derive(Clone)]
pub struct HostBox {
    inner: ProductionBoxEnvironment,
    state: Arc<Mutex<HostBoxState>>,
}

impl HostBox {
    pub fn new(inner: ProductionBoxEnvironment) -> Self {
        Self {
            inner,
            state: Arc::new(Mutex::new(HostBoxState::default())),
        }
    }

    pub fn inner(&self) -> &ProductionBoxEnvironment {
        &self.inner
    }

    pub fn subscribe(&self, listener: HostBoxStatusListener) -> u64 {
        let mut state = self.state.lock().expect("HostBox state mutex poisoned");
        state.next_listener_id = state.next_listener_id.saturating_add(1);
        let id = state.next_listener_id;
        state.listeners.insert(id, listener);
        id
    }

    pub fn unsubscribe(&self, subscription: u64) {
        if let Ok(mut state) = self.state.lock() {
            state.listeners.remove(&subscription);
        }
    }

    fn build_windows_locked(state: &HostBoxState, agent_id: &str) -> Option<Vec<BoxWindowStatus>> {
        let main = state.vnc_urls.get(agent_id);
        let forks = state.fork_vnc_urls.get(agent_id);
        if main.is_none() && forks.is_none_or(BTreeMap::is_empty) {
            return None;
        }
        let mut windows = Vec::new();
        if let Some(vnc_url) = main {
            windows.push(BoxWindowStatus {
                window_index: 0,
                vnc_url: vnc_url.clone(),
            });
        }
        if let Some(forks) = forks {
            windows.extend(forks.iter().map(|(window_index, vnc_url)| BoxWindowStatus {
                window_index: *window_index,
                vnc_url: vnc_url.clone(),
            }));
        }
        Some(windows)
    }

    fn running_status_locked(state: &HostBoxState, agent_id: &str, vnc_url: &str) -> BoxStatus {
        BoxStatus {
            agent_id: agent_id.to_string(),
            state: "running".into(),
            vnc_url: Some(vnc_url.to_string()),
            windows: Self::build_windows_locked(state, agent_id),
            image_update_available: state.image_update_available,
            pull_percent: None,
        }
    }

    fn notify(&self, status: BoxStatus) {
        let listeners = {
            let mut state = self.state.lock().expect("HostBox state mutex poisoned");
            state.last_reported.insert(status.agent_id.clone(), status.clone());
            state.listeners.values().cloned().collect::<Vec<_>>()
        };
        for listener in listeners {
            listener(&status);
        }
    }

    fn record_connection(&self, agent_id: &str, vnc_url: &str) {
        let mut state = self.state.lock().expect("HostBox state mutex poisoned");
        state.vnc_urls.insert(agent_id.to_string(), vnc_url.to_string());
        let epoch = state.connection_epochs.entry(agent_id.to_string()).or_default();
        *epoch = epoch.saturating_add(1);
    }

    pub fn record_image_update_available(&self, value: Option<bool>) {
        let statuses = {
            let mut state = self.state.lock().expect("HostBox state mutex poisoned");
            if value.is_none() || value == state.image_update_available {
                return;
            }
            state.image_update_available = value;
            let agents = state.last_reported.keys().cloned().collect::<Vec<_>>();
            agents
                .into_iter()
                .filter_map(|agent_id| {
                    if let Some(vnc_url) = state.vnc_urls.get(&agent_id) {
                        Some(Self::running_status_locked(&state, &agent_id, vnc_url))
                    } else {
                        state.last_reported.get(&agent_id).cloned().map(|mut status| {
                            status.image_update_available = value;
                            status
                        })
                    }
                })
                .collect::<Vec<_>>()
        };
        for status in statuses {
            self.notify(status);
        }
    }

    pub fn ensure_ready(&self, agent_id: &str) -> Result<LoopbackReady, LoopbackSandBoxError> {
        let connection = self.inner.ensure_ready(agent_id)?;
        self.record_connection(agent_id, &connection.vnc_url);
        let status = {
            let state = self.state.lock().expect("HostBox state mutex poisoned");
            Self::running_status_locked(&state, agent_id, &connection.vnc_url)
        };
        self.notify(status);
        Ok(connection)
    }

    pub fn ensure(&self, agent_id: &str) -> Result<BoxStatus, LoopbackSandBoxError> {
        let connection = self.ensure_ready(agent_id)?;
        let state = self.state.lock().expect("HostBox state mutex poisoned");
        Ok(Self::running_status_locked(&state, agent_id, &connection.vnc_url))
    }

    pub fn get_status(&self, agent_id: &str) -> BoxStatus {
        let running = self.inner.run_state() == "running";
        let status = {
            let mut state = self.state.lock().expect("HostBox state mutex poisoned");
            if !running {
                state.vnc_urls.remove(agent_id);
                state.fork_vnc_urls.remove(agent_id);
                BoxStatus {
                    agent_id: agent_id.to_string(),
                    state: self.inner.run_state().into(),
                    vnc_url: None,
                    windows: None,
                    image_update_available: state.image_update_available,
                    pull_percent: None,
                }
            } else if let Some(vnc_url) = state.vnc_urls.get(agent_id).cloned() {
                Self::running_status_locked(&state, agent_id, &vnc_url)
            } else {
                BoxStatus {
                    agent_id: agent_id.to_string(),
                    state: "absent".into(),
                    vnc_url: None,
                    windows: None,
                    image_update_available: state.image_update_available,
                    pull_percent: None,
                }
            }
        };
        self.notify(status.clone());
        status
    }

    pub fn release_window(&self, agent_id: &str) {
        let epoch = self
            .state
            .lock()
            .ok()
            .and_then(|state| state.connection_epochs.get(agent_id).copied())
            .unwrap_or_default();
        let _ = self.inner.release_window(agent_id);
        let should_clear = self
            .state
            .lock()
            .ok()
            .and_then(|state| state.connection_epochs.get(agent_id).copied())
            .unwrap_or_default()
            == epoch;
        if !should_clear {
            return;
        }
        if let Ok(mut state) = self.state.lock() {
            state.vnc_urls.remove(agent_id);
            state.fork_vnc_urls.remove(agent_id);
            state.connection_epochs.remove(agent_id);
        }
        self.notify(BoxStatus {
            agent_id: agent_id.to_string(),
            state: "absent".into(),
            vnc_url: None,
            windows: None,
            image_update_available: self.image_update_available(),
            pull_percent: None,
        });
        if let Ok(mut state) = self.state.lock() {
            state.last_reported.remove(agent_id);
        }
    }

    pub fn image_update_available(&self) -> Option<bool> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.image_update_available)
    }

    pub fn is_box_running(&self) -> bool {
        self.inner.run_state() == "running"
    }

    pub fn list_boxes(&self) -> Vec<(String, bool)> {
        self.inner.list_boxes()
    }

    pub fn get_agent_window_index(&self, agent_id: &str) -> Option<u32> {
        self.inner.get_agent_window_index(agent_id)
    }

    pub fn terminals_folder(&self) -> &'static str {
        self.inner.terminals_folder()
    }

    pub fn is_available(&self) -> bool {
        self.inner.is_available()
    }

    pub fn apply_environment(
        &self,
        update: &BoxEnvironmentUpdate,
    ) -> Result<(), LoopbackSandBoxError> {
        self.inner.apply_environment(update)
    }

    pub fn load_mcp_servers(
        &self,
        config_json: &str,
    ) -> Result<Vec<String>, LoopbackSandBoxError> {
        self.inner.load_mcp_servers(config_json)
    }

    pub fn mcp_resource_accessor(
        &self,
    ) -> Result<ProductionBoxResourceAccessor, LoopbackSandBoxError> {
        self.inner.mcp_resource_accessor()
    }

    pub fn upload_file(
        &self,
        agent_id: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), LoopbackSandBoxError> {
        self.inner.upload_file(agent_id, path, data)
    }

    pub fn download_file(
        &self,
        agent_id: &str,
        path: &str,
    ) -> Result<Vec<u8>, LoopbackSandBoxError> {
        self.inner.download_file(agent_id, path)
    }
}
