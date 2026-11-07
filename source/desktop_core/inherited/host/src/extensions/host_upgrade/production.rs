use std::collections::{BTreeMap, HashMap};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::extensions::telemetry::host_telemetry_service::HostStructuredLogTelemetry;
use crate::host_paths::get_host_upgrade_marker_path;

use super::extension::{
    HostUpgradeExtension, HostUpgradeMarkerStore, SharedUpdateRootTick,
    host_bundle_watch_interval_ms, host_bundle_watch_jitter_ratio,
    is_host_bundle_auto_update_enabled,
};
use super::host_bundle_source::{
    HostBundleFetcher, ReqwestHostBundleFetcher, fetch_host_bundle_tarball_with_base,
    fetch_latest_host_bundle_version, host_bundle_base_url,
};
use super::host_bundle_upgrade::{
    SAND_BOX_HOST_VERSION_PATH, SAND_SUPERVISOR_ACKS_DIR, StageHostBundleUpgradeArgs,
    is_host_version_swap_vetoed, read_local_host_version, stage_host_bundle_upgrade,
};
use super::host_upgrade_service::HostUpgradeDependencies;

pub type FallibleUpgradeHook = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;
pub type UpgradeHook = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub struct ProductionHostUpgradePeers {
    pub suspend_automation_wakes: FallibleUpgradeHook,
    pub prepare_sharing_for_upgrade: UpgradeHook,
    pub quiesce_transcript_for_upgrade: FallibleUpgradeHook,
    pub resume_interrupted_upgrade_turns: FallibleUpgradeHook,
}

pub struct ProductionHostUpgradeDependencies {
    peers: ProductionHostUpgradePeers,
    logs: HostStructuredLogTelemetry,
    fetcher: Arc<dyn HostBundleFetcher>,
    marker_store: HostUpgradeMarkerStore,
}

impl ProductionHostUpgradeDependencies {
    pub fn new(
        peers: ProductionHostUpgradePeers,
        logs: HostStructuredLogTelemetry,
        fetcher: Arc<dyn HostBundleFetcher>,
        marker_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            peers,
            logs,
            fetcher,
            marker_store: HostUpgradeMarkerStore::new(marker_path),
        }
    }

}

impl HostUpgradeDependencies for ProductionHostUpgradeDependencies {
    fn suspend_automation_wakes(&self) -> Result<(), String> {
        (self.peers.suspend_automation_wakes)()
    }

    fn prepare_sharing_for_upgrade(&self) {
        (self.peers.prepare_sharing_for_upgrade)();
    }

    fn quiesce_transcript_for_upgrade(&self) -> Result<(), String> {
        (self.peers.quiesce_transcript_for_upgrade)()
    }

    fn resume_interrupted_upgrade_turns(&self) -> Result<(), String> {
        (self.peers.resume_interrupted_upgrade_turns)()
    }

    fn resolve_bundle_version(&self) -> Result<Option<String>, String> {
        Ok(fetch_latest_host_bundle_version(self.fetcher.as_ref()))
    }

    fn load_bundle_bytes(&self, version: &str) -> Result<Vec<u8>, String> {
        fetch_host_bundle_tarball_with_base(
            self.fetcher.as_ref(),
            version,
            &host_bundle_base_url(),
        )
        .map_err(|error| error.to_string())
    }

    fn read_local_version(&self) -> Option<String> {
        read_local_host_version(SAND_BOX_HOST_VERSION_PATH)
    }

    fn is_version_swap_vetoed(&self, version: &str, local: Option<&str>) -> bool {
        is_host_version_swap_vetoed(version, local, SAND_SUPERVISOR_ACKS_DIR)
    }

    fn stage_upgrade(
        &self,
        version: &str,
        bytes: &[u8],
        force_now: bool,
        reason: &str,
    ) -> Result<(), String> {
        let mut args = StageHostBundleUpgradeArgs::production(version, bytes, self.now_ms());
        args.force_now = force_now;
        args.reason = Some(reason);
        stage_host_bundle_upgrade(&args)
            .map(|_| ())
            .map_err(|error| format!("could not stage Host bundle {version}: {error}"))
    }

    fn read_marker_raw(&self) -> Result<Option<String>, String> {
        Ok(self.marker_store.read_raw())
    }

    fn delete_marker(&self) -> Result<(), String> {
        self.marker_store.delete_marker();
        Ok(())
    }

    fn report_host_upgrade(&self, metadata: HashMap<String, String>) {
        let _ = self.logs.report_host_upgrade(metadata.into_iter().collect());
    }

    fn report_host_upgrade_confirmed(
        &self,
        metadata: &BTreeMap<String, String>,
    ) -> Result<bool, String> {
        Ok(self.logs.report_host_upgrade_confirmed(metadata.clone()))
    }

    fn log(&self, level: &str, message: &str) {
        match level {
            "warn" => eprintln!("[sand-host-upgrade] {message}"),
            _ => eprintln!("[sand-host-upgrade] {message}"),
        }
    }

    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64
    }
}

pub type ProductionHostUpgradeExtension =
    HostUpgradeExtension<ProductionHostUpgradeDependencies>;

pub fn start_production_host_upgrade_extension(
    peers: ProductionHostUpgradePeers,
    logs: HostStructuredLogTelemetry,
    run_root_update_tick: SharedUpdateRootTick,
) -> Result<Arc<ProductionHostUpgradeExtension>, String> {
    let fetcher = Arc::new(ReqwestHostBundleFetcher::new()?);
    start_production_host_upgrade_extension_with(
        peers,
        logs,
        fetcher,
        get_host_upgrade_marker_path(),
        run_root_update_tick,
    )
}

pub fn start_production_host_upgrade_extension_with(
    peers: ProductionHostUpgradePeers,
    logs: HostStructuredLogTelemetry,
    fetcher: Arc<dyn HostBundleFetcher>,
    marker_path: impl AsRef<Path>,
    run_root_update_tick: SharedUpdateRootTick,
) -> Result<Arc<ProductionHostUpgradeExtension>, String> {
    let deps = Arc::new(ProductionHostUpgradeDependencies::new(
        peers,
        logs,
        fetcher,
        marker_path.as_ref().to_path_buf(),
    ));
    let extension = Arc::new(HostUpgradeExtension::new(
        deps,
        marker_path.as_ref().to_path_buf(),
        is_host_bundle_auto_update_enabled(env::var("SAND_BOX_AUTO_UPDATE").ok().as_deref()),
        host_bundle_watch_interval_ms(env::var("SAND_BOX_UPDATE_WATCH_INTERVAL_MS").ok().as_deref()),
        host_bundle_watch_jitter_ratio(env::var("SAND_BOX_UPDATE_WATCH_JITTER_RATIO").ok().as_deref()),
    ));
    extension.start_background_work(run_root_update_tick)?;
    Ok(extension)
}
