use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransferId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferState {
    Queued,
    Resolving,
    Transferring,
    Paused,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaDescriptor {
    pub id: String,
    pub file_name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub content_hash: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub thumbnail_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaTransfer {
    pub id: TransferId,
    pub direction: TransferDirection,
    pub media: MediaDescriptor,
    pub state: TransferState,
    pub local_path: Option<String>,
    pub remote_locator: Option<String>,
    pub chunk_size: u32,
    pub transferred_bytes: u64,
    pub verified_bytes: u64,
    pub retry_count: u32,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub error: Option<String>,
}

impl MediaTransfer {
    pub fn progress_percent(&self) -> u8 {
        if self.media.size_bytes == 0 {
            return if self.state == TransferState::Completed {
                100
            } else {
                0
            };
        }
        let percent = self
            .transferred_bytes
            .saturating_mul(100)
            .checked_div(self.media.size_bytes)
            .unwrap_or_default();
        percent.min(100) as u8
    }

    pub fn remaining_bytes(&self) -> u64 {
        self.media.size_bytes.saturating_sub(self.transferred_bytes)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaTransferQueue {
    pub transfers: BTreeMap<TransferId, MediaTransfer>,
}

impl MediaTransferQueue {
    pub fn enqueue(&mut self, transfer: MediaTransfer) -> Result<(), MediaError> {
        if transfer.chunk_size == 0 {
            return Err(MediaError::InvalidChunkSize);
        }
        if self.transfers.contains_key(&transfer.id) {
            return Err(MediaError::DuplicateTransfer(transfer.id));
        }
        self.transfers.insert(transfer.id.clone(), transfer);
        Ok(())
    }

    pub fn start(&mut self, id: &TransferId, now_ms: i64) -> Result<(), MediaError> {
        let transfer = self.require_mut(id)?;
        match transfer.state {
            TransferState::Queued | TransferState::Paused | TransferState::Failed => {
                transfer.state = TransferState::Transferring;
                transfer.updated_at_ms = now_ms;
                transfer.error = None;
                Ok(())
            }
            state => Err(MediaError::InvalidTransition {
                from: state,
                to: TransferState::Transferring,
            }),
        }
    }

    pub fn acknowledge_chunk(
        &mut self,
        id: &TransferId,
        offset: u64,
        length: u32,
        now_ms: i64,
    ) -> Result<(), MediaError> {
        let transfer = self.require_mut(id)?;
        if transfer.state != TransferState::Transferring {
            return Err(MediaError::InvalidTransition {
                from: transfer.state,
                to: TransferState::Transferring,
            });
        }
        if offset != transfer.transferred_bytes {
            return Err(MediaError::UnexpectedOffset {
                expected: transfer.transferred_bytes,
                actual: offset,
            });
        }
        transfer.transferred_bytes = transfer
            .transferred_bytes
            .saturating_add(u64::from(length))
            .min(transfer.media.size_bytes);
        transfer.updated_at_ms = now_ms;
        if transfer.transferred_bytes == transfer.media.size_bytes {
            transfer.state = TransferState::Verifying;
        }
        Ok(())
    }

    pub fn complete_verification(
        &mut self,
        id: &TransferId,
        verified_bytes: u64,
        remote_locator: Option<String>,
        now_ms: i64,
    ) -> Result<(), MediaError> {
        let transfer = self.require_mut(id)?;
        if transfer.state != TransferState::Verifying {
            return Err(MediaError::InvalidTransition {
                from: transfer.state,
                to: TransferState::Completed,
            });
        }
        if verified_bytes != transfer.media.size_bytes {
            return Err(MediaError::VerificationMismatch {
                expected: transfer.media.size_bytes,
                actual: verified_bytes,
            });
        }
        transfer.verified_bytes = verified_bytes;
        transfer.remote_locator = remote_locator.or_else(|| transfer.remote_locator.clone());
        transfer.state = TransferState::Completed;
        transfer.updated_at_ms = now_ms;
        Ok(())
    }

    pub fn pause(&mut self, id: &TransferId, now_ms: i64) -> Result<(), MediaError> {
        let transfer = self.require_mut(id)?;
        if transfer.state != TransferState::Transferring {
            return Err(MediaError::InvalidTransition {
                from: transfer.state,
                to: TransferState::Paused,
            });
        }
        transfer.state = TransferState::Paused;
        transfer.updated_at_ms = now_ms;
        Ok(())
    }

    pub fn fail(
        &mut self,
        id: &TransferId,
        error: impl Into<String>,
        now_ms: i64,
    ) -> Result<(), MediaError> {
        let transfer = self.require_mut(id)?;
        if matches!(
            transfer.state,
            TransferState::Completed | TransferState::Cancelled
        ) {
            return Err(MediaError::TerminalTransfer(id.clone()));
        }
        transfer.state = TransferState::Failed;
        transfer.retry_count = transfer.retry_count.saturating_add(1);
        transfer.error = Some(error.into());
        transfer.updated_at_ms = now_ms;
        Ok(())
    }

    pub fn cancel(&mut self, id: &TransferId, now_ms: i64) -> Result<(), MediaError> {
        let transfer = self.require_mut(id)?;
        if transfer.state == TransferState::Completed {
            return Err(MediaError::TerminalTransfer(id.clone()));
        }
        transfer.state = TransferState::Cancelled;
        transfer.updated_at_ms = now_ms;
        Ok(())
    }

    fn require_mut(&mut self, id: &TransferId) -> Result<&mut MediaTransfer, MediaError> {
        self.transfers
            .get_mut(id)
            .ok_or_else(|| MediaError::TransferNotFound(id.clone()))
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MediaError {
    #[error("media transfer {0:?} already exists")]
    DuplicateTransfer(TransferId),
    #[error("media transfer {0:?} was not found")]
    TransferNotFound(TransferId),
    #[error("media transfer chunk size must be greater than zero")]
    InvalidChunkSize,
    #[error("media transfer cannot transition from {from:?} to {to:?}")]
    InvalidTransition {
        from: TransferState,
        to: TransferState,
    },
    #[error("media transfer expected offset {expected}, got {actual}")]
    UnexpectedOffset { expected: u64, actual: u64 },
    #[error("media verification expected {expected} bytes, got {actual}")]
    VerificationMismatch { expected: u64, actual: u64 },
    #[error("media transfer {0:?} is already terminal")]
    TerminalTransfer(TransferId),
}

pub const MAX_DESKTOP_MEDIA_ATTACHMENT_BYTES: u64 = 32 * 1024 * 1024; // 32 MiB per attachment
pub const MAX_DESKTOP_MEDIA_BATCH_COUNT: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
#[serde(rename_all = "camelCase")]
pub enum DesktopMediaPreflightError {
    #[error("too many attachments: {count} exceeds maximum {max}")]
    TooManyAttachments { count: usize, max: usize },
    #[error("media attachment file name cannot be empty")]
    EmptyFileName,
    #[error("media attachment file name rejected due to path traversal or control characters")]
    PathTraversalRejected,
    #[error("media attachment cannot be zero bytes")]
    ZeroByteAttachment,
    #[error("media attachment size {size_bytes} bytes exceeds maximum {max_bytes} bytes")]
    AttachmentTooLarge { size_bytes: u64, max_bytes: u64 },
}

pub fn validate_desktop_media_attachment_meta(
    file_name: &str,
    size_bytes: u64,
    max_bytes: Option<u64>,
) -> Result<(), DesktopMediaPreflightError> {
    let trimmed = file_name.trim();
    if trimmed.is_empty() {
        return Err(DesktopMediaPreflightError::EmptyFileName);
    }
    if trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains("..")
        || trimmed.chars().any(|c| c.is_control())
    {
        return Err(DesktopMediaPreflightError::PathTraversalRejected);
    }
    if size_bytes == 0 {
        return Err(DesktopMediaPreflightError::ZeroByteAttachment);
    }
    let max = max_bytes.unwrap_or(MAX_DESKTOP_MEDIA_ATTACHMENT_BYTES);
    if size_bytes > max {
        return Err(DesktopMediaPreflightError::AttachmentTooLarge {
            size_bytes,
            max_bytes: max,
        });
    }
    Ok(())
}

pub fn validate_desktop_media_batch_count(
    count: usize,
    max_count: Option<usize>,
) -> Result<(), DesktopMediaPreflightError> {
    let max = max_count.unwrap_or(MAX_DESKTOP_MEDIA_BATCH_COUNT);
    if count > max {
        return Err(DesktopMediaPreflightError::TooManyAttachments { count, max });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_root_exports_pure_policy_types_and_functions() {
        use crate::{
            validate_desktop_media_attachment_meta, DesktopMediaPreflightError,
            MAX_DESKTOP_MEDIA_ATTACHMENT_BYTES, MAX_DESKTOP_MEDIA_BATCH_COUNT,
        };
        assert_eq!(MAX_DESKTOP_MEDIA_BATCH_COUNT, 6);
        assert_eq!(MAX_DESKTOP_MEDIA_ATTACHMENT_BYTES, 32 * 1024 * 1024);
        assert_eq!(
            validate_desktop_media_attachment_meta("valid.png", 100, None),
            Ok(())
        );
        let _ = DesktopMediaPreflightError::EmptyFileName;
    }

    #[test]
    fn validate_desktop_media_attachment_meta_normal_pass() {
        assert_eq!(
            validate_desktop_media_attachment_meta("photo.png", 1024, None),
            Ok(())
        );
        assert_eq!(
            validate_desktop_media_attachment_meta("document.pdf", MAX_DESKTOP_MEDIA_ATTACHMENT_BYTES, None),
            Ok(())
        );
        assert_eq!(
            validate_desktop_media_attachment_meta("   archive.tar.gz   ", 500, Some(1000)),
            Ok(())
        );
    }

    #[test]
    fn validate_desktop_media_attachment_meta_empty_file_name() {
        assert_eq!(
            validate_desktop_media_attachment_meta("", 1024, None),
            Err(DesktopMediaPreflightError::EmptyFileName)
        );
        assert_eq!(
            validate_desktop_media_attachment_meta("   ", 1024, None),
            Err(DesktopMediaPreflightError::EmptyFileName)
        );
        assert_eq!(
            validate_desktop_media_attachment_meta("\t\n\r  ", 1024, None),
            Err(DesktopMediaPreflightError::EmptyFileName)
        );
    }

    #[test]
    fn validate_desktop_media_attachment_meta_path_traversal_and_controls() {
        let rejected_cases = [
            "../secret.key",
            "dir/photo.png",
            "dir\\photo.png",
            "..",
            "something/..",
            "photo\0.png",
            "photo\n.png",
            "photo\r.png",
            "nested/dir/foo.jpg",
            "c:\\windows\\system32",
        ];
        for case in rejected_cases {
            assert_eq!(
                validate_desktop_media_attachment_meta(case, 1024, None),
                Err(DesktopMediaPreflightError::PathTraversalRejected),
                "Expected rejection for: {:?}",
                case
            );
        }
    }

    #[test]
    fn validate_desktop_media_attachment_meta_zero_byte() {
        assert_eq!(
            validate_desktop_media_attachment_meta("empty.png", 0, None),
            Err(DesktopMediaPreflightError::ZeroByteAttachment)
        );
    }

    #[test]
    fn validate_desktop_media_attachment_meta_attachment_too_large() {
        let oversized = MAX_DESKTOP_MEDIA_ATTACHMENT_BYTES + 1;
        assert_eq!(
            validate_desktop_media_attachment_meta("huge.mov", oversized, None),
            Err(DesktopMediaPreflightError::AttachmentTooLarge {
                size_bytes: oversized,
                max_bytes: MAX_DESKTOP_MEDIA_ATTACHMENT_BYTES,
            })
        );

        let custom_max = 5000;
        assert_eq!(
            validate_desktop_media_attachment_meta("custom.bin", 5001, Some(custom_max)),
            Err(DesktopMediaPreflightError::AttachmentTooLarge {
                size_bytes: 5001,
                max_bytes: custom_max,
            })
        );
    }

    #[test]
    fn validate_desktop_media_batch_count_bounds() {
        assert_eq!(validate_desktop_media_batch_count(0, None), Ok(()));
        assert_eq!(validate_desktop_media_batch_count(MAX_DESKTOP_MEDIA_BATCH_COUNT, None), Ok(()));
        assert_eq!(
            validate_desktop_media_batch_count(MAX_DESKTOP_MEDIA_BATCH_COUNT + 1, None),
            Err(DesktopMediaPreflightError::TooManyAttachments {
                count: MAX_DESKTOP_MEDIA_BATCH_COUNT + 1,
                max: MAX_DESKTOP_MEDIA_BATCH_COUNT,
            })
        );

        assert_eq!(
            validate_desktop_media_batch_count(4, Some(3)),
            Err(DesktopMediaPreflightError::TooManyAttachments {
                count: 4,
                max: 3,
            })
        );
    }

    #[test]
    fn preflight_error_serde_and_display() {
        let cases = [
            DesktopMediaPreflightError::TooManyAttachments { count: 10, max: 6 },
            DesktopMediaPreflightError::EmptyFileName,
            DesktopMediaPreflightError::PathTraversalRejected,
            DesktopMediaPreflightError::ZeroByteAttachment,
            DesktopMediaPreflightError::AttachmentTooLarge {
                size_bytes: 1000,
                max_bytes: 500,
            },
        ];
        for err in &cases {
            let json = serde_json::to_string(err).unwrap();
            let deserialized: DesktopMediaPreflightError = serde_json::from_str(&json).unwrap();
            assert_eq!(&deserialized, err);
            assert!(!err.to_string().is_empty());
        }

        let too_large = DesktopMediaPreflightError::AttachmentTooLarge {
            size_bytes: 1000,
            max_bytes: 500,
        };
        let json = serde_json::to_string(&too_large).unwrap();
        assert!(json.contains("attachmentTooLarge"));
        assert!(too_large.to_string().contains("1000"));
        assert!(too_large.to_string().contains("500"));
    }
}
