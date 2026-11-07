use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const UA_TOKEN_DISABLED_MARKER_PATH: &str = "/tmp/sand-ua-token-disabled";

pub struct UaTokenKillSwitchReconciler<Enabled, Log> {
    path: PathBuf,
    is_kill_switch_enabled: Enabled,
    log: Log,
    last_applied: Option<bool>,
}

pub fn create_ua_token_kill_switch_reconciler<Enabled, Log>(
    path: Option<PathBuf>,
    is_kill_switch_enabled: Enabled,
    log: Log,
) -> UaTokenKillSwitchReconciler<Enabled, Log>
where
    Enabled: Fn() -> bool,
    Log: Fn(&str),
{
    UaTokenKillSwitchReconciler {
        path: path.unwrap_or_else(|| PathBuf::from(UA_TOKEN_DISABLED_MARKER_PATH)),
        is_kill_switch_enabled,
        log,
        last_applied: None,
    }
}

impl<Enabled, Log> UaTokenKillSwitchReconciler<Enabled, Log>
where
    Enabled: Fn() -> bool,
    Log: Fn(&str),
{
    pub fn reconcile(&mut self) {
        let disabled = (self.is_kill_switch_enabled)();
        if self.last_applied == Some(disabled) {
            return;
        }

        let result = if disabled {
            write_disabled_marker(&self.path)
        } else {
            remove_marker_if_present(&self.path)
        };

        match result {
            Ok(()) => self.last_applied = Some(disabled),
            Err(error) => (self.log)(&format!(
                "ua-token kill-switch marker update failed: {error}"
            )),
        }
    }

    pub fn last_applied(&self) -> Option<bool> {
        self.last_applied
    }
}

fn write_disabled_marker(path: &Path) -> io::Result<()> {
    fs::write(path, b"1\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o644))?;
    }
    Ok(())
}

fn remove_marker_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
