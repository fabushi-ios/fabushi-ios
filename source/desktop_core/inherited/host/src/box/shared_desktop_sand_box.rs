use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::ports::r#box::{
    SAND_BOX_FIRST_FORK_WINDOW_INDEX, SAND_BOX_PRIMARY_WINDOW_INDEX, is_primary_window_index,
};

use super::box_env::BoxEnvironmentUpdate;
use super::box_windows::mint_sand_window_owner_token;
use super::generated_production::ProductionBoxResourceAccessor;
use super::loopback_sand_box::{
    LoopbackReady, LoopbackSandBox, LoopbackSandBoxError,
};

pub const ASSIGNMENTS_LOAD_TIMEOUT_MS: u64 = 30_000;
pub const ASSIGNMENTS_LOAD_RETRY_INTERVAL_MS: u64 = 1_000;
pub const DEFAULT_SHARED_BOX_ID: &str = "shared";
pub const SHARED_DESKTOP_ASSIGNMENTS_BOX_PATH: &str = "/home/box/.sand-window-assignments.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAssignments {
    pub assignments: BTreeMap<String, u32>,
    pub tokens: BTreeMap<String, String>,
    pub is_corrupt: bool,
}

pub fn parse_assignments(bytes: &[u8], max_window_count: u32) -> ParsedAssignments {
    let mut assignments = BTreeMap::new();
    let mut tokens = BTreeMap::new();
    let parsed: Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => {
            return ParsedAssignments {
                assignments,
                tokens,
                is_corrupt: true,
            };
        }
    };
    let Some(root) = parsed.as_object() else {
        return ParsedAssignments {
            assignments,
            tokens,
            is_corrupt: false,
        };
    };
    let Some(raw_assignments) = root.get("assignments").and_then(Value::as_object) else {
        return ParsedAssignments {
            assignments,
            tokens,
            is_corrupt: false,
        };
    };
    let raw_tokens = root.get("tokens").and_then(Value::as_object);
    let mut used_forks = HashSet::new();
    let mut agent_ids = raw_assignments.keys().collect::<Vec<_>>();
    agent_ids.sort();
    for agent_id in agent_ids {
        let Some(index) = raw_assignments.get(agent_id).and_then(Value::as_u64) else {
            continue;
        };
        let Ok(index) = u32::try_from(index) else {
            continue;
        };
        if index < 1 || index > max_window_count {
            continue;
        }
        if index >= SAND_BOX_FIRST_FORK_WINDOW_INDEX && !used_forks.insert(index) {
            continue;
        }
        assignments.insert(agent_id.clone(), index);
        if let Some(token) = raw_tokens
            .and_then(|values| values.get(agent_id))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            tokens.insert(agent_id.clone(), token.to_string());
        }
    }
    ParsedAssignments {
        assignments,
        tokens,
        is_corrupt: false,
    }
}

pub fn resolve_shared_box_id(explicit: Option<&str>) -> String {
    explicit
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var("SAND_SHARED_BOX_ID")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| DEFAULT_SHARED_BOX_ID.to_string())
}

#[derive(Debug, Default)]
struct SharedDesktopState {
    assignments_loaded: bool,
    agent_windows: BTreeMap<String, u32>,
    agent_window_tokens: BTreeMap<String, String>,
    established_forks: HashSet<String>,
    windows_tearing_down: HashSet<u32>,
}

#[derive(Debug, Clone)]
pub struct SharedDesktopSandBox {
    inner: LoopbackSandBox,
    shared_box_id: String,
    max_window_count: u32,
    persist_assignments: bool,
    state: Arc<Mutex<SharedDesktopState>>,
}

impl SharedDesktopSandBox {
    pub fn new(
        inner: LoopbackSandBox,
        shared_box_id: Option<&str>,
        persist_assignments: bool,
    ) -> Self {
        let max_window_count = inner.max_windows().max(1);
        Self {
            inner,
            shared_box_id: resolve_shared_box_id(shared_box_id),
            max_window_count,
            persist_assignments,
            state: Arc::new(Mutex::new(SharedDesktopState::default())),
        }
    }

    pub fn inner(&self) -> &LoopbackSandBox {
        &self.inner
    }

    pub fn shared_box_id(&self) -> &str {
        &self.shared_box_id
    }

    pub fn max_window_count(&self) -> u32 {
        self.max_window_count
    }

    pub fn persist_assignments(&self) -> bool {
        self.persist_assignments
    }

    pub fn get_agent_window_index(&self, agent_id: &str) -> Option<u32> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.agent_windows.get(agent_id).copied())
    }

    pub fn assign_window(&self, agent_id: &str) -> Option<u32> {
        let mut state = self.state.lock().ok()?;
        if let Some(index) = state.agent_windows.get(agent_id).copied() {
            return Some(index);
        }
        take_free_fork_index(&mut state, agent_id, self.max_window_count)
    }

    fn ensure_assignments_loaded<Ctx>(&self, ctx: &Ctx) -> Result<(), LoopbackSandBoxError> {
        if !self.persist_assignments {
            return Ok(());
        }
        {
            let state = self
                .state
                .lock()
                .map_err(|_| LoopbackSandBoxError::Transport("shared desktop state mutex poisoned".into()))?;
            if state.assignments_loaded {
                return Ok(());
            }
        }

        let deadline = Instant::now() + Duration::from_millis(ASSIGNMENTS_LOAD_TIMEOUT_MS);
        loop {
            match self
                .inner
                .download_file(ctx, &self.shared_box_id, SHARED_DESKTOP_ASSIGNMENTS_BOX_PATH)
            {
                Ok(bytes) => {
                    let parsed = parse_assignments(&bytes, self.max_window_count);
                    if parsed.assignments.is_empty()
                        && (bytes.is_empty() || parsed.is_corrupt)
                        && Instant::now() < deadline
                    {
                        thread::sleep(Duration::from_millis(
                            ASSIGNMENTS_LOAD_RETRY_INTERVAL_MS,
                        ));
                        continue;
                    }
                    let mut state = self.state.lock().map_err(|_| {
                        LoopbackSandBoxError::Transport("shared desktop state mutex poisoned".into())
                    })?;
                    let mut used = state.agent_windows.values().copied().collect::<HashSet<_>>();
                    for (agent_id, index) in parsed.assignments {
                        if state.agent_windows.contains_key(&agent_id) || used.contains(&index) {
                            continue;
                        }
                        state.agent_windows.insert(agent_id.clone(), index);
                        used.insert(index);
                        if let Some(token) = parsed.tokens.get(&agent_id) {
                            state
                                .agent_window_tokens
                                .entry(agent_id)
                                .or_insert_with(|| token.clone());
                        }
                    }
                    state.assignments_loaded = true;
                    return Ok(());
                }
                Err(LoopbackSandBoxError::Unreadable(_)) => {
                    if let Ok(mut state) = self.state.lock() {
                        state.assignments_loaded = true;
                    }
                    return Ok(());
                }
                Err(error) if Instant::now() < deadline => {
                    let _ = error;
                    thread::sleep(Duration::from_millis(
                        ASSIGNMENTS_LOAD_RETRY_INTERVAL_MS,
                    ));
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn snapshot_assignments(&self) -> Option<Vec<u8>> {
        let state = self.state.lock().ok()?;
        let assignments = state
            .agent_windows
            .iter()
            .map(|(agent_id, index)| (agent_id.clone(), Value::from(*index)))
            .collect::<Map<String, Value>>();
        let tokens = state
            .agent_windows
            .keys()
            .filter_map(|agent_id| {
                state
                    .agent_window_tokens
                    .get(agent_id)
                    .map(|token| (agent_id.clone(), Value::String(token.clone())))
            })
            .collect::<Map<String, Value>>();
        serde_json::to_vec(&serde_json::json!({
            "assignments": assignments,
            "tokens": tokens,
        }))
        .ok()
    }

    fn write_persisted_assignments<Ctx>(&self, ctx: &Ctx) {
        if !self.persist_assignments {
            return;
        }
        let Some(bytes) = self.snapshot_assignments() else {
            return;
        };
        let _ = self.inner.upload_file(
            ctx,
            &self.shared_box_id,
            SHARED_DESKTOP_ASSIGNMENTS_BOX_PATH,
            &bytes,
        );
    }

    fn migrate_legacy_primary_seat(&self, agent_id: &str, assigned: u32) -> u32 {
        if !is_primary_window_index(assigned) {
            return assigned;
        }
        let Ok(mut state) = self.state.lock() else {
            return assigned;
        };
        let current = state.agent_windows.get(agent_id).copied().unwrap_or(assigned);
        if !is_primary_window_index(current) {
            return current;
        }
        take_free_fork_index(&mut state, agent_id, self.max_window_count).unwrap_or(current)
    }

    pub fn ensure_ready<Ctx>(
        &self,
        ctx: &Ctx,
        agent_id: &str,
    ) -> Result<LoopbackReady, LoopbackSandBoxError> {
        self.ensure_assignments_loaded(ctx)?;
        let is_new_assignment = self.get_agent_window_index(agent_id).is_none();
        let assigned = self.assign_window(agent_id);
        let primary = self.inner.ensure_ready(ctx, &self.shared_box_id)?;
        let Some(assigned) = assigned else {
            return Ok(LoopbackReady {
                remote_accessor: primary.remote_accessor.with_no_monitor_computer_use(),
                vnc_url: primary.vnc_url,
                terminals_folder: primary.terminals_folder,
            });
        };
        let index = self.migrate_legacy_primary_seat(agent_id, assigned);
        let is_migration = index != assigned;
        if is_primary_window_index(index) {
            return Ok(LoopbackReady {
                remote_accessor: primary.remote_accessor.with_no_monitor_computer_use(),
                vnc_url: primary.vnc_url,
                terminals_folder: primary.terminals_folder,
            });
        }

        let token_is_new = {
            let mut state = self.state.lock().map_err(|_| {
                LoopbackSandBoxError::Transport("shared desktop state mutex poisoned".into())
            })?;
            if state.agent_window_tokens.contains_key(agent_id) {
                false
            } else {
                state
                    .agent_window_tokens
                    .insert(agent_id.to_string(), mint_sand_window_owner_token());
                true
            }
        };
        if is_new_assignment || token_is_new || is_migration {
            self.write_persisted_assignments(ctx);
        }
        let owner_token = self
            .state
            .lock()
            .ok()
            .and_then(|state| state.agent_window_tokens.get(agent_id).cloned());

        match self.inner.ensure_window(
            ctx,
            &self.shared_box_id,
            index,
            owner_token.as_deref(),
        ) {
            Ok(window) => {
                if let Ok(mut state) = self.state.lock() {
                    state.established_forks.insert(agent_id.to_string());
                }
                Ok(LoopbackReady {
                    remote_accessor: window.computer_use,
                    vnc_url: window.vnc_url,
                    terminals_folder: primary.terminals_folder,
                })
            }
            Err(error) => {
                let established = self
                    .state
                    .lock()
                    .ok()
                    .is_some_and(|state| state.established_forks.contains(agent_id));
                if is_migration && !established {
                    let _ = self.inner.release_window(ctx, &self.shared_box_id, index);
                    if let Ok(mut state) = self.state.lock() {
                        if state.agent_windows.get(agent_id) == Some(&index) {
                            state
                                .agent_windows
                                .insert(agent_id.to_string(), SAND_BOX_PRIMARY_WINDOW_INDEX);
                            state.agent_window_tokens.remove(agent_id);
                        }
                    }
                    self.write_persisted_assignments(ctx);
                    return Ok(LoopbackReady {
                        remote_accessor: primary.remote_accessor.with_no_monitor_computer_use(),
                        vnc_url: primary.vnc_url,
                        terminals_folder: primary.terminals_folder,
                    });
                }
                let _ = self.inner.release_window(ctx, &self.shared_box_id, index);
                if is_new_assignment {
                    if let Ok(mut state) = self.state.lock() {
                        state.agent_windows.remove(agent_id);
                        state.agent_window_tokens.remove(agent_id);
                    }
                    self.write_persisted_assignments(ctx);
                }
                Err(error)
            }
        }
    }

    pub fn release_window<Ctx>(
        &self,
        ctx: &Ctx,
        agent_id: &str,
    ) -> Result<(), LoopbackSandBoxError> {
        let index = {
            let mut state = self.state.lock().map_err(|_| {
                LoopbackSandBoxError::Transport("shared desktop state mutex poisoned".into())
            })?;
            let index = state.agent_windows.remove(agent_id);
            state.agent_window_tokens.remove(agent_id);
            state.established_forks.remove(agent_id);
            if let Some(index) = index {
                state.windows_tearing_down.insert(index);
            }
            index
        };
        if index.is_some() {
            self.write_persisted_assignments(ctx);
        }
        if let Some(index) = index.filter(|index| *index >= SAND_BOX_FIRST_FORK_WINDOW_INDEX) {
            let result = self.inner.release_window(ctx, &self.shared_box_id, index);
            if let Ok(mut state) = self.state.lock() {
                state.windows_tearing_down.remove(&index);
            }
            result?;
        }
        Ok(())
    }

    pub fn apply_environment<Ctx>(
        &self,
        ctx: &Ctx,
        update: &BoxEnvironmentUpdate,
    ) -> Result<(), LoopbackSandBoxError> {
        self.inner.apply_environment(ctx, update)
    }

    pub fn load_mcp_servers<Ctx>(
        &self,
        ctx: &Ctx,
        config_json: &str,
    ) -> Result<Vec<String>, LoopbackSandBoxError> {
        self.inner.load_mcp_servers(ctx, config_json)
    }

    pub fn mcp_resource_accessor<Ctx>(
        &self,
        ctx: &Ctx,
    ) -> Result<ProductionBoxResourceAccessor, LoopbackSandBoxError> {
        self.inner.mcp_resource_accessor(ctx)
    }

    pub fn upload_file<Ctx>(
        &self,
        ctx: &Ctx,
        _agent_id: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), LoopbackSandBoxError> {
        self.inner.upload_file(ctx, &self.shared_box_id, path, data)
    }

    pub fn download_file<Ctx>(
        &self,
        ctx: &Ctx,
        _agent_id: &str,
        path: &str,
    ) -> Result<Vec<u8>, LoopbackSandBoxError> {
        self.inner.download_file(ctx, &self.shared_box_id, path)
    }

    pub fn run_state(&self) -> &'static str {
        self.inner.run_state()
    }

    pub fn list_boxes(&self) -> Vec<(String, bool)> {
        let running = self.inner.list_boxes().iter().any(|(_, running)| *running);
        self.state
            .lock()
            .map(|state| {
                state
                    .agent_windows
                    .keys()
                    .map(|agent_id| (agent_id.clone(), running))
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn take_free_fork_index(
    state: &mut SharedDesktopState,
    agent_id: &str,
    max_window_count: u32,
) -> Option<u32> {
    let used = state.agent_windows.values().copied().collect::<HashSet<_>>();
    for index in SAND_BOX_FIRST_FORK_WINDOW_INDEX..=max_window_count {
        if !used.contains(&index) && !state.windows_tearing_down.contains(&index) {
            state.agent_windows.insert(agent_id.to_string(), index);
            return Some(index);
        }
    }
    None
}
