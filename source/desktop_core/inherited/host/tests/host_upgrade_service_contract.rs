use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::host_upgrade::host_upgrade_service::{
    HostUpdateNowResult, HostUpgradeDependencies, HostUpgradeService,
};

struct Deps {
    local: Mutex<Option<String>>,
    latest: Mutex<Option<String>>,
    bundle: Mutex<Result<Vec<u8>, String>>,
    marker: Mutex<Option<String>>,
    events: Mutex<Vec<HashMap<String, String>>>,
    confirmed: Mutex<Vec<BTreeMap<String, String>>>,
    calls: Mutex<Vec<String>>,
}

impl Default for Deps {
    fn default() -> Self {
        Self {
            local: Mutex::new(None),
            latest: Mutex::new(None),
            bundle: Mutex::new(Ok(Vec::new())),
            marker: Mutex::new(None),
            events: Mutex::new(Vec::new()),
            confirmed: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl Deps {
    fn ready() -> Self {
        Self {
            local: Mutex::new(Some("v1".into())),
            latest: Mutex::new(Some("v2".into())),
            bundle: Mutex::new(Ok(vec![1, 2, 3])),
            ..Default::default()
        }
    }
}

impl HostUpgradeDependencies for Deps {
    fn suspend_automation_wakes(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("suspend".into());
        Ok(())
    }
    fn prepare_sharing_for_upgrade(&self) {
        self.calls.lock().unwrap().push("sharing".into());
    }
    fn quiesce_transcript_for_upgrade(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("quiesce".into());
        Ok(())
    }
    fn resume_interrupted_upgrade_turns(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("resume".into());
        Ok(())
    }
    fn resolve_bundle_version(&self) -> Result<Option<String>, String> {
        Ok(self.latest.lock().unwrap().clone())
    }
    fn load_bundle_bytes(&self, _version: &str) -> Result<Vec<u8>, String> {
        self.bundle.lock().unwrap().clone()
    }
    fn read_local_version(&self) -> Option<String> {
        self.local.lock().unwrap().clone()
    }
    fn is_version_swap_vetoed(&self, _version: &str, _local: Option<&str>) -> bool {
        false
    }
    fn stage_upgrade(
        &self,
        version: &str,
        bytes: &[u8],
        force_now: bool,
        reason: &str,
    ) -> Result<(), String> {
        assert_eq!(version, "v2");
        assert_eq!(bytes, &[1, 2, 3]);
        self.calls
            .lock()
            .unwrap()
            .push(format!("stage:{force_now}:{reason}"));
        Ok(())
    }
    fn read_marker_raw(&self) -> Result<Option<String>, String> {
        Ok(self.marker.lock().unwrap().clone())
    }
    fn delete_marker(&self) -> Result<(), String> {
        *self.marker.lock().unwrap() = None;
        Ok(())
    }
    fn report_host_upgrade(&self, metadata: HashMap<String, String>) {
        self.events.lock().unwrap().push(metadata);
    }
    fn report_host_upgrade_confirmed(
        &self,
        metadata: &BTreeMap<String, String>,
    ) -> Result<bool, String> {
        self.confirmed.lock().unwrap().push(metadata.clone());
        Ok(true)
    }
    fn log(&self, _level: &str, _message: &str) {}
    fn now_ms(&self) -> i64 {
        2_000
    }
}

#[test]
fn manual_update_stages_bundle_and_version_state_is_updated() {
    let deps = Arc::new(Deps::ready());
    let service = HostUpgradeService::new(deps.clone(), true);
    assert_eq!(
        service.resolve_host_bundle_identity_version(None),
        "v1".to_string()
    );
    assert_eq!(
        service.update_host_now(true, false),
        HostUpdateNowResult::Started {
            version: "v2".into()
        }
    );
    assert_eq!(service.update_state().staged_version.as_deref(), Some("v2"));
    let version = service.get_version_state();
    assert_eq!(version.host_version.as_deref(), Some("v1"));
    assert_eq!(version.latest_host_version.as_deref(), Some("v2"));
    assert_eq!(version.host_update_available, Some(true));
    assert!(
        deps.calls
            .lock()
            .unwrap()
            .contains(&"stage:true:manual-update-host".into())
    );
}

#[test]
fn upgrade_preparation_preserves_peer_order_and_resume_is_explicit() {
    let deps = Arc::new(Deps::ready());
    let service = HostUpgradeService::new(deps.clone(), false);
    service.prepare_for_upgrade().unwrap();
    service.resume_interrupted_upgrade_turns().unwrap();
    assert_eq!(
        *deps.calls.lock().unwrap(),
        vec!["suspend", "sharing", "quiesce", "resume"]
    );
}

#[test]
fn marker_forward_is_identity_gated_and_failed_swap_rearms_staged_version() {
    let deps = Arc::new(Deps::ready());
    *deps.marker.lock().unwrap() = Some(
        r#"{"outcome":"failed","fromVersion":"v1","toVersion":"v2","mode":"bundle","reason":"idle-auto-update","issuedAtMs":1000,"swapError":"swap-failed"}"#.into(),
    );
    let service = HostUpgradeService::new(deps.clone(), true);
    service.update_host_now(false, false);
    assert_eq!(service.update_state().staged_version.as_deref(), Some("v2"));
    service.activate_after_host_identity_ready().unwrap();
    assert!(deps.marker.lock().unwrap().is_none());
    assert_eq!(deps.confirmed.lock().unwrap().len(), 1);
    assert_eq!(service.update_state().staged_version, None);
    assert_eq!(
        service
            .update_state()
            .swap_retry_attempts
            .get("v2")
            .copied(),
        Some(1)
    );
}

#[test]
fn idle_watch_dedupes_already_staged_version() {
    let deps = Arc::new(Deps::ready());
    let service = HostUpgradeService::new(deps.clone(), true);
    service.run_shared_update_watch_tick(|| Ok(())).unwrap();
    service.run_shared_update_watch_tick(|| Ok(())).unwrap();
    let stages = deps
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|call| call.starts_with("stage:"))
        .count();
    assert_eq!(stages, 1);
}
