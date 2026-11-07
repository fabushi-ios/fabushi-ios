use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_PROCESS_PATH_CHARS: usize = 4096;
const MAX_PROCESS_TEXT_CHARS: usize = 512;
const MAX_SESSION_SID_CHARS: usize = 256;

/// Persistable launch identity from the provider's process owner. This is a
/// receipt shape only; it is not proof that a PID is currently alive and it
/// never performs OS inspection or process cleanup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerProcessReceipt {
    pub pid: u32,
    pub start_time: String,
    pub executable_path: String,
    pub runtime_dir: String,
    pub state_dir: String,
    pub cli: String,
    #[serde(default)]
    pub session_id: Option<u32>,
    #[serde(default)]
    pub user_sid: Option<String>,
}

impl CloudWorkerProcessReceipt {
    pub fn validate(&self) -> Result<(), CloudWorkerProcessIdentityError> {
        if self.pid == 0
            || !valid_text(&self.start_time, MAX_PROCESS_TEXT_CHARS)
            || !valid_absolute_path(&self.executable_path)
            || !valid_absolute_path(&self.runtime_dir)
            || !valid_absolute_path(&self.state_dir)
            || !valid_text(&self.cli, MAX_PROCESS_TEXT_CHARS)
        {
            return Err(CloudWorkerProcessIdentityError::InvalidReceipt);
        }
        match (&self.session_id, &self.user_sid) {
            (None, None) => Ok(()),
            (Some(session_id), Some(user_sid))
                if *session_id > 0 && valid_text(user_sid, MAX_SESSION_SID_CHARS) => Ok(()),
            _ => Err(CloudWorkerProcessIdentityError::InvalidSessionIdentity),
        }
    }

    pub fn requires_interactive_session(&self) -> bool {
        self.session_id.is_some() || self.user_sid.is_some()
    }
}

fn valid_absolute_path(value: &str) -> bool {
    if !valid_text(value, MAX_PROCESS_PATH_CHARS) {
        return false;
    }
    let bytes = value.as_bytes();
    value.starts_with('/')
        || value.starts_with("\\\\")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
}

fn valid_text(value: &str, max_chars: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= max_chars
        && !value.chars().any(|character| character.is_control())
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CloudWorkerProcessIdentityError {
    #[error("cloud-worker process receipt is invalid")]
    InvalidReceipt,
    #[error("cloud-worker process session identity must be supplied as a valid pair")]
    InvalidSessionIdentity,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt() -> CloudWorkerProcessReceipt {
        CloudWorkerProcessReceipt {
            pid: 42,
            start_time: "Fri Oct 31 11:22:33 2026".into(),
            executable_path: "/opt/openclaw/node".into(),
            runtime_dir: "/var/lib/openclaw/runtime/lease-1".into(),
            state_dir: "/var/lib/openclaw/state/lease-1".into(),
            cli: "/opt/openclaw/bin/openclaw".into(),
            session_id: None,
            user_sid: None,
        }
    }

    #[test]
    fn validates_persisted_process_identity_without_claiming_liveness() {
        let receipt = receipt();
        assert_eq!(receipt.validate(), Ok(()));
        assert!(!receipt.requires_interactive_session());
    }

    #[test]
    fn interactive_receipts_require_a_complete_session_identity_pair() {
        let mut receipt = receipt();
        receipt.session_id = Some(3);
        assert_eq!(
            receipt.validate(),
            Err(CloudWorkerProcessIdentityError::InvalidSessionIdentity)
        );
        receipt.user_sid = Some("S-1-5-21-123".into());
        assert_eq!(receipt.validate(), Ok(()));
        assert!(receipt.requires_interactive_session());
    }

    #[test]
    fn rejects_relative_paths_and_zero_pid() {
        let mut receipt = receipt();
        receipt.runtime_dir = "relative/runtime".into();
        assert_eq!(
            receipt.validate(),
            Err(CloudWorkerProcessIdentityError::InvalidReceipt)
        );
        receipt.runtime_dir = "/var/lib/openclaw/runtime/lease-1".into();
        receipt.pid = 0;
        assert_eq!(
            receipt.validate(),
            Err(CloudWorkerProcessIdentityError::InvalidReceipt)
        );
    }

    #[test]
    fn accepts_windows_absolute_receipt_paths_on_any_host() {
        let mut receipt = receipt();
        receipt.executable_path = r"C:\OpenClaw\node.exe".into();
        receipt.runtime_dir = r"C:\OpenClaw\runtime\lease-1".into();
        receipt.state_dir = r"C:\OpenClaw\state\lease-1".into();
        assert_eq!(receipt.validate(), Ok(()));
    }
}
