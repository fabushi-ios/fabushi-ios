//! Static, iOS-owned native package execution foundation.
//!
//! This runner deliberately does not spawn package-owned processes. It binds the
//! canonical PluginInstaller active pointer to an account/app/version/digest grant,
//! creates owner-only state/run directories, and hands one bounded bootstrap request
//! to the existing PlatformExecBroker settlement state machine.

use crate::dandelion_portable::{
    core::{RunStatus, Snapshot, lowercase_hex_digest},
    pacing::ControlPacer,
    scanner::{ensure_private_directory, validate_installed_directory},
    state::{RunnerPhase, RunnerState},
    transport::{BOOTSTRAP_METHOD, Bootstrap, bootstrap_params, validate_bootstrap_result},
};
use mahayana_native_engine::{PlatformExecBroker, PlatformExecRequest};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use uuid::Uuid;

const MAX_RUNNERS: usize = 8;
const CONTROL_BUDGET_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CONTROL_FRAME_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeGrant {
    pub token: String,
    pub account_scope: String,
    pub account_generation: String,
    pub app_id: String,
    pub version: String,
    pub package_digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalPackageSnapshot {
    pub plugin_id: String,
    pub version: String,
    pub artifact_sha256: String,
    pub runtime: String,
    pub installed_path: PathBuf,
    pub requested_permissions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerLease {
    pub runner_id: String,
    pub account_scope: String,
    pub account_generation: String,
    pub app_id: String,
    pub version: String,
    pub package_digest: String,
    pub grant_token: String,
}

struct StaticRunner {
    lease: RunnerLease,
    broker: Arc<PlatformExecBroker>,
    state: Arc<Mutex<RunnerState>>,
    snapshot: Arc<Mutex<Snapshot>>,
    private_directory: PathBuf,
    run_directory: PathBuf,
    worker: Option<JoinHandle<()>>,
}

impl StaticRunner {
    fn cancel(&mut self, reason: &str) {
        let _ = self.broker.cancel_all(reason);
        if let Ok(mut state) = self.state.lock() {
            state.cancel(reason.to_string());
        }
        if let Ok(mut snapshot) = self.snapshot.lock() {
            snapshot.status = RunStatus::Cancelled;
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = fs::remove_dir_all(&self.run_directory);
    }

    fn close(mut self) {
        let _ = self.broker.close("Dandelion static runner finished");
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Ok(mut state) = self.state.lock() {
            state.close();
        }
        let _ = fs::remove_dir_all(&self.run_directory);
    }

    fn status(&self) -> Value {
        let state = self.state.lock().ok();
        let snapshot = self.snapshot.lock().ok();
        json!({
            "runnerId": self.lease.runner_id,
            "accountScope": self.lease.account_scope,
            "accountGeneration": self.lease.account_generation,
            "appId": self.lease.app_id,
            "version": self.lease.version,
            "packageDigest": self.lease.package_digest,
            "phase": state.as_ref().map(|value| value.phase()),
            "revision": state.as_ref().map(|value| value.revision()).unwrap_or_default(),
            "terminalMessage": state.as_ref().and_then(|value| value.terminal_message()),
            "controlBytesReserved": snapshot.as_ref().map(|value| value.control_bytes_reserved).unwrap_or_default(),
            "privateDirectoryOwned": self.private_directory.is_dir(),
            "runDirectoryOwned": self.run_directory.is_dir(),
            "executionEnabled": false,
        })
    }
}

#[derive(Default)]
pub struct StaticRunnerRegistry {
    runners: BTreeMap<String, StaticRunner>,
}

impl StaticRunnerRegistry {
    pub fn start(
        &mut self,
        grant: &RuntimeGrant,
        package: &CanonicalPackageSnapshot,
        plugin_root: &Path,
        private_base: &Path,
    ) -> Result<RunnerLease, String> {
        if self.runners.len() >= MAX_RUNNERS {
            return Err("Dandelion static runner limit reached".into());
        }
        validate_grant(grant)?;
        validate_package(grant, package)?;

        let installed_directory =
            validate_installed_directory(&package.installed_path, plugin_root)?;
        let private_identity = private_identity(grant);
        let private_directory =
            ensure_private_directory(&private_base.join(&private_identity))?;
        let runner_id = Uuid::new_v4().to_string();
        let run_directory =
            ensure_private_directory(&private_directory.join("runs").join(&runner_id))?;

        let lease = RunnerLease {
            runner_id: runner_id.clone(),
            account_scope: grant.account_scope.clone(),
            account_generation: grant.account_generation.clone(),
            app_id: grant.app_id.clone(),
            version: grant.version.clone(),
            package_digest: grant.package_digest.clone(),
            grant_token: grant.token.clone(),
        };

        let params = bootstrap_params(Bootstrap {
            runner_id: &runner_id,
            account_scope: &grant.account_scope,
            account_generation: &grant.account_generation,
            app_id: &grant.app_id,
            version: &grant.version,
            package_digest: &grant.package_digest,
            grant_token: &grant.token,
            package_directory: &installed_directory,
            private_directory: &private_directory,
            run_directory: &run_directory,
        });
        let encoded = serde_json::to_vec(&params).map_err(|error| error.to_string())?;
        let mut pacer = ControlPacer::new(CONTROL_BUDGET_BYTES, MAX_CONTROL_FRAME_BYTES)?;
        pacer.reserve_frame(encoded.len())?;

        let broker = Arc::new(PlatformExecBroker::new(8));
        let state = Arc::new(Mutex::new(RunnerState::default()));
        let snapshot = Arc::new(Mutex::new(Snapshot {
            control_bytes_reserved: pacer.reserved(),
            ..Snapshot::default()
        }));
        let worker_broker = Arc::clone(&broker);
        let worker_state = Arc::clone(&state);
        let worker_snapshot = Arc::clone(&snapshot);
        let worker = std::thread::Builder::new()
            .name(format!("fabushi-dandelion-{}", &runner_id[..8]))
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread().build() {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        if let Ok(mut state) = worker_state.lock() {
                            state.fail(format!("create static runner runtime: {error}"));
                        }
                        return;
                    }
                };
                let result = runtime.block_on(worker_broker.execute(BOOTSTRAP_METHOD, params));
                match result {
                    Ok(value) => match validate_bootstrap_result(&value) {
                        Ok(()) => {
                            if let Ok(mut state) = worker_state.lock() {
                                let _ = state.platform_ready();
                            }
                        }
                        Err(error) => {
                            if let Ok(mut state) = worker_state.lock() {
                                state.fail(error);
                            }
                        }
                    },
                    Err(error) => {
                        let cancelled = error.starts_with("cancelled:");
                        if let Ok(mut state) = worker_state.lock() {
                            if cancelled {
                                state.cancel(error);
                            } else if state.phase() != RunnerPhase::Cancelled {
                                state.fail(error);
                            }
                        }
                        if cancelled {
                            if let Ok(mut snapshot) = worker_snapshot.lock() {
                                snapshot.status = RunStatus::Cancelled;
                            }
                        }
                    }
                }
            })
            .map_err(|error| error.to_string())?;

        self.runners.insert(
            runner_id,
            StaticRunner {
                lease: lease.clone(),
                broker,
                state,
                snapshot,
                private_directory,
                run_directory,
                worker: Some(worker),
            },
        );
        Ok(lease)
    }

    pub fn next_platform_request(
        &self,
        runner_id: &str,
    ) -> Result<Option<PlatformExecRequest>, String> {
        let runner = self.runner(runner_id)?;
        runner
            .broker
            .next_request()
            .map_err(|error| error.to_string())
    }

    pub fn resolve_platform_request(
        &self,
        runner_id: &str,
        request_id: &str,
        result: Value,
    ) -> Result<(), String> {
        let runner = self.runner(runner_id)?;
        runner
            .broker
            .resolve(request_id, result)
            .map_err(|error| error.to_string())
    }

    pub fn reject_platform_request(
        &self,
        runner_id: &str,
        request_id: &str,
        code: &str,
        message: &str,
    ) -> Result<(), String> {
        let runner = self.runner(runner_id)?;
        runner
            .broker
            .reject(request_id, code, message)
            .map_err(|error| error.to_string())
    }

    pub fn status(&self, runner_id: &str) -> Result<Value, String> {
        Ok(self.runner(runner_id)?.status())
    }

    pub fn cancel(&mut self, runner_id: &str, reason: &str) -> Result<bool, String> {
        let Some(runner) = self.runners.get_mut(runner_id) else {
            return Ok(false);
        };
        runner.cancel(reason);
        Ok(true)
    }

    pub fn finish(&mut self, runner_id: &str) -> Result<bool, String> {
        let Some(runner) = self.runners.remove(runner_id) else {
            return Ok(false);
        };
        runner.close();
        Ok(true)
    }

    pub fn cancel_all(&mut self, reason: &str) {
        for runner in self.runners.values_mut() {
            runner.cancel(reason);
        }
    }

    pub fn clear(&mut self, reason: &str) {
        let runners = std::mem::take(&mut self.runners);
        for (_, mut runner) in runners {
            runner.cancel(reason);
            runner.close();
        }
    }

    pub fn active_count(&self) -> usize {
        self.runners.len()
    }

    fn runner(&self, runner_id: &str) -> Result<&StaticRunner, String> {
        self.runners
            .get(runner_id)
            .ok_or_else(|| "Dandelion static runner is unavailable".to_string())
    }
}

fn validate_grant(grant: &RuntimeGrant) -> Result<(), String> {
    validate_token(&grant.token, "grantToken", 128)?;
    validate_identity(&grant.account_scope, "accountScope")?;
    validate_identity(&grant.account_generation, "accountGeneration")?;
    validate_token(&grant.app_id, "appId", 128)?;
    validate_token(&grant.version, "version", 128)?;
    if !lowercase_hex_digest(&grant.package_digest) {
        return Err("packageDigest must be lowercase 64-byte hexadecimal".into());
    }
    Ok(())
}

fn validate_package(
    grant: &RuntimeGrant,
    package: &CanonicalPackageSnapshot,
) -> Result<(), String> {
    if package.plugin_id != grant.app_id
        || package.version != grant.version
        || package.artifact_sha256 != grant.package_digest
    {
        return Err("Dandelion canonical active package does not match the runtime grant".into());
    }
    if package.runtime != "native" {
        return Err("Dandelion static runner requires a canonical native package runtime".into());
    }
    if !package
        .requested_permissions
        .iter()
        .any(|permission| permission == "native.runtime")
    {
        return Err("Dandelion static runner requires an explicit native.runtime declaration".into());
    }
    Ok(())
}

fn validate_identity(value: &str, field: &str) -> Result<(), String> {
    if !lowercase_hex_digest(value) {
        return Err(format!("{field} must be lowercase 64-byte hexadecimal"));
    }
    Ok(())
}

fn validate_token(value: &str, field: &str, limit: usize) -> Result<(), String> {
    if value.is_empty()
        || value.len() > limit
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!("{field} is invalid"));
    }
    Ok(())
}

fn private_identity(grant: &RuntimeGrant) -> String {
    let digest = Sha256::digest(
        [
            grant.account_scope.as_bytes(),
            b"\0",
            grant.app_id.as_bytes(),
        ]
        .concat(),
    );
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dandelion_portable::transport::BOOTSTRAP_PROTOCOL;
    use std::time::{Duration, Instant};

    fn temp_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "fabushi-dandelion-static-runner-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn fixtures(root: &Path) -> (RuntimeGrant, CanonicalPackageSnapshot, PathBuf, PathBuf) {
        let scope = "a".repeat(64);
        let generation = "b".repeat(64);
        let app_id = "global-dharma".to_string();
        let version = "1.2.3".to_string();
        let digest = "c".repeat(64);
        let plugin_root = root
            .join("plugins")
            .join("accounts")
            .join(&scope)
            .join(&app_id);
        let installed = plugin_root
            .join("versions")
            .join(&version)
            .join("artifact");
        fs::create_dir_all(&installed).unwrap();
        let private = root.join("dandelion-native").join("accounts").join(&scope);
        (
            RuntimeGrant {
                token: "grant-token".into(),
                account_scope: scope,
                account_generation: generation,
                app_id: app_id.clone(),
                version: version.clone(),
                package_digest: digest.clone(),
            },
            CanonicalPackageSnapshot {
                plugin_id: app_id,
                version,
                artifact_sha256: digest,
                runtime: "native".into(),
                installed_path: installed,
                requested_permissions: vec!["native.runtime".into()],
            },
            plugin_root,
            private,
        )
    }

    fn wait_request(registry: &StaticRunnerRegistry, runner_id: &str) -> PlatformExecRequest {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(request) = registry.next_platform_request(runner_id).unwrap() {
                return request;
            }
            assert!(Instant::now() < deadline, "static runner did not enqueue bootstrap");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn account_app_version_digest_and_grant_are_fenced_before_queueing() {
        let root = temp_root();
        let (grant, package, plugin_root, private) = fixtures(&root);
        let mut registry = StaticRunnerRegistry::default();

        let mut stale = grant.clone();
        stale.package_digest = "d".repeat(64);
        assert!(registry.start(&stale, &package, &plugin_root, &private).is_err());

        let mut wrong_app = package.clone();
        wrong_app.plugin_id = "other-app".into();
        assert!(registry.start(&grant, &wrong_app, &plugin_root, &private).is_err());

        let mut wrong_version = package.clone();
        wrong_version.version = "9.9.9".into();
        assert!(registry.start(&grant, &wrong_version, &plugin_root, &private).is_err());

        let mut bad_token = grant.clone();
        bad_token.token = "../escape".into();
        assert!(registry.start(&bad_token, &package, &plugin_root, &private).is_err());

        let mut web_package = package.clone();
        web_package.runtime = "local-web".into();
        assert!(registry.start(&grant, &web_package, &plugin_root, &private).is_err());

        let mut undeclared = package.clone();
        undeclared.requested_permissions.clear();
        assert!(registry.start(&grant, &undeclared, &plugin_root, &private).is_err());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn runner_uses_platform_exec_pending_settlement_and_cancellation() {
        let root = temp_root();
        let (grant, package, plugin_root, private) = fixtures(&root);
        let mut registry = StaticRunnerRegistry::default();
        let lease = registry.start(&grant, &package, &plugin_root, &private).unwrap();
        let request = wait_request(&registry, &lease.runner_id);
        assert_eq!(request.method, BOOTSTRAP_METHOD);
        assert_eq!(request.params["grantToken"], grant.token);

        assert!(registry.cancel(&lease.runner_id, "test cancellation").unwrap());
        let status = registry.status(&lease.runner_id).unwrap();
        assert_eq!(status["phase"], "cancelled");
        assert_eq!(status["runDirectoryOwned"], false);
        registry.finish(&lease.runner_id).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn private_directory_persists_but_per_run_directory_is_owned_and_reclaimed() {
        let root = temp_root();
        let (grant, package, plugin_root, private) = fixtures(&root);
        let mut registry = StaticRunnerRegistry::default();
        let lease = registry.start(&grant, &package, &plugin_root, &private).unwrap();
        let request = wait_request(&registry, &lease.runner_id);
        registry
            .resolve_platform_request(
                &lease.runner_id,
                &request.request_id,
                json!({"protocol": BOOTSTRAP_PROTOCOL, "accepted": true, "executionEnabled": false}),
            )
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let status = registry.status(&lease.runner_id).unwrap();
            if status["phase"] == "running" {
                assert_eq!(status["privateDirectoryOwned"], true);
                assert_eq!(status["runDirectoryOwned"], true);
                break;
            }
            assert!(Instant::now() < deadline, "static runner did not settle bootstrap");
            std::thread::sleep(Duration::from_millis(5));
        }
        registry.finish(&lease.runner_id).unwrap();
        assert!(private.exists(), "account-scoped private root remains durable");
        let _ = fs::remove_dir_all(root);
    }
}
