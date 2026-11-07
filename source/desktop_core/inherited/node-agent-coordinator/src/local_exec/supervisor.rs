use super::daemon_files::LocalExecDaemonDiscovery;

pub const LOCAL_EXEC_DAEMON_REFRESH_INTERVAL_MS: u64 = 30_000;
pub const LOCAL_EXEC_DAEMON_LIVENESS_INTERVAL_MS: u64 = 1_000;
pub const LOCAL_EXEC_DAEMON_READINESS_TIMEOUT_MS: u64 = 5_000;
pub const LOCAL_EXEC_DAEMON_READINESS_POLL_MS: u64 = 50;
pub const LOCAL_EXEC_DAEMON_DISCOVERY_MISS_LIMIT: u32 = 2;
pub const LOCAL_EXEC_DAEMON_RESPAWN_LIMIT: u32 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalExecDaemonAction {
    Spawn,
    Adopt { pid: u32 },
    Replace { pid: u32 },
}

pub fn decide_local_exec_daemon_action(
    existing: Option<&LocalExecDaemonDiscovery>,
) -> LocalExecDaemonAction {
    match existing {
        None => LocalExecDaemonAction::Spawn,
        Some(existing) if existing.inflight_count.unwrap_or(0) > 0 => {
            LocalExecDaemonAction::Adopt { pid: existing.pid }
        }
        Some(existing) => LocalExecDaemonAction::Replace { pid: existing.pid },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalExecDaemonOrigin {
    Spawned,
    Adopted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalExecDaemonState {
    Absent,
    Adopting { pid: u32 },
    Replacing { pid: u32 },
    Active {
        origin: LocalExecDaemonOrigin,
        pid: u32,
        process_start_epoch_ms: u64,
        command: String,
        entry_realpath: String,
        generation_token: String,
    },
    Failed { reason: String },
}

impl Default for LocalExecDaemonState {
    fn default() -> Self {
        Self::Absent
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LocalExecSupervisor {
    generation: u64,
    running: bool,
    restart_count: u32,
    state: LocalExecDaemonState,
}

impl LocalExecSupervisor {
    pub fn start(&mut self) -> u64 {
        self.generation = self.generation.saturating_add(1);
        self.running = true;
        self.generation
    }

    pub fn crash_and_restart(&mut self) -> u64 {
        self.running = false;
        self.restart_count = self.restart_count.saturating_add(1);
        if self.restart_count > LOCAL_EXEC_DAEMON_RESPAWN_LIMIT {
            self.state = LocalExecDaemonState::Failed {
                reason: "respawn limit reached".to_string(),
            };
            return self.generation;
        }
        self.start()
    }

    pub fn stop(&mut self) {
        self.running = false;
        self.state = LocalExecDaemonState::Absent;
    }

    pub fn reconcile_discovery(
        &mut self,
        existing: Option<&LocalExecDaemonDiscovery>,
    ) -> LocalExecDaemonAction {
        let action = decide_local_exec_daemon_action(existing);
        self.state = match action {
            LocalExecDaemonAction::Spawn => LocalExecDaemonState::Absent,
            LocalExecDaemonAction::Adopt { pid } => LocalExecDaemonState::Adopting { pid },
            LocalExecDaemonAction::Replace { pid } => LocalExecDaemonState::Replacing { pid },
        };
        action
    }

    pub fn mark_active(
        &mut self,
        origin: LocalExecDaemonOrigin,
        pid: u32,
        process_start_epoch_ms: u64,
        command: impl Into<String>,
        entry_realpath: impl Into<String>,
        generation_token: impl Into<String>,
    ) -> Result<(), &'static str> {
        let command = command.into();
        let entry_realpath = entry_realpath.into();
        let generation_token = generation_token.into();
        if pid == 0
            || process_start_epoch_ms == 0
            || command.trim().is_empty()
            || entry_realpath.trim().is_empty()
            || generation_token.trim().is_empty()
        {
            return Err("active local-exec daemon identity is incomplete");
        }
        self.running = true;
        self.state = LocalExecDaemonState::Active {
            origin,
            pid,
            process_start_epoch_ms,
            command,
            entry_realpath,
            generation_token,
        };
        Ok(())
    }

    pub fn mark_failed(&mut self, reason: impl Into<String>) {
        self.running = false;
        self.state = LocalExecDaemonState::Failed {
            reason: reason.into(),
        };
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn restart_count(&self) -> u32 {
        self.restart_count
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn state(&self) -> &LocalExecDaemonState {
        &self.state
    }
}


pub const SAND_CLIENT_PAUSE_REASON: &str = "SAND_CLIENT_PAUSE";
pub const LOCAL_EXEC_GENERATION_TOKEN_ENV: &str = "SAND_LOCAL_EXEC_GENERATION_TOKEN";
pub const LOCAL_EXEC_GENERATION_TOKEN_ARG: &str = "--sand-local-exec-generation=";
pub const LOCAL_EXEC_DAEMON_PUBLICATION_LAG_MS: u64 = 60_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalExecProcessIdentity {
    pub pid: u32,
    pub start_epoch_ms: u64,
    pub command: String,
    pub entry_realpath: String,
    pub generation_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedLocalExecProcessIdentity {
    pub pid: u32,
    pub entry_realpath: String,
    pub generation_token: String,
    pub start_epoch_ms: Option<u64>,
    pub command: Option<String>,
    pub discovery_started_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnLocalExecDaemonRequest {
    pub env: std::collections::BTreeMap<String, String>,
    pub log_path: std::path::PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct LocalExecSupervisorError {
    pub message: String,
}

impl LocalExecSupervisorError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl From<std::io::Error> for LocalExecSupervisorError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

pub trait LocalExecControl {
    fn now_ms(&self) -> u64;
    fn sleep_ms(&mut self, duration_ms: u64);
    fn resolve_gateway_connection(&mut self) -> Result<serde_json::Value, LocalExecSupervisorError>;
    fn mint_local_exec_daemon_credential(
        &mut self,
    ) -> Result<Option<serde_json::Value>, LocalExecSupervisorError>;
    fn spawn_local_exec_daemon(
        &mut self,
        request: SpawnLocalExecDaemonRequest,
    ) -> Result<LocalExecProcessIdentity, LocalExecSupervisorError>;
    fn is_process_alive(&mut self, pid: u32) -> Result<bool, LocalExecSupervisorError>;
    fn get_process_identity(
        &mut self,
        expected: &ExpectedLocalExecProcessIdentity,
    ) -> Result<Option<LocalExecProcessIdentity>, LocalExecSupervisorError>;
    fn terminate_process(
        &mut self,
        identity: &LocalExecProcessIdentity,
    ) -> Result<bool, LocalExecSupervisorError>;
}

fn contains_exact_argument(command: &str, argument: &str) -> bool {
    if argument.is_empty() {
        return false;
    }
    let bytes = command.as_bytes();
    let mut offset = 0usize;
    while offset <= command.len().saturating_sub(argument.len()) {
        let Some(relative) = command[offset..].find(argument) else {
            return false;
        };
        let found = offset + relative;
        let end = found + argument.len();
        let before = found == 0 || bytes[found - 1].is_ascii_whitespace();
        let after = end == bytes.len() || bytes[end].is_ascii_whitespace();
        if before && after {
            return true;
        }
        offset = found.saturating_add(1);
    }
    false
}

pub fn command_carries_local_exec_generation(
    command: &str,
    entry_realpath: &str,
    generation_token: &str,
) -> bool {
    !entry_realpath.is_empty()
        && !generation_token.is_empty()
        && contains_exact_argument(command, entry_realpath)
        && contains_exact_argument(
            command,
            &format!("{LOCAL_EXEC_GENERATION_TOKEN_ARG}{generation_token}"),
        )
}

pub fn same_local_exec_process_identity(
    left: &LocalExecProcessIdentity,
    right: &LocalExecProcessIdentity,
) -> bool {
    left == right
}

pub fn local_exec_discovery_time_matches_process(
    discovery_started_at: f64,
    process_start_epoch_ms: u64,
    observed_at_ms: u64,
) -> bool {
    if !discovery_started_at.is_finite() || discovery_started_at < 0.0 {
        return false;
    }
    let discovery_started_at = discovery_started_at as u64;
    discovery_started_at >= process_start_epoch_ms
        && discovery_started_at.saturating_sub(process_start_epoch_ms)
            <= LOCAL_EXEC_DAEMON_PUBLICATION_LAG_MS
        && discovery_started_at <= observed_at_ms
}

fn identity_is_valid(identity: &LocalExecProcessIdentity) -> bool {
    identity.pid > 0
        && identity.start_epoch_ms > 0
        && !identity.command.trim().is_empty()
        && !identity.entry_realpath.trim().is_empty()
        && !identity.generation_token.trim().is_empty()
        && command_carries_local_exec_generation(
            &identity.command,
            &identity.entry_realpath,
            &identity.generation_token,
        )
}

fn discovery_has_identity(discovery: &LocalExecDaemonDiscovery) -> bool {
    discovery
        .entry_realpath
        .as_ref()
        .is_some_and(|value| !value.is_empty())
        && discovery
            .generation_token
            .as_ref()
            .is_some_and(|value| !value.is_empty())
}

fn expected_from_discovery(
    discovery: &LocalExecDaemonDiscovery,
) -> Option<ExpectedLocalExecProcessIdentity> {
    Some(ExpectedLocalExecProcessIdentity {
        pid: discovery.pid,
        entry_realpath: discovery.entry_realpath.clone()?,
        generation_token: discovery.generation_token.clone()?,
        start_epoch_ms: None,
        command: None,
        discovery_started_at: discovery.started_at.is_finite().then(|| discovery.started_at as u64),
    })
}

fn descriptor_matches_process(
    discovery: &LocalExecDaemonDiscovery,
    identity: &LocalExecProcessIdentity,
    observed_at_ms: u64,
) -> bool {
    discovery_has_identity(discovery)
        && discovery.pid == identity.pid
        && local_exec_discovery_time_matches_process(
            discovery.started_at,
            identity.start_epoch_ms,
            observed_at_ms,
        )
        && discovery.entry_realpath.as_deref() == Some(identity.entry_realpath.as_str())
        && discovery.generation_token.as_deref() == Some(identity.generation_token.as_str())
        && command_carries_local_exec_generation(
            &identity.command,
            &identity.entry_realpath,
            &identity.generation_token,
        )
}

fn identity_from_active(state: &LocalExecDaemonState) -> Option<LocalExecProcessIdentity> {
    let LocalExecDaemonState::Active {
        pid,
        process_start_epoch_ms,
        command,
        entry_realpath,
        generation_token,
        ..
    } = state
    else {
        return None;
    };
    Some(LocalExecProcessIdentity {
        pid: *pid,
        start_epoch_ms: *process_start_epoch_ms,
        command: command.clone(),
        entry_realpath: entry_realpath.clone(),
        generation_token: generation_token.clone(),
    })
}

pub struct LocalExecDaemonRuntime<C: LocalExecControl> {
    data_dir: std::path::PathBuf,
    is_packaged: bool,
    control: C,
    supervisor: LocalExecSupervisor,
    started: bool,
    disposed: bool,
    paused: bool,
    refused_for_client_pause: bool,
    credential_handed_off: bool,
    missing_discovery_ticks: u32,
    consecutive_respawns: u32,
    quarantined_identity: Option<LocalExecProcessIdentity>,
}

impl<C: LocalExecControl> LocalExecDaemonRuntime<C> {
    pub fn new(
        data_dir: impl Into<std::path::PathBuf>,
        is_packaged: bool,
        control: C,
    ) -> Self {
        Self {
            data_dir: data_dir.into(),
            is_packaged,
            control,
            supervisor: LocalExecSupervisor::default(),
            started: false,
            disposed: false,
            paused: false,
            refused_for_client_pause: false,
            credential_handed_off: false,
            missing_discovery_ticks: 0,
            consecutive_respawns: 0,
            quarantined_identity: None,
        }
    }

    pub fn state(&self) -> &LocalExecDaemonState {
        self.supervisor.state()
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn refused_for_client_pause(&self) -> bool {
        self.refused_for_client_pause
    }

    pub fn consecutive_respawns(&self) -> u32 {
        self.consecutive_respawns
    }

    pub fn control(&self) -> &C {
        &self.control
    }

    pub fn control_mut(&mut self) -> &mut C {
        &mut self.control
    }

    pub fn into_control(self) -> C {
        self.control
    }

    pub fn start(&mut self) {
        if self.disposed || self.started {
            return;
        }
        self.started = true;
        self.supervisor.start();
        self.refresh_connection();
        self.refresh_credential();
        if !self.disposed && !self.paused && !self.refused_for_client_pause {
            self.establish_daemon();
        }
    }

    pub fn refresh_tick(&mut self) {
        if self.disposed || self.paused {
            return;
        }
        self.refresh_connection();
        self.refresh_credential();
    }

    pub fn liveness_tick(&mut self) {
        if self.disposed {
            return;
        }
        self.heal_daemon();
    }

    pub fn set_paused(&mut self, next: bool) {
        if self.disposed || self.paused == next {
            return;
        }
        self.paused = next;
        if next {
            self.retire_daemon_for_pause();
            return;
        }
        if !self.started {
            return;
        }
        self.refused_for_client_pause = false;
        self.refresh_connection();
        self.refresh_credential();
        if self.quarantine_blocks_spawn() {
            self.supervisor.mark_failed(
                "local-exec active discovery generation changed while its verified process remained live",
            );
            return;
        }
        self.establish_daemon();
    }

    pub fn observe_spawned_exit(&mut self, identity: &LocalExecProcessIdentity) {
        if self.disposed {
            return;
        }
        let Some(active) = identity_from_active(self.supervisor.state()) else {
            return;
        };
        if !same_local_exec_process_identity(&active, identity) {
            return;
        }
        if self.consecutive_respawns >= LOCAL_EXEC_DAEMON_RESPAWN_LIMIT {
            self.supervisor.mark_failed("respawn limit reached");
            return;
        }
        self.supervisor.stop();
        if !self.paused && !self.refused_for_client_pause {
            self.consecutive_respawns = self.consecutive_respawns.saturating_add(1);
            self.supervisor.start();
            self.establish_daemon();
        }
    }

    pub fn dispose(&mut self) {
        self.disposed = true;
    }

    fn paths(&self) -> super::daemon_files::LocalExecDaemonPaths {
        super::daemon_files::resolve_local_exec_daemon_paths(&self.data_dir)
    }

    fn refresh_connection(&mut self) {
        if self.disposed || self.paused {
            return;
        }
        let paths = self.paths();
        match self.control.resolve_gateway_connection() {
            Ok(connection) => {
                if super::daemon_files::write_local_exec_daemon_connection(
                    &paths.connection_path,
                    &connection,
                )
                .is_ok()
                {
                    let _ = super::daemon_files::write_local_exec_supervisor_heartbeat(
                        &paths.supervisor_heartbeat_path,
                        self.control.now_ms(),
                    );
                    self.refused_for_client_pause = false;
                }
            }
            Err(error) if error.message.contains(SAND_CLIENT_PAUSE_REASON) => {
                self.refused_for_client_pause = true;
                self.retire_daemon_for_pause();
            }
            Err(_) => {}
        }
    }

    fn refresh_credential(&mut self) {
        if self.credential_handed_off || self.disposed || self.paused {
            return;
        }
        let paths = self.paths();
        let Ok(Some(credential)) = self.control.mint_local_exec_daemon_credential() else {
            return;
        };
        if super::daemon_files::write_local_exec_daemon_credential(
            &paths.credential_path,
            &credential,
        )
        .is_ok()
        {
            self.credential_handed_off = true;
        }
    }

    fn current_identity(
        &mut self,
        expected: &ExpectedLocalExecProcessIdentity,
    ) -> Option<LocalExecProcessIdentity> {
        self.control
            .get_process_identity(expected)
            .ok()
            .flatten()
            .filter(identity_is_valid)
    }

    fn terminate_if_still_owned(&mut self, expected: &LocalExecProcessIdentity) -> bool {
        self.control
            .terminate_process(expected)
            .unwrap_or(false)
    }

    fn quarantine_blocks_spawn(&mut self) -> bool {
        let Some(quarantined) = self.quarantined_identity.clone() else {
            return false;
        };
        let expected = ExpectedLocalExecProcessIdentity {
            pid: quarantined.pid,
            entry_realpath: quarantined.entry_realpath.clone(),
            generation_token: quarantined.generation_token.clone(),
            start_epoch_ms: Some(quarantined.start_epoch_ms),
            command: Some(quarantined.command.clone()),
            discovery_started_at: None,
        };
        if self
            .current_identity(&expected)
            .as_ref()
            .is_some_and(|observed| same_local_exec_process_identity(observed, &quarantined))
        {
            return true;
        }
        if self.control.is_process_alive(quarantined.pid).unwrap_or(false) {
            return true;
        }
        self.quarantined_identity = None;
        false
    }

    fn spawn_daemon(&mut self) -> Result<(), LocalExecSupervisorError> {
        if self.paused || self.refused_for_client_pause || self.disposed {
            return Ok(());
        }
        let paths = self.paths();
        let mut env = std::collections::BTreeMap::new();
        env.insert("ELECTRON_RUN_AS_NODE".into(), "1".into());
        env.insert(
            "SAND_PACKAGED".into(),
            if self.is_packaged { "1" } else { "0" }.into(),
        );
        let spawned = self.control.spawn_local_exec_daemon(SpawnLocalExecDaemonRequest {
            env,
            log_path: paths.log_path.clone(),
        })?;
        if !identity_is_valid(&spawned) {
            return Err(LocalExecSupervisorError::new(
                "local-exec spawn did not return a valid daemon identity",
            ));
        }

        let deadline = self
            .control
            .now_ms()
            .saturating_add(LOCAL_EXEC_DAEMON_READINESS_TIMEOUT_MS);
        loop {
            if self.disposed || self.paused || self.refused_for_client_pause {
                self.cleanup_failed_spawn(&spawned);
                return Ok(());
            }
            let discovery = super::daemon_files::read_local_exec_daemon_discovery(
                &paths.discovery_path,
            )?;
            let expected = ExpectedLocalExecProcessIdentity {
                pid: spawned.pid,
                entry_realpath: spawned.entry_realpath.clone(),
                generation_token: spawned.generation_token.clone(),
                start_epoch_ms: Some(spawned.start_epoch_ms),
                command: Some(spawned.command.clone()),
                discovery_started_at: discovery.as_ref().map(|value| value.started_at as u64),
            };
            let observed = self.current_identity(&expected).ok_or_else(|| {
                LocalExecSupervisorError::new(format!(
                    "local-exec daemon {} exited or changed identity before readiness",
                    spawned.pid
                ))
            })?;
            if !same_local_exec_process_identity(&observed, &spawned) {
                self.cleanup_failed_spawn(&spawned);
                return Err(LocalExecSupervisorError::new(format!(
                    "local-exec daemon {} changed identity before readiness",
                    spawned.pid
                )));
            }
            if let Some(discovery) = discovery {
                if descriptor_matches_process(&discovery, &spawned, self.control.now_ms()) {
                    self.quarantined_identity = None;
                    self.supervisor
                        .mark_active(
                            LocalExecDaemonOrigin::Spawned,
                            spawned.pid,
                            spawned.start_epoch_ms,
                            spawned.command.clone(),
                            spawned.entry_realpath.clone(),
                            spawned.generation_token.clone(),
                        )
                        .map_err(LocalExecSupervisorError::new)?;
                    self.missing_discovery_ticks = 0;
                    return Ok(());
                }
            }
            if self.control.now_ms() >= deadline {
                self.cleanup_failed_spawn(&spawned);
                return Err(LocalExecSupervisorError::new(format!(
                    "local-exec daemon {} did not publish matching discovery before readiness timeout",
                    spawned.pid
                )));
            }
            self.control.sleep_ms(LOCAL_EXEC_DAEMON_READINESS_POLL_MS);
        }
    }

    fn cleanup_failed_spawn(&mut self, spawned: &LocalExecProcessIdentity) {
        let _ = self.terminate_if_still_owned(spawned);
        let paths = self.paths();
        if let Ok(Some(discovery)) =
            super::daemon_files::read_local_exec_daemon_discovery(&paths.discovery_path)
        {
            if discovery.pid == spawned.pid
                && discovery.entry_realpath.as_deref() == Some(spawned.entry_realpath.as_str())
                && discovery.generation_token.as_deref()
                    == Some(spawned.generation_token.as_str())
            {
                let _ = super::daemon_files::remove_local_exec_daemon_discovery_if_matches(
                    &paths.discovery_path,
                    &discovery,
                );
            }
        }
    }

    fn establish_daemon(&mut self) {
        if self.paused || self.refused_for_client_pause || self.disposed {
            return;
        }
        let result = self.establish_daemon_inner();
        if let Err(error) = result {
            self.supervisor.mark_failed(error.message);
        }
    }

    fn establish_daemon_inner(&mut self) -> Result<(), LocalExecSupervisorError> {
        let paths = self.paths();
        let existing =
            super::daemon_files::read_local_exec_daemon_discovery(&paths.discovery_path)?;
        let Some(existing) = existing else {
            return self.spawn_daemon();
        };
        let Some(expected) = expected_from_discovery(&existing) else {
            let _ = super::daemon_files::remove_local_exec_daemon_discovery_if_matches(
                &paths.discovery_path,
                &existing,
            );
            return self.spawn_daemon();
        };
        let identity = self.current_identity(&expected);
        let valid = identity
            .as_ref()
            .is_some_and(|identity| {
                descriptor_matches_process(&existing, identity, self.control.now_ms())
            });
        if !valid {
            let _ = super::daemon_files::remove_local_exec_daemon_discovery_if_matches(
                &paths.discovery_path,
                &existing,
            );
            return self.spawn_daemon();
        }
        let identity = identity.expect("validated identity");
        if existing.inflight_count.unwrap_or(0) > 0 {
            self.supervisor.reconcile_discovery(Some(&existing));
            self.supervisor
                .mark_active(
                    LocalExecDaemonOrigin::Adopted,
                    identity.pid,
                    identity.start_epoch_ms,
                    identity.command,
                    identity.entry_realpath,
                    identity.generation_token,
                )
                .map_err(LocalExecSupervisorError::new)?;
            self.missing_discovery_ticks = 0;
            self.quarantined_identity = None;
            return Ok(());
        }

        self.supervisor.reconcile_discovery(Some(&existing));
        let _ = self.terminate_if_still_owned(&identity);
        let _ = super::daemon_files::remove_local_exec_daemon_discovery_if_matches(
            &paths.discovery_path,
            &existing,
        );
        self.spawn_daemon()
    }

    fn retire_daemon_for_pause(&mut self) {
        let paths = self.paths();
        if self.quarantine_blocks_spawn() {
            if let Ok(Some(existing)) =
                super::daemon_files::read_local_exec_daemon_discovery(&paths.discovery_path)
            {
                let _ = super::daemon_files::remove_local_exec_daemon_discovery_if_matches(
                    &paths.discovery_path,
                    &existing,
                );
            }
            let _ = super::daemon_files::remove_local_exec_daemon_file(&paths.connection_path);
            self.supervisor.mark_failed(
                "local-exec active discovery generation changed while its verified process remained live",
            );
            self.missing_discovery_ticks = 0;
            return;
        }

        if let Some(active) = identity_from_active(self.supervisor.state()) {
            let _ = self.terminate_if_still_owned(&active);
        }
        if let Ok(Some(existing)) =
            super::daemon_files::read_local_exec_daemon_discovery(&paths.discovery_path)
        {
            if let Some(expected) = expected_from_discovery(&existing) {
                if let Some(identity) = self.current_identity(&expected) {
                    if descriptor_matches_process(&existing, &identity, self.control.now_ms()) {
                        let _ = self.terminate_if_still_owned(&identity);
                    }
                }
            }
            let _ = super::daemon_files::remove_local_exec_daemon_discovery_if_matches(
                &paths.discovery_path,
                &existing,
            );
        }
        let _ = super::daemon_files::remove_local_exec_daemon_file(&paths.connection_path);
        self.quarantined_identity = None;
        self.supervisor.stop();
        self.missing_discovery_ticks = 0;
    }

    fn heal_daemon(&mut self) {
        if self.paused || self.refused_for_client_pause {
            self.retire_daemon_for_pause();
            return;
        }
        if matches!(self.supervisor.state(), LocalExecDaemonState::Absent) {
            self.consecutive_respawns = 0;
            self.establish_daemon();
            return;
        }
        if matches!(self.supervisor.state(), LocalExecDaemonState::Failed { .. }) {
            if self.quarantine_blocks_spawn()
                || self.consecutive_respawns >= LOCAL_EXEC_DAEMON_RESPAWN_LIMIT
            {
                return;
            }
            self.consecutive_respawns = self.consecutive_respawns.saturating_add(1);
            self.establish_daemon();
            return;
        }
        let Some(active_identity) = identity_from_active(self.supervisor.state()) else {
            return;
        };

        let paths = self.paths();
        let discovery = match super::daemon_files::read_local_exec_daemon_discovery(
            &paths.discovery_path,
        ) {
            Ok(value) => value,
            Err(error) => {
                self.supervisor.mark_failed(format!(
                    "local-exec discovery unreadable: {error}"
                ));
                return;
            }
        };

        let mut needs_respawn = false;
        match discovery {
            None => {
                self.missing_discovery_ticks = self.missing_discovery_ticks.saturating_add(1);
                if self.missing_discovery_ticks < LOCAL_EXEC_DAEMON_DISCOVERY_MISS_LIMIT {
                    return;
                }
                let _ = self.terminate_if_still_owned(&active_identity);
                self.supervisor.stop();
                needs_respawn = true;
            }
            Some(discovery)
                if discovery.pid == active_identity.pid
                    && discovery.entry_realpath.as_deref()
                        == Some(active_identity.entry_realpath.as_str())
                    && discovery.generation_token.as_deref()
                        == Some(active_identity.generation_token.as_str()) =>
            {
                let expected = ExpectedLocalExecProcessIdentity {
                    pid: active_identity.pid,
                    entry_realpath: active_identity.entry_realpath.clone(),
                    generation_token: active_identity.generation_token.clone(),
                    start_epoch_ms: Some(active_identity.start_epoch_ms),
                    command: Some(active_identity.command.clone()),
                    discovery_started_at: Some(discovery.started_at as u64),
                };
                let healthy = self
                    .current_identity(&expected)
                    .as_ref()
                    .is_some_and(|observed| {
                        same_local_exec_process_identity(observed, &active_identity)
                    });
                if healthy {
                    self.missing_discovery_ticks = 0;
                    self.consecutive_respawns = 0;
                    return;
                }
                let _ = super::daemon_files::remove_local_exec_daemon_discovery_if_matches(
                    &paths.discovery_path,
                    &discovery,
                );
                self.supervisor.stop();
                needs_respawn = true;
            }
            Some(discovery) if discovery.pid == active_identity.pid => {
                let _ = super::daemon_files::remove_local_exec_daemon_discovery_if_matches(
                    &paths.discovery_path,
                    &discovery,
                );
                let expected = ExpectedLocalExecProcessIdentity {
                    pid: active_identity.pid,
                    entry_realpath: active_identity.entry_realpath.clone(),
                    generation_token: active_identity.generation_token.clone(),
                    start_epoch_ms: Some(active_identity.start_epoch_ms),
                    command: Some(active_identity.command.clone()),
                    discovery_started_at: None,
                };
                if self
                    .current_identity(&expected)
                    .as_ref()
                    .is_some_and(|observed| {
                        same_local_exec_process_identity(observed, &active_identity)
                    })
                {
                    self.quarantined_identity = Some(active_identity);
                    self.missing_discovery_ticks = 0;
                    self.supervisor.mark_failed(
                        "local-exec active discovery generation changed while its verified process remained live",
                    );
                    return;
                }
                self.supervisor.stop();
                needs_respawn = true;
            }
            Some(discovery) => {
                let successor = expected_from_discovery(&discovery)
                    .and_then(|expected| self.current_identity(&expected));
                if let Some(successor) = successor.filter(|identity| {
                    descriptor_matches_process(&discovery, identity, self.control.now_ms())
                }) {
                    let _ = self.terminate_if_still_owned(&active_identity);
                    self.supervisor
                        .mark_active(
                            LocalExecDaemonOrigin::Adopted,
                            successor.pid,
                            successor.start_epoch_ms,
                            successor.command,
                            successor.entry_realpath,
                            successor.generation_token,
                        )
                        .ok();
                    self.missing_discovery_ticks = 0;
                    self.consecutive_respawns = 0;
                    self.quarantined_identity = None;
                    return;
                }
                let _ = super::daemon_files::remove_local_exec_daemon_discovery_if_matches(
                    &paths.discovery_path,
                    &discovery,
                );
                let _ = self.terminate_if_still_owned(&active_identity);
                self.supervisor.stop();
                needs_respawn = true;
            }
        }

        if needs_respawn {
            if self.consecutive_respawns >= LOCAL_EXEC_DAEMON_RESPAWN_LIMIT {
                self.supervisor.mark_failed("respawn limit reached");
                return;
            }
            self.consecutive_respawns = self.consecutive_respawns.saturating_add(1);
            self.supervisor.start();
            self.establish_daemon();
        }
    }
}
