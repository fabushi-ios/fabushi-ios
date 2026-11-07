use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub const SAND_CSNAPS_BIN_ENV: &str = "SAND_CSNAPS_BIN";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsnapsUnavailableReason {
    Missing,
    NotFile,
    NotExecutable,
}

impl CsnapsUnavailableReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::NotFile => "not-file",
            Self::NotExecutable => "not-executable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CsnapsCapability {
    Available {
        executable_path: PathBuf,
    },
    Unavailable {
        executable_path: PathBuf,
        reason: CsnapsUnavailableReason,
    },
}

impl CsnapsCapability {
    pub fn available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    pub fn executable_path(&self) -> &Path {
        match self {
            Self::Available { executable_path }
            | Self::Unavailable {
                executable_path, ..
            } => executable_path,
        }
    }
}

pub fn resolve_csnaps_bin_path_in(
    env: &BTreeMap<String, String>,
    host_bundle_directory: &Path,
) -> PathBuf {
    if let Some(value) = env
        .get(SAND_CSNAPS_BIN_ENV)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(value);
    }

    host_bundle_directory
        .join("extensions")
        .join("codebase-telemetry")
        .join("csnaps")
}

fn default_host_bundle_directory() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn resolve_csnaps_bin_path() -> PathBuf {
    let env = std::env::vars().collect::<BTreeMap<_, _>>();
    resolve_csnaps_bin_path_in(&env, &default_host_bundle_directory())
}

fn is_executable_file(path: &Path) -> Result<bool, CsnapsUnavailableReason> {
    let metadata = fs::metadata(path).map_err(|_| CsnapsUnavailableReason::Missing)?;
    if !metadata.is_file() {
        return Err(CsnapsUnavailableReason::NotFile);
    }

    #[cfg(unix)]
    {
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(CsnapsUnavailableReason::NotExecutable);
        }
    }

    Ok(true)
}

pub fn resolve_csnaps_capability_at(executable_path: PathBuf) -> CsnapsCapability {
    match is_executable_file(&executable_path) {
        Ok(true) => CsnapsCapability::Available { executable_path },
        Ok(false) => CsnapsCapability::Unavailable {
            executable_path,
            reason: CsnapsUnavailableReason::NotExecutable,
        },
        Err(reason) => CsnapsCapability::Unavailable {
            executable_path,
            reason,
        },
    }
}

pub fn resolve_csnaps_capability() -> CsnapsCapability {
    resolve_csnaps_capability_at(resolve_csnaps_bin_path())
}
