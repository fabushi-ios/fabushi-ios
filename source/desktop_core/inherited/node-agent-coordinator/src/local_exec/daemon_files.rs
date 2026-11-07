use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const LOCAL_EXEC_DAEMON_CONNECTION_FILENAME: &str = "local-exec-daemon-connection.json";
pub const LOCAL_EXEC_DAEMON_DISCOVERY_FILENAME: &str = "local-exec-daemon.json";
pub const LOCAL_EXEC_DAEMON_CREDENTIAL_FILENAME: &str = "local-exec-daemon-credential.json";
pub const LOCAL_EXEC_SUPERVISOR_HEARTBEAT_FILENAME: &str = "local-exec-supervisor.json";
pub const LOCAL_EXEC_DAEMON_LOG_FILENAME: &str = "local-exec-daemon.log";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalExecDaemonPaths {
    pub connection_path: PathBuf,
    pub discovery_path: PathBuf,
    pub credential_path: PathBuf,
    pub log_path: PathBuf,
    pub supervisor_heartbeat_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalExecDaemonDiscovery {
    pub pid: u32,
    pub started_at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_realpath: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inflight_count: Option<u64>,
}

impl LocalExecDaemonDiscovery {
    pub fn validate(&self) -> bool {
        self.pid > 0
            && self.started_at.is_finite()
            && self.entry_realpath.as_ref().is_none_or(|value| !value.is_empty())
            && self.generation_token.as_ref().is_none_or(|value| !value.is_empty())
    }

    pub fn same_generation(&self, other: &Self) -> bool {
        self.pid == other.pid
            && self.started_at == other.started_at
            && self.entry_realpath == other.entry_realpath
            && self.generation_token == other.generation_token
    }
}

pub fn resolve_local_exec_daemon_paths(data_dir: impl AsRef<Path>) -> LocalExecDaemonPaths {
    let data_dir = data_dir.as_ref();
    LocalExecDaemonPaths {
        connection_path: data_dir.join(LOCAL_EXEC_DAEMON_CONNECTION_FILENAME),
        discovery_path: data_dir.join(LOCAL_EXEC_DAEMON_DISCOVERY_FILENAME),
        credential_path: data_dir.join(LOCAL_EXEC_DAEMON_CREDENTIAL_FILENAME),
        log_path: data_dir.join(LOCAL_EXEC_DAEMON_LOG_FILENAME),
        supervisor_heartbeat_path: data_dir.join(LOCAL_EXEC_SUPERVISOR_HEARTBEAT_FILENAME),
    }
}

pub fn parse_discovery(value: Value) -> Option<LocalExecDaemonDiscovery> {
    let discovery: LocalExecDaemonDiscovery = serde_json::from_value(value).ok()?;
    discovery.validate().then_some(discovery)
}

pub fn read_local_exec_daemon_discovery(path: impl AsRef<Path>) -> io::Result<Option<LocalExecDaemonDiscovery>> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let parsed = serde_json::from_str::<Value>(&raw).ok().and_then(parse_discovery);
    Ok(parsed)
}

pub fn write_secret_json_file(path: impl AsRef<Path>, data: &Value) -> io::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("{}.{}.tmp", path.extension().and_then(|v| v.to_str()).unwrap_or("json"), std::process::id()));
    {
        let mut file = fs::File::create(&temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        serde_json::to_writer(&mut file, data)
            .map_err(|error| io::Error::other(format!("serialize secret json: {error}")))?;
        file.flush()?;
        file.sync_all()?;
    }
    fs::rename(temporary, path)
}

pub fn write_local_exec_daemon_connection(path: impl AsRef<Path>, connection: &Value) -> io::Result<()> {
    write_secret_json_file(path, connection)
}

pub fn write_local_exec_daemon_credential(path: impl AsRef<Path>, credential: &Value) -> io::Result<()> {
    write_secret_json_file(path, credential)
}

pub fn write_local_exec_supervisor_heartbeat(path: impl AsRef<Path>, at_ms: u64) -> io::Result<()> {
    write_secret_json_file(
        path,
        &serde_json::json!({ "pid": std::process::id(), "at": at_ms }),
    )
}

pub fn remove_local_exec_daemon_file(path: impl AsRef<Path>) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub fn remove_local_exec_daemon_discovery_if_matches(
    path: impl AsRef<Path>,
    expected: &LocalExecDaemonDiscovery,
) -> io::Result<bool> {
    let path = path.as_ref();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let quarantine = PathBuf::from(format!(
        "{}.{}.{}.retired",
        path.display(),
        std::process::id(),
        nonce
    ));

    match fs::rename(path, &quarantine) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    }

    let result = match read_local_exec_daemon_discovery(&quarantine)? {
        Some(actual) if actual.same_generation(expected) => true,
        _ => {
            match fs::hard_link(&quarantine, path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    let _ = fs::remove_file(&quarantine);
                    return Err(error);
                }
            }
            false
        }
    };
    remove_local_exec_daemon_file(&quarantine)?;
    Ok(result)
}
