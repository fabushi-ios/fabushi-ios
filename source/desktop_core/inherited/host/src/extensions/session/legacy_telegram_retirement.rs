use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const TELEGRAM_DIRNAME: &str = "telegram";
pub const RETIRED_DIR_PREFIX: &str = ".retired-telegram-";
pub const RETIRED_MARKER_FILENAME: &str = ".retired-telegram-marker.json";
pub const APP_HOST_DIRNAME: &str = "app-host";
pub const FEATURE_HOST_DIRNAME: &str = "feature-host";
pub const PHASE_N_PLUS_2: &str = "N+2";
pub const PHASE_N_PLUS_3: &str = "N+3";

pub type TelegramKeyClearHook = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetiredTelegramMarker {
    pub retired_dir_name: Option<String>,
    pub retired_at_ms: u64,
    pub release_phase: String,
    pub purged: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purged_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyTelegramRetirementReport {
    pub success: bool,
    pub phase: String,
    pub retired_dir_name: Option<String>,
    pub retired_at_ms: Option<u64>,
    pub purged: bool,
    pub purged_at_ms: Option<u64>,
    pub telegram_key_cleared: bool,
    pub marker_recovered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LegacyTelegramRetirementError {
    #[error("no valid native credential")]
    NoValidNativeCredential,
    #[error("path validation failed: {0}")]
    SecurityViolation(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("internal error: {0}")]
    Internal(String),
}

pub fn feature_host_dir(account_data_root: &Path) -> PathBuf {
    account_data_root
        .join(APP_HOST_DIRNAME)
        .join(FEATURE_HOST_DIRNAME)
}

pub fn marker_path(account_data_root: &Path) -> PathBuf {
    feature_host_dir(account_data_root).join(RETIRED_MARKER_FILENAME)
}

pub fn legacy_telegram_dir(account_data_root: &Path) -> PathBuf {
    feature_host_dir(account_data_root).join(TELEGRAM_DIRNAME)
}

pub fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn validate_path_containment(
    parent: &Path,
    child: &Path,
) -> Result<(), LegacyTelegramRetirementError> {
    for component in child.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(LegacyTelegramRetirementError::SecurityViolation(
                "parent directory references (..) not allowed".into(),
            ));
        }
    }
    if parent.exists() && child.exists() {
        let canonical_parent = parent
            .canonicalize()
            .map_err(|e| LegacyTelegramRetirementError::Io(e.to_string()))?;
        let canonical_child = child
            .canonicalize()
            .map_err(|e| LegacyTelegramRetirementError::Io(e.to_string()))?;
        if !canonical_child.starts_with(&canonical_parent) {
            return Err(LegacyTelegramRetirementError::SecurityViolation(format!(
                "path {} escapes allowed parent {}",
                child.display(),
                parent.display()
            )));
        }
    }
    Ok(())
}

pub fn scan_retired_telegram_dirs(
    feature_host: &Path,
) -> Result<Vec<PathBuf>, LegacyTelegramRetirementError> {
    if !feature_host.exists() {
        return Ok(Vec::new());
    }
    let mut dirs = Vec::new();
    let entries =
        fs::read_dir(feature_host).map_err(|e| LegacyTelegramRetirementError::Io(e.to_string()))?;
    for entry in entries {
        let entry = entry.map_err(|e| LegacyTelegramRetirementError::Io(e.to_string()))?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with(RETIRED_DIR_PREFIX)
            && name_str != RETIRED_MARKER_FILENAME
            && !name_str.ends_with(".tmp")
        {
            let path = entry.path();
            let ft = entry
                .file_type()
                .map_err(|e| LegacyTelegramRetirementError::Io(e.to_string()))?;
            if ft.is_dir() {
                dirs.push(path);
            }
        }
    }
    dirs.sort();
    Ok(dirs)
}

pub fn recover_marker_if_needed(
    feature_host: &Path,
) -> Result<Option<RetiredTelegramMarker>, LegacyTelegramRetirementError> {
    let marker_file = feature_host.join(RETIRED_MARKER_FILENAME);
    if marker_file.exists() {
        let content = fs::read_to_string(&marker_file)
            .map_err(|e| LegacyTelegramRetirementError::Io(e.to_string()))?;
        let marker: RetiredTelegramMarker = serde_json::from_str(&content)
            .map_err(|e| LegacyTelegramRetirementError::Internal(e.to_string()))?;
        return Ok(Some(marker));
    }

    let retired_dirs = scan_retired_telegram_dirs(feature_host)?;
    if let Some(retired_dir) = retired_dirs.last() {
        let dir_name = retired_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let timestamp = dir_name
            .strip_prefix(RETIRED_DIR_PREFIX)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or_else(current_timestamp_ms);

        let marker = RetiredTelegramMarker {
            retired_dir_name: Some(dir_name),
            retired_at_ms: timestamp,
            release_phase: PHASE_N_PLUS_2.to_string(),
            purged: false,
            purged_at_ms: None,
        };
        write_marker_atomically(&marker_file, &marker)?;
        return Ok(Some(marker));
    }

    Ok(None)
}

pub fn write_marker_atomically(
    path: &Path,
    marker: &RetiredTelegramMarker,
) -> Result<(), LegacyTelegramRetirementError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| LegacyTelegramRetirementError::Io(e.to_string()))?;
    }
    let data = serde_json::to_vec_pretty(marker)
        .map_err(|e| LegacyTelegramRetirementError::Internal(e.to_string()))?;
    let tmp_path = path.with_extension(format!("tmp.{}", uuid::Uuid::new_v4()));
    fs::write(&tmp_path, data).map_err(|e| LegacyTelegramRetirementError::Io(e.to_string()))?;
    fs::rename(&tmp_path, path).map_err(|e| LegacyTelegramRetirementError::Io(e.to_string()))?;
    Ok(())
}

pub fn retire_legacy_telegram_data_internal(
    account_data_root: &Path,
    has_valid_native_credential: bool,
    remove_immediately: bool,
    key_clear_callback: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<LegacyTelegramRetirementReport, LegacyTelegramRetirementError> {
    // 1. Precondition gate: only allowed when has_valid_native_credential == true
    if !has_valid_native_credential {
        return Err(LegacyTelegramRetirementError::NoValidNativeCredential);
    }

    let feature_host = feature_host_dir(account_data_root);
    let marker_file = feature_host.join(RETIRED_MARKER_FILENAME);
    let telegram_dir = feature_host.join(TELEGRAM_DIRNAME);

    // Crash recovery scan: check if .retired-telegram-* exists without marker
    let (current_marker, marker_recovered) = if feature_host.exists() {
        let had_marker = marker_file.exists();
        let marker = recover_marker_if_needed(&feature_host)?;
        let was_recovered = !had_marker && marker.is_some();
        (marker, was_recovered)
    } else {
        (None, false)
    };

    if remove_immediately {
        // Phase 2: Purge
        let mut key_cleared = false;

        // If marker already indicates purged: true, remain completely idempotent
        if let Some(ref m) = current_marker {
            if m.purged {
                if let Some(cb) = key_clear_callback {
                    cb().map_err(|e| {
                        LegacyTelegramRetirementError::Internal(format!(
                            "failed to clear telegram-key: {e}"
                        ))
                    })?;
                    key_cleared = true;
                }
                return Ok(LegacyTelegramRetirementReport {
                    success: true,
                    phase: PHASE_N_PLUS_3.to_string(),
                    retired_dir_name: m.retired_dir_name.clone(),
                    retired_at_ms: Some(m.retired_at_ms),
                    purged: true,
                    purged_at_ms: m.purged_at_ms,
                    telegram_key_cleared: key_cleared,
                    marker_recovered,
                });
            }
        }

        if feature_host.exists() {
            // Delete any .retired-telegram-* directories
            let retired_dirs = scan_retired_telegram_dirs(&feature_host)?;
            for dir in retired_dirs {
                if let Ok(meta) = fs::symlink_metadata(&dir) {
                    if meta.file_type().is_symlink() {
                        let _ = fs::remove_file(&dir);
                    } else if validate_path_containment(&feature_host, &dir).is_ok() {
                        fs::remove_dir_all(&dir)
                            .map_err(|e| LegacyTelegramRetirementError::Io(e.to_string()))?;
                    }
                }
            }

            // If still unrenamed telegram directory exists, validate boundary and remove safely
            if telegram_dir.exists() {
                if let Ok(meta) = fs::symlink_metadata(&telegram_dir) {
                    if meta.file_type().is_symlink() {
                        let _ = fs::remove_file(&telegram_dir);
                    } else {
                        validate_path_containment(&feature_host, &telegram_dir)?;
                        fs::remove_dir_all(&telegram_dir)
                            .map_err(|e| LegacyTelegramRetirementError::Io(e.to_string()))?;
                    }
                }
            }
        }

        // Trigger telegram-key clear callback
        if let Some(cb) = key_clear_callback {
            cb().map_err(|e| {
                LegacyTelegramRetirementError::Internal(format!(
                    "failed to clear telegram-key: {e}"
                ))
            })?;
            key_cleared = true;
        }

        let now_ms = current_timestamp_ms();
        let retired_at_ms = current_marker
            .as_ref()
            .map(|m| m.retired_at_ms)
            .unwrap_or(now_ms);
        let retired_dir_name = current_marker
            .as_ref()
            .and_then(|m| m.retired_dir_name.clone());

        let marker = RetiredTelegramMarker {
            retired_dir_name: retired_dir_name.clone(),
            retired_at_ms,
            release_phase: PHASE_N_PLUS_3.to_string(),
            purged: true,
            purged_at_ms: Some(now_ms),
        };
        write_marker_atomically(&marker_file, &marker)?;

        Ok(LegacyTelegramRetirementReport {
            success: true,
            phase: PHASE_N_PLUS_3.to_string(),
            retired_dir_name,
            retired_at_ms: Some(retired_at_ms),
            purged: true,
            purged_at_ms: Some(now_ms),
            telegram_key_cleared: key_cleared,
            marker_recovered,
        })
    } else {
        // Phase 1: Retire / Rename (Release N+2)
        if let Some(ref m) = current_marker {
            // Already retired or purged: idempotent
            return Ok(LegacyTelegramRetirementReport {
                success: true,
                phase: m.release_phase.clone(),
                retired_dir_name: m.retired_dir_name.clone(),
                retired_at_ms: Some(m.retired_at_ms),
                purged: m.purged,
                purged_at_ms: m.purged_at_ms,
                telegram_key_cleared: false,
                marker_recovered,
            });
        }

        if telegram_dir.exists() {
            let meta = fs::symlink_metadata(&telegram_dir)
                .map_err(|e| LegacyTelegramRetirementError::Io(e.to_string()))?;
            if meta.file_type().is_symlink() {
                return Err(LegacyTelegramRetirementError::SecurityViolation(
                    "telegram directory is a symlink".into(),
                ));
            }
            validate_path_containment(&feature_host, &telegram_dir)?;

            let now_ms = current_timestamp_ms();
            let retired_name = format!("{RETIRED_DIR_PREFIX}{now_ms}");
            let dest_dir = feature_host.join(&retired_name);

            fs::rename(&telegram_dir, &dest_dir)
                .map_err(|e| LegacyTelegramRetirementError::Io(e.to_string()))?;

            let marker = RetiredTelegramMarker {
                retired_dir_name: Some(retired_name.clone()),
                retired_at_ms: now_ms,
                release_phase: PHASE_N_PLUS_2.to_string(),
                purged: false,
                purged_at_ms: None,
            };
            write_marker_atomically(&marker_file, &marker)?;

            Ok(LegacyTelegramRetirementReport {
                success: true,
                phase: PHASE_N_PLUS_2.to_string(),
                retired_dir_name: Some(retired_name),
                retired_at_ms: Some(now_ms),
                purged: false,
                purged_at_ms: None,
                telegram_key_cleared: false,
                marker_recovered: false,
            })
        } else {
            // Nothing to retire
            Ok(LegacyTelegramRetirementReport {
                success: true,
                phase: PHASE_N_PLUS_2.to_string(),
                retired_dir_name: None,
                retired_at_ms: None,
                purged: false,
                purged_at_ms: None,
                telegram_key_cleared: false,
                marker_recovered,
            })
        }
    }
}
