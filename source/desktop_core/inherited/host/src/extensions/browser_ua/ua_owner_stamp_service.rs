use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use serde_json::Value;

use crate::sha256::sha256_hex;

pub const UA_OWNER_STAMP_PATH: &str = "/tmp/sand-ua-user";
pub const UA_OWNER_STAMP_LENGTH: usize = 16;

pub fn ua_owner_stamp_for_access_token(access_token: &str) -> Option<String> {
    let mut segments = access_token.split('.');
    let _header = segments.next()?;
    let payload = segments.next()?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| URL_SAFE.decode(payload))
        .ok()?;
    let value: Value = serde_json::from_slice(&decoded).ok()?;
    let subject = value.get("sub")?.as_str()?;
    if subject.is_empty() {
        return None;
    }
    Some(sha256_hex(subject.as_bytes())[..UA_OWNER_STAMP_LENGTH].to_string())
}

pub struct UaOwnerStampWriter<Log> {
    path: PathBuf,
    log: Log,
    last_written: Option<String>,
}

pub fn create_ua_owner_stamp_writer<Log>(
    path: Option<PathBuf>,
    log: Log,
) -> UaOwnerStampWriter<Log>
where
    Log: Fn(&str),
{
    UaOwnerStampWriter {
        path: path.unwrap_or_else(|| PathBuf::from(UA_OWNER_STAMP_PATH)),
        log,
        last_written: None,
    }
}

impl<Log> UaOwnerStampWriter<Log>
where
    Log: Fn(&str),
{
    pub fn write(&mut self, access_token: Option<&str>) {
        let Some(access_token) = access_token else {
            return;
        };
        let Some(stamp) = ua_owner_stamp_for_access_token(access_token) else {
            return;
        };
        if self.last_written.as_deref() == Some(stamp.as_str()) {
            return;
        }

        if let Err(error) = write_stamp_atomically(&self.path, &stamp) {
            (self.log)(&format!("ua-owner stamp write failed: {error}"));
            return;
        }
        self.last_written = Some(stamp);
    }

    pub fn last_written(&self) -> Option<&str> {
        self.last_written.as_deref()
    }
}

fn write_stamp_atomically(path: &Path, stamp: &str) -> io::Result<()> {
    let temp_path = PathBuf::from(format!("{}.tmp", path.to_string_lossy()));
    fs::write(&temp_path, format!("{stamp}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o644))?;
    }
    fs::rename(temp_path, path)
}
