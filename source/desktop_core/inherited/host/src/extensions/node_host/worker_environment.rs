//! Node-worker environment custody without process or workspace side effects.
//!
//! The node owner captures provider transport and platform trust before the
//! generic environment is sanitized. A later worker owner may project the
//! returned snapshot into an admitted child; this module owns no process,
//! workspace, lease, persistence, or retry lifecycle.

use std::collections::BTreeMap;

const MAX_ENV_VALUE_BYTES: usize = 16 * 1024;

const POSIX_KEYS: &[&str] = &[
    "PATH",
    "HOME",
    "TMPDIR",
    "TMP",
    "TEMP",
    "LANG",
    "LANGUAGE",
    "TZ",
    "DISPLAY",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_RUNTIME_DIR",
    "NODE_EXTRA_CA_CERTS",
    "NODE_USE_SYSTEM_CA",
    "OPENCLAW_ALLOW_INSECURE_PRIVATE_WS",
];

const WINDOWS_KEYS: &[&str] = &[
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeWorkerPlatform {
    Posix,
    Windows,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeWorkerPlatformTrust {
    pub requests_ca_bundle: Option<String>,
    pub ssl_cert_file: Option<String>,
    pub node_extra_ca_certs: Option<String>,
    pub node_use_system_ca: Option<String>,
}

impl NodeWorkerPlatformTrust {
    pub fn capture(source: &BTreeMap<String, String>) -> Self {
        Self {
            requests_ca_bundle: source.get("REQUESTS_CA_BUNDLE").cloned(),
            ssl_cert_file: source.get("SSL_CERT_FILE").cloned(),
            node_extra_ca_certs: source.get("NODE_EXTRA_CA_CERTS").cloned(),
            node_use_system_ca: source.get("NODE_USE_SYSTEM_CA").cloned(),
        }
    }

    pub fn apply(&self, target: &mut BTreeMap<String, String>) {
        for key in [
            "REQUESTS_CA_BUNDLE",
            "SSL_CERT_FILE",
            "NODE_EXTRA_CA_CERTS",
            "NODE_USE_SYSTEM_CA",
        ] {
            target.remove(key);
        }
        insert_optional(target, "REQUESTS_CA_BUNDLE", self.requests_ca_bundle.as_deref());
        insert_optional(target, "SSL_CERT_FILE", self.ssl_cert_file.as_deref());
        insert_optional(
            target,
            "NODE_EXTRA_CA_CERTS",
            self.node_extra_ca_certs.as_deref(),
        );
        insert_optional(
            target,
            "NODE_USE_SYSTEM_CA",
            self.node_use_system_ca.as_deref(),
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeWorkerManagedIdentityTransport {
    pub endpoint: String,
    pub header: String,
}

impl NodeWorkerManagedIdentityTransport {
    pub fn capture(
        source: &BTreeMap<String, String>,
        registrar: Option<&dyn NodeWorkerSecretRegistrar>,
    ) -> Result<Option<Self>, String> {
        let endpoint = source.get("IDENTITY_ENDPOINT");
        let header = source.get("IDENTITY_HEADER");
        if endpoint.is_none() && header.is_none() {
            return Ok(None);
        }
        let endpoint = endpoint
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "managed-identity endpoint is incomplete".to_string())?;
        let header = header
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "managed-identity header is incomplete".to_string())?;
        validate_env_value(endpoint, "managed-identity endpoint")?;
        validate_env_value(header, "managed-identity header")?;
        let registrar = registrar.ok_or_else(|| {
            "managed-identity header requires an injected secret registrar".to_string()
        })?;
        registrar.register_secret(header);
        Ok(Some(Self {
            endpoint: endpoint.clone(),
            header: header.clone(),
        }))
    }
}

pub trait NodeWorkerSecretRegistrar: Send + Sync {
    fn register_secret(&self, value: &str);
}

/// Produce the bounded non-secret environment inherited by a node worker.
/// Identity transport is removed before the optional admitted transport is
/// restored; caller-controlled environment keys cannot smuggle it in.
pub fn snapshot_worker_environment(
    source: &BTreeMap<String, String>,
    platform: NodeWorkerPlatform,
    home_dir: Option<&str>,
    trust: Option<&NodeWorkerPlatformTrust>,
    managed_identity: Option<&NodeWorkerManagedIdentityTransport>,
) -> Result<BTreeMap<String, String>, String> {
    let mut snapshot = BTreeMap::new();
    let windows = matches!(platform, NodeWorkerPlatform::Windows);
    for (key, value) in source {
        validate_env_value(value, key)?;
        let normalized = if windows {
            key.to_ascii_uppercase()
        } else {
            key.clone()
        };
        let allowed = POSIX_KEYS.contains(&normalized.as_str())
            || (windows && WINDOWS_KEYS.contains(&normalized.as_str()))
            || normalized.starts_with("LC_");
        if allowed {
            snapshot.insert(key.clone(), value.clone());
        }
    }

    remove_case_insensitive(&mut snapshot, "IDENTITY_ENDPOINT");
    remove_case_insensitive(&mut snapshot, "IDENTITY_HEADER");
    if let Some(home_dir) = home_dir {
        validate_env_value(home_dir, "worker home")?;
        remove_case_insensitive(&mut snapshot, "HOME");
        if windows {
            remove_case_insensitive(&mut snapshot, "USERPROFILE");
        }
        snapshot.insert("HOME".into(), home_dir.into());
        if windows {
            snapshot.insert("USERPROFILE".into(), home_dir.into());
        }
    }
    if let Some(trust) = trust {
        trust.apply(&mut snapshot);
    }
    if let Some(managed_identity) = managed_identity {
        snapshot.insert(
            "IDENTITY_ENDPOINT".into(),
            managed_identity.endpoint.clone(),
        );
        snapshot.insert("IDENTITY_HEADER".into(), managed_identity.header.clone());
    }
    Ok(snapshot)
}

fn insert_optional(target: &mut BTreeMap<String, String>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        target.insert(key.into(), value.into());
    }
}

fn remove_case_insensitive(target: &mut BTreeMap<String, String>, key: &str) {
    let key = key.to_ascii_uppercase();
    target.retain(|candidate, _| candidate.to_ascii_uppercase() != key);
}

fn validate_env_value(value: &str, field: &str) -> Result<(), String> {
    if value.len() > MAX_ENV_VALUE_BYTES || value.chars().any(char::is_control) {
        return Err(format!("{field} is invalid or too large"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        NodeWorkerManagedIdentityTransport, NodeWorkerPlatform, NodeWorkerPlatformTrust,
        NodeWorkerSecretRegistrar, snapshot_worker_environment,
    };
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    struct Registrar(Mutex<Vec<String>>);

    impl NodeWorkerSecretRegistrar for Registrar {
        fn register_secret(&self, value: &str) {
            self.0.lock().expect("registrar lock").push(value.into());
        }
    }

    #[test]
    fn managed_identity_requires_pair_and_registration() {
        let mut source = BTreeMap::from([(String::from("IDENTITY_HEADER"), String::from("h"))]);
        assert!(NodeWorkerManagedIdentityTransport::capture(&source, None).is_err());
        source.insert("IDENTITY_ENDPOINT".into(), "https://identity".into());
        let registrar = Registrar(Mutex::new(Vec::new()));
        let transport = NodeWorkerManagedIdentityTransport::capture(&source, Some(&registrar))
            .expect("capture")
            .expect("transport");
        assert_eq!(transport.header, "h");
        assert_eq!(registrar.0.lock().expect("registrar lock").as_slice(), ["h"]);
    }

    #[test]
    fn snapshot_filters_caller_identity_and_restores_admitted_values() {
        let source = BTreeMap::from([
            ("PATH".into(), "/bin".into()),
            ("SECRET_TOKEN".into(), "must-drop".into()),
            ("IDENTITY_ENDPOINT".into(), "caller-endpoint".into()),
            ("IDENTITY_HEADER".into(), "caller-header".into()),
        ]);
        let trust = NodeWorkerPlatformTrust {
            requests_ca_bundle: Some("/trusted/ca".into()),
            ssl_cert_file: None,
            node_extra_ca_certs: None,
            node_use_system_ca: None,
        };
        let identity = NodeWorkerManagedIdentityTransport {
            endpoint: "https://owned".into(),
            header: "owned-header".into(),
        };
        let snapshot = snapshot_worker_environment(
            &source,
            NodeWorkerPlatform::Posix,
            Some("/owned/home"),
            Some(&trust),
            Some(&identity),
        )
        .expect("snapshot");
        assert!(!snapshot.contains_key("SECRET_TOKEN"));
        assert_eq!(snapshot.get("IDENTITY_ENDPOINT"), Some(&"https://owned".into()));
        assert_eq!(snapshot.get("IDENTITY_HEADER"), Some(&"owned-header".into()));
        assert_eq!(snapshot.get("REQUESTS_CA_BUNDLE"), Some(&"/trusted/ca".into()));
    }
}
