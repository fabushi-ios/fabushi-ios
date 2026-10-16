mod csnaps_protocol;
mod native_csnaps;
pub(crate) mod cursor_backend;
mod privacy_mode;
mod production;
mod codebase_snapshot_trigger;

pub use csnaps_protocol::{
    CsnapsErrorCode, CsnapsOperation, CsnapsOperationResult, CsnapsProtocolError,
    CsnapsResponse, CsnapsResponseDecoder, CsnapsServiceState, CsnapsTrackedCodebase,
    MAX_FRAME_BYTES, decode_frame_payloads, encode_csnaps_request, encode_frame,
    parse_csnaps_operation_result, parse_csnaps_response,
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
