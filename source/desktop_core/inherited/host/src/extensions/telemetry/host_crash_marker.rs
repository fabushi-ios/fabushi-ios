use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

pub const HOST_CRASH_EXIT_SIGNALS: [&str; 20] = [
    "none",
    "unknown",
    "other",
    "SIGABRT",
    "SIGALRM",
    "SIGBUS",
    "SIGFPE",
    "SIGHUP",
    "SIGILL",
    "SIGINT",
    "SIGKILL",
    "SIGPIPE",
    "SIGQUIT",
    "SIGSEGV",
    "SIGTERM",
    "SIGTRAP",
    "SIGUSR1",
    "SIGUSR2",
    "SIGXCPU",
    "SIGXFSZ",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostCrashErrorClass {
    SignalExit,
    NonzeroExit,
    UnexpectedCleanExit,
    UnobservedExit,
}

impl HostCrashErrorClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SignalExit => "signal_exit",
            Self::NonzeroExit => "nonzero_exit",
            Self::UnexpectedCleanExit => "unexpected_clean_exit",
            Self::UnobservedExit => "unobserved_exit",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HostCrashMarker {
    pub schema_version: u8,
    pub error_class: HostCrashErrorClass,
    pub exit_signal: String,
    pub crashed_at_ms: f64,
    pub started_at_ms: Option<f64>,
    pub uptime_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCrashMarkerRead {
    Present(String),
    Absent,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostCrashMarkerDelete {
    Deleted,
    Unavailable,
}

pub trait HostCrashMarkerStore {
    fn read(&self) -> HostCrashMarkerRead;
    fn delete(&self) -> HostCrashMarkerDelete;
}

#[derive(Debug, Clone)]
pub struct FileHostCrashMarkerStore {
    path: PathBuf,
}

impl FileHostCrashMarkerStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl HostCrashMarkerStore for FileHostCrashMarkerStore {
    fn read(&self) -> HostCrashMarkerRead {
        match fs::read_to_string(&self.path) {
            Ok(raw) => HostCrashMarkerRead::Present(raw),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => HostCrashMarkerRead::Absent,
            Err(_) => HostCrashMarkerRead::Unavailable,
        }
    }

    fn delete(&self) -> HostCrashMarkerDelete {
        match fs::remove_file(&self.path) {
            Ok(()) => HostCrashMarkerDelete::Deleted,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                HostCrashMarkerDelete::Deleted
            }
            Err(_) => HostCrashMarkerDelete::Unavailable,
        }
    }
}

pub fn is_non_negative_finite(value: &Value) -> bool {
    value
        .as_f64()
        .is_some_and(|number| number.is_finite() && number >= 0.0)
}

pub fn is_fatal_exit_signal(value: &Value) -> bool {
    let Some(signal) = value.as_str() else {
        return false;
    };
    signal != "none"
        && signal != "unknown"
        && HOST_CRASH_EXIT_SIGNALS
            .iter()
            .any(|candidate| *candidate == signal)
}

fn read_non_negative(value: &Map<String, Value>, key: &str) -> Option<f64> {
    value
        .get(key)
        .filter(|candidate| is_non_negative_finite(candidate))
        .and_then(Value::as_f64)
}

fn read_common_times(value: &Map<String, Value>) -> Option<(f64, f64, f64)> {
    Some((
        read_non_negative(value, "startedAtMs")?,
        read_non_negative(value, "crashedAtMs")?,
        read_non_negative(value, "uptimeMs")?,
    ))
}

pub fn parse_host_crash_marker(raw: &str) -> Option<HostCrashMarker> {
    let parsed: Value = serde_json::from_str(raw).ok()?;
    let value = parsed.as_object()?;
    if value.get("schemaVersion").and_then(Value::as_u64) != Some(1) {
        return None;
    }

    let error_class = value.get("errorClass")?.as_str()?;
    let exit_signal = value.get("exitSignal")?.as_str()?;

    match error_class {
        "signal_exit" if is_fatal_exit_signal(value.get("exitSignal")?) => {
            let (started_at_ms, crashed_at_ms, uptime_ms) = read_common_times(value)?;
            Some(HostCrashMarker {
                schema_version: 1,
                error_class: HostCrashErrorClass::SignalExit,
                exit_signal: exit_signal.to_owned(),
                crashed_at_ms,
                started_at_ms: Some(started_at_ms),
                uptime_ms: Some(uptime_ms),
            })
        }
        "nonzero_exit" | "unexpected_clean_exit" if exit_signal == "none" => {
            let (started_at_ms, crashed_at_ms, uptime_ms) = read_common_times(value)?;
            Some(HostCrashMarker {
                schema_version: 1,
                error_class: if error_class == "nonzero_exit" {
                    HostCrashErrorClass::NonzeroExit
                } else {
                    HostCrashErrorClass::UnexpectedCleanExit
                },
                exit_signal: exit_signal.to_owned(),
                crashed_at_ms,
                started_at_ms: Some(started_at_ms),
                uptime_ms: Some(uptime_ms),
            })
        }
        "unobserved_exit" if exit_signal == "unknown" => {
            let crashed_at_ms = read_non_negative(value, "crashedAtMs")?;
            let started_at_ms = match value.get("startedAtMs") {
                Some(candidate) => {
                    if !is_non_negative_finite(candidate) {
                        return None;
                    }
                    candidate.as_f64()
                }
                None => None,
            };
            let uptime_ms = match value.get("uptimeMs") {
                Some(candidate) => {
                    if !is_non_negative_finite(candidate) {
                        return None;
                    }
                    candidate.as_f64()
                }
                None => None,
            };
            Some(HostCrashMarker {
                schema_version: 1,
                error_class: HostCrashErrorClass::UnobservedExit,
                exit_signal: exit_signal.to_owned(),
                crashed_at_ms,
                started_at_ms,
                uptime_ms,
            })
        }
        _ => None,
    }
}

fn rounded_ms(value: f64) -> String {
    format!("{:.0}", value.round())
}

pub fn host_crash_marker_metadata(marker: &HostCrashMarker) -> BTreeMap<String, String> {
    let mut metadata = BTreeMap::new();
    metadata.insert("kind".into(), "process_exit".into());
    metadata.insert("error_class".into(), marker.error_class.as_str().into());
    metadata.insert("exit_signal".into(), marker.exit_signal.clone());
    if let Some(started_at_ms) = marker.started_at_ms {
        metadata.insert("started_at_ms".into(), rounded_ms(started_at_ms));
    }
    metadata.insert("crashed_at_ms".into(), rounded_ms(marker.crashed_at_ms));
    if let Some(uptime_ms) = marker.uptime_ms {
        metadata.insert("uptime_ms".into(), rounded_ms(uptime_ms));
    }
    metadata
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteIfUnchangedResult {
    Failed,
    Deleted,
    Changed,
}

pub fn delete_if_unchanged(
    store: &impl HostCrashMarkerStore,
    raw: &str,
) -> DeleteIfUnchangedResult {
    match store.read() {
        HostCrashMarkerRead::Unavailable => DeleteIfUnchangedResult::Failed,
        HostCrashMarkerRead::Absent => DeleteIfUnchangedResult::Deleted,
        HostCrashMarkerRead::Present(current) if current != raw => {
            DeleteIfUnchangedResult::Changed
        }
        HostCrashMarkerRead::Present(_) => match store.delete() {
            HostCrashMarkerDelete::Deleted => DeleteIfUnchangedResult::Deleted,
            HostCrashMarkerDelete::Unavailable => DeleteIfUnchangedResult::Failed,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardHostCrashMarkerResult {
    Deferred,
    Absent,
    DeleteDeferred,
    Pending,
    Delivered,
    ParseError,
}

pub fn forward_host_crash_marker_with(
    store: &impl HostCrashMarkerStore,
    mut was_forwarded: impl FnMut(&str) -> bool,
    mut mark_forwarded: impl FnMut(&str),
    mut emit: impl FnMut(&HostCrashMarker) -> bool,
) -> ForwardHostCrashMarkerResult {
    let raw = match store.read() {
        HostCrashMarkerRead::Unavailable => return ForwardHostCrashMarkerResult::Deferred,
        HostCrashMarkerRead::Absent => return ForwardHostCrashMarkerResult::Absent,
        HostCrashMarkerRead::Present(raw) => raw,
    };

    if was_forwarded(&raw) {
        return match delete_if_unchanged(store, &raw) {
            DeleteIfUnchangedResult::Failed => ForwardHostCrashMarkerResult::DeleteDeferred,
            DeleteIfUnchangedResult::Changed => ForwardHostCrashMarkerResult::Pending,
            DeleteIfUnchangedResult::Deleted => ForwardHostCrashMarkerResult::Delivered,
        };
    }

    let Some(marker) = parse_host_crash_marker(&raw) else {
        mark_forwarded(&raw);
        return match delete_if_unchanged(store, &raw) {
            DeleteIfUnchangedResult::Failed => ForwardHostCrashMarkerResult::DeleteDeferred,
            DeleteIfUnchangedResult::Changed => ForwardHostCrashMarkerResult::Pending,
            DeleteIfUnchangedResult::Deleted => ForwardHostCrashMarkerResult::ParseError,
        };
    };

    if !emit(&marker) {
        return ForwardHostCrashMarkerResult::Deferred;
    }

    mark_forwarded(&raw);
    match delete_if_unchanged(store, &raw) {
        DeleteIfUnchangedResult::Failed => ForwardHostCrashMarkerResult::DeleteDeferred,
        DeleteIfUnchangedResult::Changed => ForwardHostCrashMarkerResult::Pending,
        DeleteIfUnchangedResult::Deleted => ForwardHostCrashMarkerResult::Delivered,
    }
}
