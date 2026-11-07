use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::host_bundle_upgrade::{
    HOST_BUNDLE_WATCH_INTERVAL_MS, HOST_BUNDLE_WATCH_JITTER_RATIO,
    host_bundle_watch_initial_delay_ms, host_bundle_watch_next_delay_ms,
};
use super::host_upgrade_service::{HostUpgradeDependencies, HostUpgradeService};

pub const HOST_UPGRADE_EXTENSION_ID: &str = "host-upgrade";
pub const HOST_UPGRADE_EXTENSION_DEPENDENCIES: &[&str] = &[
    "automations",
    "cross-user-sharing",
    "telemetry",
    "transcript",
];
pub const HOST_UPGRADE_MARKER_FORWARD_INTERVAL_MS: u64 = 5 * 60_000;

pub type SharedUpdateRootTick = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;
pub type HostUpgradeRandom = Arc<dyn Fn() -> f64 + Send + Sync>;

pub fn is_host_bundle_auto_update_enabled(raw: Option<&str>) -> bool {
    !matches!(
        raw.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("0" | "false" | "no")
    )
}

pub fn host_bundle_watch_interval_ms(raw: Option<&str>) -> u64 {
    raw.and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(HOST_BUNDLE_WATCH_INTERVAL_MS)
}

pub fn host_bundle_watch_jitter_ratio(raw: Option<&str>) -> f64 {
    raw.and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(HOST_BUNDLE_WATCH_JITTER_RATIO)
}

fn system_random_unit() -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    f64::from(nanos) / f64::from(u32::MAX)
}

#[derive(Debug, Clone)]
pub struct HostUpgradeMarkerStore {
    path: PathBuf,
}

impl HostUpgradeMarkerStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read_raw(&self) -> Option<String> {
        fs::read_to_string(&self.path).ok()
    }

    pub fn delete_marker(&self) {
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {}
        }
    }
}

struct HostUpgradeWorkers {
    marker_stop: mpsc::Sender<()>,
    marker_worker: Option<JoinHandle<()>>,
    watch_stop: mpsc::Sender<()>,
    watch_worker: Option<JoinHandle<()>>,
}

impl HostUpgradeWorkers {
    fn stop(mut self) {
        let _ = self.marker_stop.send(());
        let _ = self.watch_stop.send(());
        if let Some(worker) = self.marker_worker.take() {
            let _ = worker.join();
        }
        if let Some(worker) = self.watch_worker.take() {
            let _ = worker.join();
        }
    }
}

pub struct HostUpgradeExtension<D: HostUpgradeDependencies + 'static> {
    service: Arc<HostUpgradeService<D>>,
    marker_store: HostUpgradeMarkerStore,
    update_watch_interval_ms: u64,
    update_watch_jitter_ratio: f64,
    workers: Mutex<Option<HostUpgradeWorkers>>,
}

impl<D: HostUpgradeDependencies + 'static> HostUpgradeExtension<D> {
    pub fn new(
        deps: Arc<D>,
        marker_path: impl Into<PathBuf>,
        auto_update_enabled: bool,
        update_watch_interval_ms: u64,
        update_watch_jitter_ratio: f64,
    ) -> Self {
        Self {
            service: Arc::new(HostUpgradeService::new(deps, auto_update_enabled)),
            marker_store: HostUpgradeMarkerStore::new(marker_path),
            update_watch_interval_ms,
            update_watch_jitter_ratio,
            workers: Mutex::new(None),
        }
    }

    pub fn service(&self) -> Arc<HostUpgradeService<D>> {
        Arc::clone(&self.service)
    }

    pub fn marker_store(&self) -> &HostUpgradeMarkerStore {
        &self.marker_store
    }

    pub fn update_watch_interval_ms(&self) -> u64 {
        self.update_watch_interval_ms
    }

    pub fn update_watch_jitter_ratio(&self) -> f64 {
        self.update_watch_jitter_ratio
    }

    pub fn start_background_work(
        &self,
        run_root_update_tick: SharedUpdateRootTick,
    ) -> Result<(), String> {
        self.start_background_work_with_random(
            run_root_update_tick,
            Arc::new(system_random_unit),
        )
    }

    pub fn start_background_work_with_random(
        &self,
        run_root_update_tick: SharedUpdateRootTick,
        random: HostUpgradeRandom,
    ) -> Result<(), String> {
        let mut slot = self
            .workers
            .lock()
            .map_err(|_| "host upgrade worker mutex poisoned".to_string())?;
        if slot.is_some() {
            return Ok(());
        }

        self.service.activate_after_host_identity_ready()?;

        let marker_service = Arc::clone(&self.service);
        let (marker_stop, marker_rx) = mpsc::channel();
        let marker_worker = thread::Builder::new()
            .name("sand-host-upgrade-marker".into())
            .spawn(move || loop {
                match marker_rx.recv_timeout(Duration::from_millis(
                    HOST_UPGRADE_MARKER_FORWARD_INTERVAL_MS,
                )) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if let Err(error) = marker_service.forward_host_upgrade_marker() {
                            eprintln!(
                                "[sand-host-upgrade] marker forward wake failed: {error}"
                            );
                        }
                    }
                }
            })
            .map_err(|error| format!("could not start host-upgrade marker worker: {error}"))?;

        let watch_service = Arc::clone(&self.service);
        let watch_random = Arc::clone(&random);
        let watch_root_tick = Arc::clone(&run_root_update_tick);
        let base_ms = self.update_watch_interval_ms.max(1);
        let jitter_ratio = self.update_watch_jitter_ratio;
        let initial_delay_ms = host_bundle_watch_initial_delay_ms(
            base_ms,
            jitter_ratio,
            || random(),
        );
        let (watch_stop, watch_rx) = mpsc::channel();
        let watch_worker = match thread::Builder::new()
            .name("sand-host-upgrade-watch".into())
            .spawn(move || {
                let mut delay_ms = initial_delay_ms.max(1);
                loop {
                    match watch_rx.recv_timeout(Duration::from_millis(delay_ms)) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if let Err(error) = watch_service.run_shared_update_watch_tick(|| {
                                (watch_root_tick)()
                            }) {
                                eprintln!(
                                    "[sand-host-upgrade] shared update watch tick failed: {error}"
                                );
                            }
                            delay_ms = host_bundle_watch_next_delay_ms(
                                base_ms,
                                jitter_ratio,
                                || watch_random(),
                            )
                            .max(1);
                        }
                    }
                }
            }) {
            Ok(worker) => worker,
            Err(error) => {
                let _ = marker_stop.send(());
                let _ = marker_worker.join();
                return Err(format!(
                    "could not start host-upgrade watch worker: {error}"
                ));
            }
        };

        *slot = Some(HostUpgradeWorkers {
            marker_stop,
            marker_worker: Some(marker_worker),
            watch_stop,
            watch_worker: Some(watch_worker),
        });
        Ok(())
    }

    pub fn stop_background_work(&self) {
        let workers = self
            .workers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(workers) = workers {
            workers.stop();
        }
    }

    pub fn is_background_work_running(&self) -> bool {
        self.workers
            .lock()
            .map(|workers| workers.is_some())
            .unwrap_or(false)
    }
}

impl<D: HostUpgradeDependencies + 'static> Drop for HostUpgradeExtension<D> {
    fn drop(&mut self) {
        self.stop_background_work();
    }
}
