use std::collections::BTreeMap;

use super::box_store_transfer::CategoryTransferSummary;
use super::store_db_bundle_capture::{StoreDbCaptureFailurePhase, StoreDbCaptureTrace};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreDbCaptureOutcome {
    Error,
    Oversize,
    Uploaded,
    Skipped,
    Unchanged,
}

impl StoreDbCaptureOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Oversize => "oversize",
            Self::Uploaded => "uploaded",
            Self::Skipped => "skipped",
            Self::Unchanged => "unchanged",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreDbSweepResult {
    pub summary: CategoryTransferSummary,
    pub complete: bool,
    pub agent_count: usize,
    pub captured_this_sweep: Vec<String>,
    pub capture_trace: StoreDbCaptureTrace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreDbSnapshotResult {
    pub outcome: StoreDbCaptureOutcome,
    pub summary: CategoryTransferSummary,
    pub capture_trace: StoreDbCaptureTrace,
    pub is_committed: bool,
    pub store_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxStoreDbCaptureTelemetrySummary {
    pub outcome: StoreDbCaptureOutcome,
    pub trigger: &'static str,
    pub failure_phase: Option<StoreDbCaptureFailurePhase>,
    pub is_committed: bool,
    pub agent_count: usize,
    pub files_scanned: usize,
    pub files_uploaded: usize,
    pub bytes: u64,
    pub duration_ms: u64,
    pub queue_duration_ms: u64,
    pub capture_duration_ms: u64,
    pub blob_upload_duration_ms: u64,
    pub manifest_commit_duration_ms: u64,
    pub store_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxStoreDbCaptureTelemetry {
    pub level: &'static str,
    pub metadata: BTreeMap<String, String>,
}

pub fn aggregate_store_db_sweep_outcome(
    failures: usize,
    oversize: usize,
    files_uploaded: usize,
    files_scanned: usize,
) -> StoreDbCaptureOutcome {
    if failures > 0 {
        StoreDbCaptureOutcome::Error
    } else if oversize > 0 {
        StoreDbCaptureOutcome::Oversize
    } else if files_uploaded > 0 {
        StoreDbCaptureOutcome::Uploaded
    } else if files_scanned == 0 {
        StoreDbCaptureOutcome::Skipped
    } else {
        StoreDbCaptureOutcome::Unchanged
    }
}

pub fn box_store_db_capture_telemetry_level(outcome: StoreDbCaptureOutcome) -> &'static str {
    match outcome {
        StoreDbCaptureOutcome::Error | StoreDbCaptureOutcome::Skipped => "warn",
        StoreDbCaptureOutcome::Oversize
        | StoreDbCaptureOutcome::Uploaded
        | StoreDbCaptureOutcome::Unchanged => "info",
    }
}

pub fn box_store_db_capture_telemetry(
    summary: &BoxStoreDbCaptureTelemetrySummary,
) -> BoxStoreDbCaptureTelemetry {
    let mut metadata = BTreeMap::from([
        ("outcome".into(), summary.outcome.as_str().into()),
        ("trigger".into(), summary.trigger.into()),
        ("committed".into(), summary.is_committed.to_string()),
        ("agent_count".into(), summary.agent_count.to_string()),
        ("files_scanned".into(), summary.files_scanned.to_string()),
        ("files_uploaded".into(), summary.files_uploaded.to_string()),
        ("bytes".into(), summary.bytes.to_string()),
        ("duration_ms".into(), summary.duration_ms.to_string()),
        ("queue_duration_ms".into(), summary.queue_duration_ms.to_string()),
        ("capture_duration_ms".into(), summary.capture_duration_ms.to_string()),
        (
            "blob_upload_duration_ms".into(),
            summary.blob_upload_duration_ms.to_string(),
        ),
        (
            "manifest_commit_duration_ms".into(),
            summary.manifest_commit_duration_ms.to_string(),
        ),
    ]);
    if let Some(failure_phase) = summary.failure_phase {
        metadata.insert(
            "failure_phase".into(),
            match failure_phase {
                StoreDbCaptureFailurePhase::Capture => "capture",
                StoreDbCaptureFailurePhase::BlobUpload => "blob_upload",
                StoreDbCaptureFailurePhase::ManifestCommit => "manifest_commit",
            }
            .into(),
        );
    }
    if let Some(store_id) = summary.store_id.as_ref() {
        metadata.insert("store_id".into(), store_id.clone());
    }
    BoxStoreDbCaptureTelemetry {
        level: box_store_db_capture_telemetry_level(summary.outcome),
        metadata,
    }
}
