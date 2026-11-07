use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use mahayana_node_agent_coordinator::local_exec::{
    ExpectedLocalExecProcessIdentity, LocalExecControl, LocalExecDaemonOrigin,
    LocalExecDaemonRuntime, LocalExecDaemonState, LocalExecProcessIdentity,
    LocalExecSupervisorError, SpawnLocalExecDaemonRequest, SAND_CLIENT_PAUSE_REASON,
};
use mahayana_node_agent_coordinator::local_exec::daemon_files::{
    resolve_local_exec_daemon_paths, write_secret_json_file,
};
use serde_json::{json, Value};
use uuid::Uuid;

struct FakeControl {
    data_dir: PathBuf,
    now_ms: u64,
    next_pid: u32,
    spawn_count: u32,
    terminate_count: u32,
    resolve_count: u32,
    credential_count: u32,
    pause_connection: bool,
    identities: HashMap<u32, LocalExecProcessIdentity>,
}

impl FakeControl {
    fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            now_ms: 10_000,
            next_pid: 100,
            spawn_count: 0,
            terminate_count: 0,
            resolve_count: 0,
            credential_count: 0,
            pause_connection: false,
            identities: HashMap::new(),
        }
    }

    fn install_existing(&mut self, pid: u32, generation: &str, inflight_count: u64) {
        let entry = "/opt/fabushi/local-exec.js";
        let identity = LocalExecProcessIdentity {
            pid,
            start_epoch_ms: self.now_ms.saturating_sub(100),
            command: format!(
                "/usr/bin/node {entry} --sand-local-exec-generation={generation}"
            ),
            entry_realpath: entry.into(),
            generation_token: generation.into(),
        };
        self.identities.insert(pid, identity.clone());
        let paths = resolve_local_exec_daemon_paths(&self.data_dir);
        write_secret_json_file(
            &paths.discovery_path,
            &json!({
                "pid": pid,
                "startedAt": self.now_ms - 50,
                "entryRealpath": entry,
                "generationToken": generation,
                "inflightCount": inflight_count,
            }),
        )
        .expect("write existing discovery");
    }
}

impl LocalExecControl for FakeControl {
    fn now_ms(&self) -> u64 {
        self.now_ms
    }

    fn sleep_ms(&mut self, duration_ms: u64) {
        self.now_ms = self.now_ms.saturating_add(duration_ms);
    }

    fn resolve_gateway_connection(&mut self) -> Result<Value, LocalExecSupervisorError> {
        self.resolve_count += 1;
        if self.pause_connection {
            return Err(LocalExecSupervisorError::new(format!(
                "{SAND_CLIENT_PAUSE_REASON}: client requested pause"
            )));
        }
        Ok(json!({
            "baseUrl": "http://127.0.0.1:4567",
            "headers": { "authorization": "Bearer test" }
        }))
    }

    fn mint_local_exec_daemon_credential(
        &mut self,
    ) -> Result<Option<Value>, LocalExecSupervisorError> {
        self.credential_count += 1;
        Ok(Some(json!({ "token": "credential" })))
    }

    fn spawn_local_exec_daemon(
        &mut self,
        request: SpawnLocalExecDaemonRequest,
    ) -> Result<LocalExecProcessIdentity, LocalExecSupervisorError> {
        assert_eq!(request.env.get("ELECTRON_RUN_AS_NODE").map(String::as_str), Some("1"));
        self.spawn_count += 1;
        let pid = self.next_pid;
        self.next_pid += 1;
        let generation = format!("generation-{}", self.spawn_count);
        let entry = "/opt/fabushi/local-exec.js";
        let identity = LocalExecProcessIdentity {
            pid,
            start_epoch_ms: self.now_ms,
            command: format!(
                "/usr/bin/node {entry} --sand-local-exec-generation={generation}"
            ),
            entry_realpath: entry.into(),
            generation_token: generation.clone(),
        };
        self.identities.insert(pid, identity.clone());
        let paths = resolve_local_exec_daemon_paths(&self.data_dir);
        write_secret_json_file(
            &paths.discovery_path,
            &json!({
                "pid": pid,
                "startedAt": self.now_ms,
                "entryRealpath": entry,
                "generationToken": generation,
                "inflightCount": 0,
            }),
        )
        .map_err(|error| LocalExecSupervisorError::new(error.to_string()))?;
        Ok(identity)
    }

    fn is_process_alive(&mut self, pid: u32) -> Result<bool, LocalExecSupervisorError> {
        Ok(self.identities.contains_key(&pid))
    }

    fn get_process_identity(
        &mut self,
        expected: &ExpectedLocalExecProcessIdentity,
    ) -> Result<Option<LocalExecProcessIdentity>, LocalExecSupervisorError> {
        let Some(identity) = self.identities.get(&expected.pid).cloned() else {
            return Ok(None);
        };
        if identity.entry_realpath != expected.entry_realpath
            || identity.generation_token != expected.generation_token
        {
            return Ok(None);
        }
        if expected.start_epoch_ms.is_some_and(|value| value != identity.start_epoch_ms) {
            return Ok(None);
        }
        if expected.command.as_ref().is_some_and(|value| value != &identity.command) {
            return Ok(None);
        }
        Ok(Some(identity))
    }

    fn terminate_process(
        &mut self,
        identity: &LocalExecProcessIdentity,
    ) -> Result<bool, LocalExecSupervisorError> {
        if self
            .identities
            .get(&identity.pid)
            .is_some_and(|current| current == identity)
        {
            self.identities.remove(&identity.pid);
            self.terminate_count += 1;
            return Ok(true);
        }
        Ok(false)
    }
}

fn temp_dir(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "fabushi-local-exec-{label}-{}",
        Uuid::new_v4()
    ));
    fs::create_dir_all(&root).expect("create temp root");
    root
}

fn active_pid(state: &LocalExecDaemonState) -> u32 {
    match state {
        LocalExecDaemonState::Active { pid, .. } => *pid,
        other => panic!("expected active daemon, got {other:?}"),
    }
}

#[test]
fn runtime_spawns_pauses_and_resumes_with_generation_fencing() {
    let root = temp_dir("spawn-pause");
    let control = FakeControl::new(root.clone());
    let mut runtime = LocalExecDaemonRuntime::new(root.clone(), true, control);

    runtime.start();
    assert!(matches!(
        runtime.state(),
        LocalExecDaemonState::Active {
            origin: LocalExecDaemonOrigin::Spawned,
            ..
        }
    ));
    let first_pid = active_pid(runtime.state());
    assert_eq!(runtime.control().spawn_count, 1);
    assert_eq!(runtime.control().credential_count, 1);

    let paths = resolve_local_exec_daemon_paths(&root);
    assert!(paths.connection_path.exists());
    assert!(paths.credential_path.exists());
    assert!(paths.supervisor_heartbeat_path.exists());

    runtime.set_paused(true);
    assert!(runtime.is_paused());
    assert!(matches!(runtime.state(), LocalExecDaemonState::Absent));
    assert!(!paths.connection_path.exists());
    assert!(!runtime.control().identities.contains_key(&first_pid));

    runtime.set_paused(false);
    assert!(!runtime.is_paused());
    assert!(matches!(runtime.state(), LocalExecDaemonState::Active { .. }));
    assert_ne!(active_pid(runtime.state()), first_pid);
    assert_eq!(runtime.control().spawn_count, 2);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn runtime_adopts_busy_generation_and_replaces_idle_generation() {
    let busy_root = temp_dir("adopt");
    let mut busy_control = FakeControl::new(busy_root.clone());
    busy_control.install_existing(71, "busy-generation", 2);
    let mut busy_runtime = LocalExecDaemonRuntime::new(busy_root.clone(), false, busy_control);
    busy_runtime.start();
    assert!(matches!(
        busy_runtime.state(),
        LocalExecDaemonState::Active {
            origin: LocalExecDaemonOrigin::Adopted,
            pid: 71,
            ..
        }
    ));
    assert_eq!(busy_runtime.control().spawn_count, 0);
    assert_eq!(busy_runtime.control().terminate_count, 0);

    let idle_root = temp_dir("replace");
    let mut idle_control = FakeControl::new(idle_root.clone());
    idle_control.install_existing(72, "idle-generation", 0);
    let mut idle_runtime = LocalExecDaemonRuntime::new(idle_root.clone(), false, idle_control);
    idle_runtime.start();
    assert!(matches!(
        idle_runtime.state(),
        LocalExecDaemonState::Active {
            origin: LocalExecDaemonOrigin::Spawned,
            ..
        }
    ));
    assert_eq!(idle_runtime.control().terminate_count, 1);
    assert_eq!(idle_runtime.control().spawn_count, 1);
    assert_ne!(active_pid(idle_runtime.state()), 72);

    let _ = fs::remove_dir_all(busy_root);
    let _ = fs::remove_dir_all(idle_root);
}

#[test]
fn runtime_quarantines_same_pid_generation_change_without_duplicate_spawn() {
    let root = temp_dir("quarantine");
    let control = FakeControl::new(root.clone());
    let mut runtime = LocalExecDaemonRuntime::new(root.clone(), false, control);
    runtime.start();

    let pid = active_pid(runtime.state());
    let paths = resolve_local_exec_daemon_paths(&root);
    write_secret_json_file(
        &paths.discovery_path,
        &json!({
            "pid": pid,
            "startedAt": runtime.control().now_ms,
            "entryRealpath": "/opt/fabushi/local-exec.js",
            "generationToken": "foreign-generation",
            "inflightCount": 0,
        }),
    )
    .expect("replace discovery generation");

    runtime.liveness_tick();
    assert!(matches!(
        runtime.state(),
        LocalExecDaemonState::Failed { reason }
            if reason.contains("generation changed")
    ));
    assert_eq!(runtime.control().spawn_count, 1);
    assert!(runtime.control().identities.contains_key(&pid));

    runtime.liveness_tick();
    assert_eq!(
        runtime.control().spawn_count,
        1,
        "verified live quarantined process blocks duplicate spawn"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn client_pause_refusal_prevents_spawn_until_connection_recovers() {
    let root = temp_dir("client-pause");
    let mut control = FakeControl::new(root.clone());
    control.pause_connection = true;
    let mut runtime = LocalExecDaemonRuntime::new(root.clone(), false, control);
    runtime.start();

    assert!(runtime.refused_for_client_pause());
    assert_eq!(runtime.control().spawn_count, 0);
    assert!(matches!(runtime.state(), LocalExecDaemonState::Absent));

    runtime.control_mut().pause_connection = false;
    runtime.set_paused(true);
    runtime.set_paused(false);
    assert!(!runtime.refused_for_client_pause());
    assert_eq!(runtime.control().spawn_count, 1);
    assert!(matches!(runtime.state(), LocalExecDaemonState::Active { .. }));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn identity_helpers_require_exact_generation_arguments_and_bounded_publication_time() {
    use mahayana_node_agent_coordinator::local_exec::{
        command_carries_local_exec_generation, local_exec_discovery_time_matches_process,
    };

    assert!(command_carries_local_exec_generation(
        "/usr/bin/node /opt/local.js --sand-local-exec-generation=g1",
        "/opt/local.js",
        "g1",
    ));
    assert!(!command_carries_local_exec_generation(
        "/usr/bin/node /opt/local.js.bak --sand-local-exec-generation=g10",
        "/opt/local.js",
        "g1",
    ));
    assert!(local_exec_discovery_time_matches_process(10_500.0, 10_000, 11_000));
    assert!(!local_exec_discovery_time_matches_process(70_001.0, 10_000, 70_001));

    let _ = Path::new(".");
}
