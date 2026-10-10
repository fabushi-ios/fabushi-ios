#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestPathModule {
    Posix,
    Win32,
}

/// Rust counterpart of Grok's getRequestPathModule helper.
///
/// iOS production requests resolve to POSIX. The Win32 branch remains part of
/// the package contract so imported request metadata behaves like the pinned
/// Grok implementation when it explicitly reports a win32 OS version.
pub fn get_request_path_module(os_version: Option<&str>) -> RequestPathModule {
    if os_version
        .map(|value| value.to_ascii_lowercase().contains("win32"))
        .unwrap_or(false)
    {
        RequestPathModule::Win32
    } else {
        RequestPathModule::Posix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_win32_only_for_explicit_win32_metadata() {
        assert_eq!(get_request_path_module(Some("win32")), RequestPathModule::Win32);
        assert_eq!(
            get_request_path_module(Some("Windows win32 10.0")),
            RequestPathModule::Win32
        );
        assert_eq!(
            get_request_path_module(Some("Darwin 25.0")),
            RequestPathModule::Posix
        );
        assert_eq!(get_request_path_module(None), RequestPathModule::Posix);
    }
}
