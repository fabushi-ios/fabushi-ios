use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use super::host_bundle_upgrade::{
    HostBundleStageFailure, HostBundleUpdateState, MAX_FAILED_SWAP_RESTAGE_RETRIES,
    fetch_stage_and_report_host_bundle, is_sand_host_upgrade_available,
    note_failed_swap_marker_for_retry,
};
use super::host_upgrade_marker::{
    HostUpgradeMarker, HostUpgradeMarkerForwardDeps, HostUpgradeMarkerForwardOutcome,
    forward_host_upgrade_marker_with,
};

pub trait HostUpgradeDependencies: Send + Sync {
    fn suspend_automation_wakes(&self) -> Result<(), String>;
    fn prepare_sharing_for_upgrade(&self);
    fn quiesce_transcript_for_upgrade(&self) -> Result<(), String>;
    fn resume_interrupted_upgrade_turns(&self) -> Result<(), String>;

    fn resolve_bundle_version(&self) -> Result<Option<String>, String>;
    fn load_bundle_bytes(&self, version: &str) -> Result<Vec<u8>, String>;
    fn read_local_version(&self) -> Option<String>;
    fn is_version_swap_vetoed(&self, version: &str, local_version: Option<&str>) -> bool;
    fn stage_upgrade(
        &self,
        version: &str,
        bytes: &[u8],
        force_now: bool,
        reason: &str,
    ) -> Result<(), String>;

    fn read_marker_raw(&self) -> Result<Option<String>, String>;
    fn delete_marker(&self) -> Result<(), String>;
    fn report_host_upgrade(&self, metadata: HashMap<String, String>);
    fn report_host_upgrade_confirmed(
        &self,
        metadata: &BTreeMap<String, String>,
    ) -> Result<bool, String>;
    fn log(&self, level: &str, message: &str);
    fn now_ms(&self) -> i64;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostUpdateNowResult {
    Started {
        version: String,
    },
    NotStarted {
        reason: String,
        version: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostVersionState {
    pub host_version: Option<String>,
    pub latest_host_version: Option<String>,
    pub host_update_available: Option<bool>,
}

#[derive(Debug, Default)]
struct ServiceState {
    update_in_flight: bool,
    local_version: Option<String>,
    latest_version: Option<String>,
    host_identity_ready: bool,
    last_handled_marker: Option<String>,
    marker_now: Option<(String, i64)>,
}

pub struct HostUpgradeService<D: HostUpgradeDependencies> {
    deps: Arc<D>,
    update_state: Mutex<HostBundleUpdateState>,
    state: Mutex<ServiceState>,
    auto_update_enabled: bool,
}

impl<D: HostUpgradeDependencies> HostUpgradeService<D> {
    pub fn new(deps: Arc<D>, auto_update_enabled: bool) -> Self {
        Self {
            deps,
            update_state: Mutex::new(HostBundleUpdateState::default()),
            state: Mutex::new(ServiceState::default()),
            auto_update_enabled,
        }
    }

    pub fn is_auto_update_enabled(&self) -> bool {
        self.auto_update_enabled
    }

    pub fn resolve_host_bundle_identity_version(&self, fallback: Option<&str>) -> String {
        let disk = self.deps.read_local_version();
        let mut state = self.state.lock().expect("host upgrade state poisoned");
        if disk.is_some() {
            state.local_version = disk.clone();
        }
        disk.or_else(|| state.local_version.clone())
            .or_else(|| fallback.map(str::to_string))
            .unwrap_or_else(|| "unknown".into())
    }

    pub fn activate_after_host_identity_ready(&self) -> Result<(), String> {
        let should_forward = {
            let mut state = self.state.lock().expect("host upgrade state poisoned");
            if state.host_identity_ready {
                false
            } else {
                state.host_identity_ready = true;
                true
            }
        };
        if should_forward {
            let _ = self.forward_host_upgrade_marker()?;
        }
        Ok(())
    }

    pub fn update_host_now(&self, force: bool, include_error_detail: bool) -> HostUpdateNowResult {
        let version = match self.deps.resolve_bundle_version() {
            Ok(Some(version)) => version,
            Ok(None) => {
                return HostUpdateNowResult::NotStarted {
                    reason: "no-bundle-source".into(),
                    version: None,
                };
            }
            Err(error) => {
                return HostUpdateNowResult::NotStarted {
                    reason: error,
                    version: None,
                };
            }
        };
        {
            self.state
                .lock()
                .expect("host upgrade state poisoned")
                .latest_version = Some(version.clone());
        }
        let local = self.deps.read_local_version();
        {
            self.state
                .lock()
                .expect("host upgrade state poisoned")
                .local_version = local.clone();
        }
        if !is_sand_host_upgrade_available(local.as_deref(), Some(&version)) {
            return HostUpdateNowResult::NotStarted {
                reason: "already-latest".into(),
                version: Some(version),
            };
        }
        if self.deps.is_version_swap_vetoed(&version, local.as_deref()) {
            return HostUpdateNowResult::NotStarted {
                reason: "host-version-rolled-back".into(),
                version: Some(version),
            };
        }
        let trigger = if force {
            "manual-update-host"
        } else {
            "manual-update-host-idle"
        };
        match self.run_host_bundle_fetch_stage(&version, local.as_deref(), trigger, force) {
            Ok(()) => HostUpdateNowResult::Started { version },
            Err(error) => HostUpdateNowResult::NotStarted {
                reason: if include_error_detail {
                    format!(
                        "@phase:{} {}",
                        match error.phase {
                            super::host_bundle_upgrade::HostBundleStagePhase::Fetch => "fetch",
                            super::host_bundle_upgrade::HostBundleStagePhase::Stage => "stage",
                        },
                        error.error
                    )
                } else {
                    error.error
                },
                version: None,
            },
        }
    }

    pub fn get_version_state(&self) -> HostVersionState {
        let state = self.state.lock().expect("host upgrade state poisoned");
        HostVersionState {
            host_version: state.local_version.clone(),
            latest_host_version: state.latest_version.clone(),
            host_update_available: state.latest_version.as_ref().map(|latest| {
                is_sand_host_upgrade_available(state.local_version.as_deref(), Some(latest))
            }),
        }
    }

    pub fn prepare_for_upgrade(&self) -> Result<(), String> {
        self.deps.suspend_automation_wakes()?;
        self.deps.prepare_sharing_for_upgrade();
        self.deps.quiesce_transcript_for_upgrade()
    }

    pub fn resume_interrupted_upgrade_turns(&self) -> Result<(), String> {
        self.deps.resume_interrupted_upgrade_turns()
    }

    pub fn forward_host_upgrade_marker(&self) -> Result<HostUpgradeMarkerForwardOutcome, String> {
        forward_host_upgrade_marker_with(&MarkerAdapter { service: self })
    }

    pub fn run_shared_update_watch_tick(
        &self,
        run_root_update_tick: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        if let Err(error) = run_root_update_tick() {
            self.deps.log(
                "warn",
                &format!("shared update watch root tick failed: {error}"),
            );
        }
        self.maybe_auto_update_host_bundle()
    }

    pub fn update_state(&self) -> HostBundleUpdateState {
        self.update_state
            .lock()
            .expect("host bundle update state poisoned")
            .clone()
    }

    fn maybe_auto_update_host_bundle(&self) -> Result<(), String> {
        if !self.auto_update_enabled {
            return Ok(());
        }
        if self
            .state
            .lock()
            .expect("host upgrade state poisoned")
            .update_in_flight
        {
            return Ok(());
        }
        let Some(version) = self.deps.resolve_bundle_version()? else {
            return Ok(());
        };
        {
            self.state
                .lock()
                .expect("host upgrade state poisoned")
                .latest_version = Some(version.clone());
        }
        let local = {
            let mut state = self.state.lock().expect("host upgrade state poisoned");
            if state.local_version.is_none() {
                state.local_version = self.deps.read_local_version();
            }
            state.local_version.clone()
        };
        let already_staged = self
            .update_state
            .lock()
            .expect("host bundle update state poisoned")
            .staged_version
            .as_deref()
            == Some(version.as_str());
        if !is_sand_host_upgrade_available(local.as_deref(), Some(&version))
            || already_staged
            || self.deps.is_version_swap_vetoed(&version, local.as_deref())
        {
            return Ok(());
        }
        self.run_host_bundle_fetch_stage(&version, local.as_deref(), "idle-auto-update", false)
            .map_err(|error| error.error)
    }

    fn run_host_bundle_fetch_stage(
        &self,
        version: &str,
        local_version: Option<&str>,
        trigger: &str,
        force_now: bool,
    ) -> Result<(), HostBundleStageFailure> {
        {
            let mut state = self.state.lock().expect("host upgrade state poisoned");
            if state.update_in_flight {
                return Err(HostBundleStageFailure {
                    phase: super::host_bundle_upgrade::HostBundleStagePhase::Fetch,
                    error: "host bundle update already in flight".into(),
                });
            }
            state.update_in_flight = true;
        }

        let result = {
            let mut update_state = self
                .update_state
                .lock()
                .expect("host bundle update state poisoned");
            fetch_stage_and_report_host_bundle(
                version,
                local_version,
                trigger,
                &mut update_state,
                || self.deps.load_bundle_bytes(version),
                |bytes| self.deps.stage_upgrade(version, bytes, force_now, trigger),
                |metadata| self.deps.report_host_upgrade(metadata),
                |level, message| self.deps.log(level, message),
            )
        };
        self.state
            .lock()
            .expect("host upgrade state poisoned")
            .update_in_flight = false;
        result
    }
}

struct MarkerAdapter<'a, D: HostUpgradeDependencies> {
    service: &'a HostUpgradeService<D>,
}

impl<D: HostUpgradeDependencies> HostUpgradeMarkerForwardDeps for MarkerAdapter<'_, D> {
    fn read_raw(&self) -> Result<Option<String>, String> {
        self.service.deps.read_marker_raw()
    }

    fn delete_marker(&self) -> Result<(), String> {
        self.service.deps.delete_marker()
    }

    fn emit(&self, metadata: &BTreeMap<String, String>) -> Result<bool, String> {
        self.service.deps.report_host_upgrade_confirmed(metadata)
    }

    fn warn(&self, message: &str) {
        self.service.deps.log("warn", message)
    }

    fn now(&self, raw: &str) -> f64 {
        let mut state = self
            .service
            .state
            .lock()
            .expect("host upgrade state poisoned");
        if state
            .marker_now
            .as_ref()
            .is_none_or(|(previous, _)| previous != raw)
        {
            state.marker_now = Some((raw.to_string(), self.service.deps.now_ms()));
        }
        state.marker_now.as_ref().expect("marker now set").1 as f64
    }

    fn was_forwarded(&self, raw: &str) -> bool {
        self.service
            .state
            .lock()
            .expect("host upgrade state poisoned")
            .last_handled_marker
            .as_deref()
            == Some(raw)
    }

    fn mark_forwarded(&self, raw: &str) {
        self.service
            .state
            .lock()
            .expect("host upgrade state poisoned")
            .last_handled_marker = Some(raw.to_string());
    }

    fn on_forwarded(&self, marker: &HostUpgradeMarker) {
        let mut update_state = self
            .service
            .update_state
            .lock()
            .expect("host bundle update state poisoned");
        if note_failed_swap_marker_for_retry(
            &mut update_state,
            marker,
            MAX_FAILED_SWAP_RESTAGE_RETRIES,
        ) {
            self.service.deps.log(
                "info",
                &format!(
                    "supervisor swap of {} failed ({}); re-staging on the next idle watch tick",
                    marker.to_version.as_deref().unwrap_or("?"),
                    marker.swap_error.as_deref().unwrap_or("swap-failed")
                ),
            );
        }
    }
}
