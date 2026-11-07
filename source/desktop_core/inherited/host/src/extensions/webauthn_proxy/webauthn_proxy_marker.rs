use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

pub const SAND_WEBAUTHN_PROXY_MARKER_PATH: &str = "/home/box/.sand-webauthn-proxy-enabled";
pub const BOX_CHROME_POLICY_COMMAND: &str = "/usr/local/bin/box-chrome-policy";

fn write_empty_marker(path: &Path) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o644);
    options.open(path).map(drop)
}

pub fn apply_web_authn_proxy_marker(enabled: bool) -> io::Result<&'static str> {
    apply_web_authn_proxy_marker_at(
        enabled,
        PathBuf::from(SAND_WEBAUTHN_PROXY_MARKER_PATH),
        PathBuf::from(BOX_CHROME_POLICY_COMMAND),
    )
}

pub fn apply_web_authn_proxy_marker_at(
    enabled: bool,
    marker_path: impl AsRef<Path>,
    policy_command: impl AsRef<Path>,
) -> io::Result<&'static str> {
    let marker_path = marker_path.as_ref();
    let Some(parent) = marker_path.parent() else {
        return Ok("not-a-box");
    };
    if !parent.exists() {
        return Ok("not-a-box");
    }

    let present = marker_path.exists();
    if present == enabled {
        return Ok("unchanged");
    }

    if enabled {
        write_empty_marker(marker_path)?;
    } else {
        fs::remove_file(marker_path)?;
    }

    let policy_command = policy_command.as_ref();
    if policy_command.exists() {
        let _ = Command::new(policy_command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    Ok("applied")
}
