use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;

use serde_json::{Map, Value};

use crate::storage::folder_id::is_safe_folder_id;

use super::session_diagnostics::{SessionDiagnostic, report_session_diagnostic};

pub type SecretRecord = Map<String, Value>;

#[derive(Debug, thiserror::Error)]
pub enum ConnectorSecretStoreError {
    #[error("connector secret filesystem error: {0}")]
    Io(#[from] io::Error),
    #[error("connector secret serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct SandConnectorSecretStore {
    secrets_root: PathBuf,
}

impl SandConnectorSecretStore {
    pub fn new(secrets_root: impl Into<PathBuf>) -> Self {
        Self {
            secrets_root: secrets_root.into(),
        }
    }

    pub fn secrets_root(&self) -> &Path {
        &self.secrets_root
    }

    pub fn file_path(&self, agent_id: &str, platform: &str) -> PathBuf {
        self.secrets_root
            .join(agent_id)
            .join(format!("{platform}.json"))
    }

    pub fn read(&self, agent_id: &str, platform: &str) -> SecretRecord {
        let path = self.file_path(agent_id, platform);
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return SecretRecord::new(),
            Err(error) => {
                report_unreadable(agent_id, &error);
                return SecretRecord::new();
            }
        };
        match serde_json::from_str::<Value>(&raw) {
            Ok(Value::Object(record)) => record,
            Ok(_) => SecretRecord::new(),
            Err(error) => {
                report_session_diagnostic(&SessionDiagnostic {
                    family: "store_db".into(),
                    kind: "connector_secrets_unreadable".into(),
                    metadata: BTreeMap::from([
                        ("agentId".into(), Value::String(agent_id.to_string())),
                        ("errorClass".into(), Value::String(format!("{:?}", error.classify()))),
                    ]),
                });
                SecretRecord::new()
            }
        }
    }

    pub fn set_secret(
        &self,
        agent_id: &str,
        platform: &str,
        field: &str,
        value: &str,
    ) -> Result<bool, ConnectorSecretStoreError> {
        if !is_safe_folder_id(agent_id)
            || !is_safe_folder_id(platform)
            || field.is_empty()
        {
            return Ok(false);
        }
        let path = self.file_path(agent_id, platform);
        let mut merged = self.read(agent_id, platform);
        merged.insert(field.to_string(), Value::String(value.to_string()));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = PathBuf::from(format!("{}.{}.tmp", path.display(), process::id()));
        let mut serialized = serde_json::to_string_pretty(&Value::Object(merged))?;
        serialized.push('\n');
        fs::write(&temporary, serialized)?;
        if let Err(error) = fs::rename(&temporary, &path) {
            let _ = fs::remove_file(&temporary);
            return Err(error.into());
        }
        Ok(true)
    }

    pub fn get_secret(
        &self,
        agent_id: &str,
        platform: &str,
        field: &str,
    ) -> Option<String> {
        if !is_safe_folder_id(agent_id) || !is_safe_folder_id(platform) {
            return None;
        }
        self.read(agent_id, platform)
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }

    pub fn remove_agent_platform(
        &self,
        agent_id: &str,
        platform: &str,
    ) -> Result<bool, ConnectorSecretStoreError> {
        if !is_safe_folder_id(agent_id) || !is_safe_folder_id(platform) {
            return Ok(false);
        }
        let path = self.file_path(agent_id, platform);
        match fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
}

fn report_unreadable(agent_id: &str, error: &io::Error) {
    report_session_diagnostic(&SessionDiagnostic {
        family: "store_db".into(),
        kind: "connector_secrets_unreadable".into(),
        metadata: BTreeMap::from([
            ("agentId".into(), Value::String(agent_id.to_string())),
            (
                "errorClass".into(),
                Value::String(format!("{:?}", error.kind())),
            ),
        ]),
    });
}
