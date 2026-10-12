use mahayana_secrets::{
    LocalSecretsNamespace, SecretName, SecretScope, SecretsBackendKind, SecretsManager,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const CHANNEL_METADATA_VERSION: u8 = 1;
const CHANNEL_TOKEN_NAME: &str = "CHANNEL_TOKEN";
const MAX_CHANNEL_LABEL: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentChannelConnection {
    pub platform: String,
    pub label: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ChannelMetadata {
    label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChannelMetadataFile {
    version: u8,
    #[serde(default)]
    agents: BTreeMap<String, BTreeMap<String, ChannelMetadata>>,
}

impl Default for ChannelMetadataFile {
    fn default() -> Self {
        Self {
            version: CHANNEL_METADATA_VERSION,
            agents: BTreeMap::new(),
        }
    }
}

/// Rust-owned channel state for the Agent Info surface.
///
/// Metadata is account-scoped and durable. Credential payloads live in a
/// separate encrypted Mahayana secret namespace rooted below the Host runtime.
/// The secret value is never part of AgentChannelConnection or any RPC reply.
pub(crate) struct AgentChannelStore {
    metadata_base: Option<PathBuf>,
    secrets: Option<SecretsManager>,
    memory_metadata: Mutex<BTreeMap<String, ChannelMetadataFile>>,
    memory_tokens: Mutex<BTreeMap<String, String>>,
}

impl AgentChannelStore {
    pub(crate) fn new(
        runtime_data_dir: Option<&Path>,
        storage_passphrase: Option<String>,
    ) -> Self {
        let metadata_base = runtime_data_dir.map(|root| root.join("channels.json"));
        let secrets = runtime_data_dir.map(|root| {
            let home = root.join("channel-secret-store");
            match storage_passphrase {
                Some(passphrase) => SecretsManager::new_with_namespace_and_passphrase(
                    home,
                    SecretsBackendKind::Local,
                    LocalSecretsNamespace::ManagedSecrets,
                    passphrase,
                ),
                None => SecretsManager::new_with_namespace(
                    home,
                    SecretsBackendKind::Local,
                    LocalSecretsNamespace::ManagedSecrets,
                ),
            }
        });
        Self {
            metadata_base,
            secrets,
            memory_metadata: Mutex::new(BTreeMap::new()),
            memory_tokens: Mutex::new(BTreeMap::new()),
        }
    }

    pub(crate) fn list_connections(
        &self,
        account_id: &str,
        agent_id: &str,
    ) -> Result<Vec<AgentChannelConnection>, String> {
        validate_agent_id(agent_id)?;
        let file = self.load_metadata(account_id)?;
        Ok(project_connections(
            file.agents.get(agent_id),
        ))
    }

    pub(crate) fn connect(
        &self,
        account_id: &str,
        agent_id: &str,
        platform: &str,
        token: &str,
    ) -> Result<Vec<AgentChannelConnection>, String> {
        validate_agent_id(agent_id)?;
        let platform = normalize_platform(platform)?;
        if token.trim().is_empty() {
            return Err("channel credential must not be empty".into());
        }

        let mut file = self.load_metadata(account_id)?;
        let label = normalize_label("", &platform);
        self.set_token(account_id, agent_id, &platform, token)?;
        file.agents
            .entry(agent_id.to_string())
            .or_default()
            .insert(platform.clone(), ChannelMetadata { label });
        if let Err(error) = self.save_metadata(account_id, &file) {
            let _ = self.delete_token(account_id, agent_id, &platform);
            return Err(error);
        }
        Ok(project_connections(file.agents.get(agent_id)))
    }

    pub(crate) fn disconnect(
        &self,
        account_id: &str,
        agent_id: &str,
        platform: &str,
    ) -> Result<Vec<AgentChannelConnection>, String> {
        validate_agent_id(agent_id)?;
        let platform = normalize_platform(platform)?;
        let mut file = self.load_metadata(account_id)?;

        // Match the Desktop ownership order: retire the secret first, then
        // remove the UI-safe metadata projection.
        self.delete_token(account_id, agent_id, &platform)?;
        if let Some(agent) = file.agents.get_mut(agent_id) {
            agent.remove(&platform);
            if agent.is_empty() {
                file.agents.remove(agent_id);
            }
        }
        self.save_metadata(account_id, &file)?;
        Ok(project_connections(file.agents.get(agent_id)))
    }

    pub(crate) fn refresh(
        &self,
        account_id: &str,
        agent_id: &str,
        platform: &str,
    ) -> Result<Vec<AgentChannelConnection>, String> {
        validate_agent_id(agent_id)?;
        let _ = normalize_platform(platform)?;
        self.list_connections(account_id, agent_id)
    }

    fn metadata_path(&self, account_id: &str) -> Option<PathBuf> {
        let base = self.metadata_base.as_deref()?;
        Some(
            base.parent()
                .unwrap_or_else(|| Path::new("."))
                .join("accounts")
                .join(fingerprint(account_id))
                .join(
                    base.file_name()
                        .unwrap_or_else(|| std::ffi::OsStr::new("channels.json")),
                ),
        )
    }

    fn load_metadata(&self, account_id: &str) -> Result<ChannelMetadataFile, String> {
        if let Some(path) = self.metadata_path(account_id) {
            if !path.exists() {
                return Ok(ChannelMetadataFile::default());
            }
            let bytes = fs::read(&path)
                .map_err(|error| format!("read channel metadata: {error}"))?;
            let file: ChannelMetadataFile = serde_json::from_slice(&bytes)
                .map_err(|error| format!("decode channel metadata: {error}"))?;
            if file.version > CHANNEL_METADATA_VERSION {
                return Err(format!(
                    "channel metadata version {} is newer than supported version {}",
                    file.version, CHANNEL_METADATA_VERSION
                ));
            }
            return Ok(file);
        }

        self.memory_metadata
            .lock()
            .map_err(|_| "channel metadata lock poisoned".to_string())
            .map(|state| {
                state
                    .get(&fingerprint(account_id))
                    .cloned()
                    .unwrap_or_default()
            })
    }

    fn save_metadata(
        &self,
        account_id: &str,
        file: &ChannelMetadataFile,
    ) -> Result<(), String> {
        if let Some(path) = self.metadata_path(account_id) {
            if file.agents.is_empty() {
                match fs::remove_file(&path) {
                    Ok(()) => return Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                    Err(error) => return Err(format!("remove channel metadata: {error}")),
                }
            }
            let parent = path
                .parent()
                .ok_or_else(|| "channel metadata path has no parent".to_string())?;
            fs::create_dir_all(parent)
                .map_err(|error| format!("create channel metadata directory: {error}"))?;
            let bytes = serde_json::to_vec(file)
                .map_err(|error| format!("encode channel metadata: {error}"))?;
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos());
            let part = parent.join(format!(
                ".channels-{}-{nonce}.part",
                std::process::id()
            ));
            fs::write(&part, bytes)
                .map_err(|error| format!("write channel metadata: {error}"))?;
            match fs::rename(&part, &path) {
                Ok(()) => Ok(()),
                Err(first_error) if path.exists() => {
                    fs::remove_file(&path)
                        .map_err(|error| format!("replace channel metadata: {error}"))?;
                    fs::rename(&part, &path).map_err(|_| {
                        format!("commit channel metadata: {first_error}")
                    })
                }
                Err(error) => Err(format!("commit channel metadata: {error}")),
            }
        } else {
            let mut state = self
                .memory_metadata
                .lock()
                .map_err(|_| "channel metadata lock poisoned".to_string())?;
            let key = fingerprint(account_id);
            if file.agents.is_empty() {
                state.remove(&key);
            } else {
                state.insert(key, file.clone());
            }
            Ok(())
        }
    }

    fn secret_scope(
        account_id: &str,
        agent_id: &str,
        platform: &str,
    ) -> Result<SecretScope, String> {
        SecretScope::environment(format!(
            "channel-{}-{}-{}",
            fingerprint(account_id),
            fingerprint(agent_id),
            platform
        ))
        .map_err(|error| format!("channel secret scope: {error}"))
    }

    fn secret_name() -> Result<SecretName, String> {
        SecretName::new(CHANNEL_TOKEN_NAME)
            .map_err(|error| format!("channel secret name: {error}"))
    }

    fn memory_secret_key(account_id: &str, agent_id: &str, platform: &str) -> String {
        format!(
            "{}:{}:{}",
            fingerprint(account_id),
            fingerprint(agent_id),
            platform
        )
    }

    fn set_token(
        &self,
        account_id: &str,
        agent_id: &str,
        platform: &str,
        token: &str,
    ) -> Result<(), String> {
        if let Some(secrets) = self.secrets.as_ref() {
            return secrets
                .set(
                    &Self::secret_scope(account_id, agent_id, platform)?,
                    &Self::secret_name()?,
                    token,
                )
                .map_err(|error| format!("store channel credential: {error}"));
        }
        self.memory_tokens
            .lock()
            .map_err(|_| "channel credential lock poisoned".to_string())?
            .insert(
                Self::memory_secret_key(account_id, agent_id, platform),
                token.to_string(),
            );
        Ok(())
    }

    fn delete_token(
        &self,
        account_id: &str,
        agent_id: &str,
        platform: &str,
    ) -> Result<(), String> {
        if let Some(secrets) = self.secrets.as_ref() {
            secrets
                .delete(
                    &Self::secret_scope(account_id, agent_id, platform)?,
                    &Self::secret_name()?,
                )
                .map_err(|error| format!("delete channel credential: {error}"))?;
            return Ok(());
        }
        self.memory_tokens
            .lock()
            .map_err(|_| "channel credential lock poisoned".to_string())?
            .remove(&Self::memory_secret_key(account_id, agent_id, platform));
        Ok(())
    }

    #[cfg(test)]
    fn token_exists(
        &self,
        account_id: &str,
        agent_id: &str,
        platform: &str,
    ) -> Result<bool, String> {
        if let Some(secrets) = self.secrets.as_ref() {
            return secrets
                .get(
                    &Self::secret_scope(account_id, agent_id, platform)?,
                    &Self::secret_name()?,
                )
                .map(|value| value.is_some())
                .map_err(|error| format!("read channel credential: {error}"));
        }
        Ok(self
            .memory_tokens
            .lock()
            .map_err(|_| "channel credential lock poisoned".to_string())?
            .contains_key(&Self::memory_secret_key(account_id, agent_id, platform)))
    }
}

fn validate_agent_id(agent_id: &str) -> Result<(), String> {
    let trimmed = agent_id.trim();
    if trimmed.is_empty() || trimmed.len() > 512 {
        return Err("channel agent id is invalid".into());
    }
    Ok(())
}

fn normalize_platform(platform: &str) -> Result<String, String> {
    let value = platform.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 64
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric()
                || character == '-'
                || character == '_'
                || character == '.')
    {
        return Err("channel platform is invalid".into());
    }
    Ok(value)
}

fn normalize_label(raw: &str, platform: &str) -> String {
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized = if normalized.is_empty() {
        match platform {
            "discord" => "Discord",
            "slack" => "Slack",
            _ => platform,
        }
        .to_string()
    } else {
        normalized
    };
    normalized.chars().take(MAX_CHANNEL_LABEL).collect()
}

fn project_connections(
    metadata: Option<&BTreeMap<String, ChannelMetadata>>,
) -> Vec<AgentChannelConnection> {
    metadata
        .into_iter()
        .flat_map(BTreeMap::iter)
        .map(|(platform, metadata)| AgentChannelConnection {
            platform: platform.clone(),
            label: metadata.label.clone(),
            status: "configured".into(),
            detail: None,
        })
        .collect()
}

fn fingerprint(value: &str) -> String {
    Sha256::digest(value.as_bytes())[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        std::env::temp_dir().join(format!(
            "fabushi-channel-store-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn in_memory_projection_never_returns_channel_credential() {
        let store = AgentChannelStore::new(None, None);
        let connections = store
            .connect("account-a", "agent-a", "discord", "secret-discord-token")
            .expect("connect");
        assert_eq!(connections.len(), 1);
        assert_eq!(connections[0].platform, "discord");
        assert_eq!(connections[0].label, "Discord");
        assert_eq!(connections[0].status, "configured");
        let encoded = serde_json::to_string(&connections).expect("encode");
        assert!(!encoded.contains("secret-discord-token"));
        assert!(store
            .token_exists("account-a", "agent-a", "discord")
            .expect("token exists"));

        let disconnected = store
            .disconnect("account-a", "agent-a", "discord")
            .expect("disconnect");
        assert!(disconnected.is_empty());
        assert!(!store
            .token_exists("account-a", "agent-a", "discord")
            .expect("token removed"));
    }

    #[test]
    fn encrypted_channel_token_is_separate_from_account_scoped_metadata() {
        let root = temp_root("encrypted");
        fs::create_dir_all(&root).expect("create root");
        let store = AgentChannelStore::new(
            Some(&root),
            Some("fixed-channel-test-passphrase".into()),
        );
        store
            .connect("account-a", "agent-a", "slack", "secret-slack-token")
            .expect("connect");

        let metadata_path = store
            .metadata_path("account-a")
            .expect("metadata path");
        let metadata = fs::read_to_string(&metadata_path).expect("metadata");
        assert!(metadata.contains("Slack"));
        assert!(!metadata.contains("secret-slack-token"));

        let ciphertext = fs::read(root.join("channel-secret-store/secrets/local.age"))
            .expect("encrypted secret store");
        assert!(!String::from_utf8_lossy(&ciphertext).contains("secret-slack-token"));

        drop(store);
        let reopened = AgentChannelStore::new(
            Some(&root),
            Some("fixed-channel-test-passphrase".into()),
        );
        assert_eq!(
            reopened
                .list_connections("account-a", "agent-a")
                .expect("restore")
                .len(),
            1
        );
        assert!(reopened
            .token_exists("account-a", "agent-a", "slack")
            .expect("restore token"));
        assert!(reopened
            .list_connections("account-b", "agent-a")
            .expect("isolated account")
            .is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn platform_and_token_validation_fail_closed() {
        let store = AgentChannelStore::new(None, None);
        assert!(store
            .connect("account", "agent", "../slack", "token")
            .is_err());
        assert!(store
            .connect("account", "agent", "slack", "   ")
            .is_err());
    }
}
