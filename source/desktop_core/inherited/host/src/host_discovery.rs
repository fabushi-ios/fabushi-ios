use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayDiscoveryInfo {
    pub port: u16,
    pub pid: u32,
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl GatewayDiscoveryInfo {
    pub fn validate(&self) -> io::Result<()> {
        if self.port == 0 || self.pid == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid gateway discovery info",
            ));
        }
        if self
            .scheme
            .as_deref()
            .is_some_and(|scheme| scheme != "http" && scheme != "https")
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid gateway discovery scheme",
            ));
        }
        Ok(())
    }
}

fn temp_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.{}.tmp", path.display(), std::process::id()))
}

pub fn write_gateway_discovery(info: &GatewayDiscoveryInfo, path: &Path) -> io::Result<()> {
    info.validate()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = temp_path(path);
    let encoded = serde_json::to_vec_pretty(info)
        .map_err(|error| io::Error::other(format!("serialize gateway discovery: {error}")))?;
    fs::write(&temporary, encoded)?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::AlreadyExists | io::ErrorKind::PermissionDenied
            ) =>
        {
            let _ = fs::remove_file(path);
            fs::rename(&temporary, path)
        }
        Err(error) => Err(error),
    }
}

pub fn clear_gateway_discovery(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
