mod csnaps_protocol;
pub(crate) mod cursor_backend;
mod privacy_mode;
mod production;
mod codebase_snapshot_trigger;

pub use csnaps_protocol::{
    CsnapsProtocolError, CsnapsResponseTracker, MAX_FRAME_BYTES, decode_frame_payloads,
    encode_frame,
};
pub use privacy_mode::{
    CodebaseTelemetryPrivacyMode, PrivacyLookupError, PrivacyMode, TelemetryAuth,
};
pub use production::{
    CodebaseTelemetryHostBridge, MobileCodebaseTelemetryOwner,
    CODEBASE_TELEMETRY_FEATURE_GATE,
};
pub use codebase_snapshot_trigger::{
    CodebaseSnapshotTrigger, SnapshotReason, SnapshotReasonType,
};
