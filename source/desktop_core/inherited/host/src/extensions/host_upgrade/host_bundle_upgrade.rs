use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::host_upgrade_marker::HostUpgradeMarker;

pub const SAND_SUPERVISOR_DIR: &str = "/tmp/sand-supervisor";
pub const SAND_SUPERVISOR_COMMAND_PATH: &str = "/tmp/sand-supervisor/command.json";
pub const SAND_SUPERVISOR_COMMAND_PART_PATH: &str = "/tmp/sand-supervisor/command.json.part";
pub const SAND_SUPERVISOR_ACKS_DIR: &str = "/tmp/sand-supervisor/acks";
pub const SAND_SUPERVISOR_STAGED_BUNDLE_PATH: &str =
    "/tmp/sand-supervisor/incoming-host-bundle.tgz";
pub const SAND_SUPERVISOR_STAGED_BUNDLE_PART_PATH: &str =
    "/tmp/sand-supervisor/incoming-host-bundle.tgz.part";
pub const SAND_BOX_HOST_VERSION_PATH: &str = "/home/box/sand-host/version";
pub const MAX_FAILED_SWAP_RESTAGE_RETRIES: usize = 2;
pub const POST_SWAP_CRASH_LOOP_ERROR_CLASS: &str = "post-swap-crash-loop";
pub const HOST_BUNDLE_WATCH_INTERVAL_MS: u64 = 24 * 60 * 60_000;
pub const HOST_BUNDLE_WATCH_JITTER_RATIO: f64 = 0.5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandSupervisorUpgradeCommand {
    pub id: String,
    pub kind: String,
    pub issued_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_now: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostBundleUpdateState {
    pub staged_version: Option<String>,
    pub last_failed_version: Option<String>,
    pub swap_retry_attempts: HashMap<String, usize>,
}

#[derive(Debug, Clone)]
pub struct StageHostBundleUpgradeArgs<'a> {
    pub version: &'a str,
    pub bytes: &'a [u8],
    pub reason: Option<&'a str>,
    pub force_now: bool,
    pub now_ms: i64,
    pub dir: PathBuf,
    pub staged_bundle_path: PathBuf,
    pub staged_bundle_part_path: PathBuf,
    pub command_path: PathBuf,
    pub command_part_path: PathBuf,
}

impl<'a> StageHostBundleUpgradeArgs<'a> {
    pub fn production(version: &'a str, bytes: &'a [u8], now_ms: i64) -> Self {
        Self {
            version,
            bytes,
            reason: None,
            force_now: false,
            now_ms,
            dir: PathBuf::from(SAND_SUPERVISOR_DIR),
            staged_bundle_path: PathBuf::from(SAND_SUPERVISOR_STAGED_BUNDLE_PATH),
            staged_bundle_part_path: PathBuf::from(SAND_SUPERVISOR_STAGED_BUNDLE_PART_PATH),
            command_path: PathBuf::from(SAND_SUPERVISOR_COMMAND_PATH),
            command_part_path: PathBuf::from(SAND_SUPERVISOR_COMMAND_PART_PATH),
        }
    }
}

pub fn build_sand_supervisor_command(
    id: impl Into<String>,
    now_ms: i64,
    version: Option<&str>,
    bundle_path: Option<&Path>,
    reason: Option<&str>,
    force_now: bool,
) -> SandSupervisorUpgradeCommand {
    SandSupervisorUpgradeCommand {
        id: id.into(),
        kind: "upgrade".into(),
        issued_at_ms: now_ms,
        mode: version.map(|_| "bundle".to_string()),
        version: version.map(str::to_string),
        bundle_path: bundle_path.map(|path| path.to_string_lossy().into_owned()),
        reason: reason.map(str::to_string),
        force_now: force_now.then_some(true),
    }
}

pub fn serialize_sand_supervisor_command(
    command: &SandSupervisorUpgradeCommand,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(command)
}

pub fn is_sand_host_upgrade_available(current: Option<&str>, target: Option<&str>) -> bool {
    target.is_some_and(|target| !target.is_empty() && current != Some(target))
}

pub fn read_local_host_version(path: impl AsRef<Path>) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub fn stage_host_bundle_upgrade(
    args: &StageHostBundleUpgradeArgs<'_>,
) -> io::Result<SandSupervisorUpgradeCommand> {
    fs::create_dir_all(&args.dir)?;
    fs::write(&args.staged_bundle_part_path, args.bytes)?;
    fs::rename(&args.staged_bundle_part_path, &args.staged_bundle_path)?;
    let command = build_sand_supervisor_command(
        format!("upgrade-{}", args.version),
        args.now_ms,
        Some(args.version),
        Some(&args.staged_bundle_path),
        args.reason,
        args.force_now,
    );
    let payload = serialize_sand_supervisor_command(&command)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    fs::write(&args.command_part_path, payload)?;
    fs::rename(&args.command_part_path, &args.command_path)?;
    Ok(command)
}

pub fn is_host_version_swap_vetoed(
    version: &str,
    local_version: Option<&str>,
    acks_dir: impl AsRef<Path>,
) -> bool {
    if version.is_empty() || local_version == Some(version) {
        return false;
    }
    let ack_name = format!("upgrade-{version}")
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    acks_dir.as_ref().join(ack_name).exists()
}

pub fn note_failed_swap_marker_for_retry(
    state: &mut HostBundleUpdateState,
    marker: &HostUpgradeMarker,
    max_retries: usize,
) -> bool {
    if marker.outcome.as_deref() != Some("failed")
        || marker.mode.as_deref() != Some("bundle")
        || marker.to_version.as_deref().is_none_or(str::is_empty)
        || marker.swap_error.as_deref() == Some(POST_SWAP_CRASH_LOOP_ERROR_CLASS)
        || state.staged_version.as_deref() != marker.to_version.as_deref()
    {
        return false;
    }
    let version = marker.to_version.as_ref().expect("checked above").clone();
    let attempts = state
        .swap_retry_attempts
        .get(&version)
        .copied()
        .unwrap_or(0);
    if attempts >= max_retries {
        return false;
    }
    state.swap_retry_attempts.insert(version, attempts + 1);
    state.staged_version = None;
    true
}

fn clamp(value: f64, low: f64, high: f64) -> f64 {
    if value.is_finite() {
        value.min(high).max(low)
    } else {
        low
    }
}

pub fn host_bundle_watch_initial_delay_ms(
    base_ms: u64,
    jitter_ratio: f64,
    random: impl FnOnce() -> f64,
) -> u64 {
    if base_ms == 0 || jitter_ratio <= 0.0 {
        return 0;
    }
    (clamp(random(), 0.0, 1.0) * base_ms as f64).round() as u64
}

pub fn host_bundle_watch_next_delay_ms(
    base_ms: u64,
    jitter_ratio: f64,
    random: impl FnOnce() -> f64,
) -> u64 {
    let ratio = clamp(jitter_ratio, 0.0, 1.0);
    if ratio == 0.0 {
        return base_ms;
    }
    let value = base_ms as f64 + (clamp(random(), 0.0, 1.0) * 2.0 - 1.0) * base_ms as f64 * ratio;
    value.max(0.0).round() as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostBundleStagePhase {
    Fetch,
    Stage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostBundleStageFailure {
    pub phase: HostBundleStagePhase,
    pub error: String,
}

pub fn fetch_stage_and_report_host_bundle(
    version: &str,
    from_version: Option<&str>,
    trigger: &str,
    state: &mut HostBundleUpdateState,
    load_bundle: impl FnOnce() -> Result<Vec<u8>, String>,
    stage: impl FnOnce(&[u8]) -> Result<(), String>,
    mut report_upgrade: impl FnMut(HashMap<String, String>),
    mut log: impl FnMut(&str, &str),
) -> Result<(), HostBundleStageFailure> {
    let bytes = match load_bundle() {
        Ok(bytes) => bytes,
        Err(error) => {
            report_failed_once(
                version,
                from_version,
                trigger,
                state,
                HostBundleStagePhase::Fetch,
                &error,
                &mut report_upgrade,
            );
            log(
                "warn",
                &format!("host bundle update ({trigger}): fetch failed (box unchanged): {error}"),
            );
            return Err(HostBundleStageFailure {
                phase: HostBundleStagePhase::Fetch,
                error,
            });
        }
    };
    if let Err(error) = stage(&bytes) {
        report_failed_once(
            version,
            from_version,
            trigger,
            state,
            HostBundleStagePhase::Stage,
            &error,
            &mut report_upgrade,
        );
        log(
            "warn",
            &format!("host bundle update ({trigger}): stage failed (box unchanged): {error}"),
        );
        return Err(HostBundleStageFailure {
            phase: HostBundleStagePhase::Stage,
            error,
        });
    }
    state.staged_version = Some(version.to_string());
    state.last_failed_version = None;
    log(
        "info",
        &format!("host bundle update ({trigger}): staged {version}; supervisor swaps when idle"),
    );
    Ok(())
}

fn report_failed_once(
    version: &str,
    from_version: Option<&str>,
    trigger: &str,
    state: &mut HostBundleUpdateState,
    phase: HostBundleStagePhase,
    error: &str,
    report: &mut impl FnMut(HashMap<String, String>),
) {
    if state.last_failed_version.as_deref() == Some(version) {
        return;
    }
    state.last_failed_version = Some(version.to_string());
    let mut metadata = HashMap::from([
        ("outcome".into(), "failed".into()),
        (
            "phase".into(),
            match phase {
                HostBundleStagePhase::Fetch => "fetch",
                HostBundleStagePhase::Stage => "stage",
            }
            .into(),
        ),
        ("mode".into(), "bundle".into()),
        ("trigger".into(), trigger.into()),
        ("to_version".into(), version.into()),
        ("error_class".into(), error.into()),
    ]);
    if let Some(from_version) = from_version {
        metadata.insert("from_version".into(), from_version.into());
    }
    report(metadata);
}
