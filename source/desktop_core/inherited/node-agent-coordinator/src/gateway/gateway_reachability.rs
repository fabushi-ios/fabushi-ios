#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReachabilityOutcome {
    Reachable,
    NoStorage,
    BoxBlocked,
    AccessDenied,
    Refused,
    Dns,
    Timeout,
    Network,
    Http(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseUrlKind {
    Unknown,
    Loopback,
    PodProxy,
}

impl ReachabilityOutcome {
    pub fn retryable(self) -> bool {
        matches!(
            self,
            Self::Refused | Self::Dns | Self::Timeout | Self::Network | Self::Http(500..=599)
        )
    }

    pub fn transport_kind(self) -> &'static str {
        match self {
            Self::Reachable => "reachable",
            Self::NoStorage => "no_storage",
            Self::BoxBlocked => "box_blocked",
            Self::AccessDenied => "access_denied",
            Self::Refused => "refused",
            Self::Dns => "dns",
            Self::Timeout => "timeout",
            Self::Network => "network",
            Self::Http(500..=599) => "http_5xx",
            Self::Http(_) => "network",
        }
    }
}

pub fn outcome_for_http_status(status: u16) -> Option<ReachabilityOutcome> {
    if status >= 500 {
        Some(ReachabilityOutcome::Http(status))
    } else if matches!(status, 401 | 403) {
        Some(ReachabilityOutcome::AccessDenied)
    } else {
        None
    }
}

pub fn classify_system_error(code: Option<&str>, timed_out: bool, message: &str) -> ReachabilityOutcome {
    if message.contains("NO_STORAGE") {
        return ReachabilityOutcome::NoStorage;
    }
    if message.contains("BOX_BLOCKED") {
        return ReachabilityOutcome::BoxBlocked;
    }
    if message.contains("ACCESS_DENIED") {
        return ReachabilityOutcome::AccessDenied;
    }
    match code {
        Some("ECONNREFUSED") => ReachabilityOutcome::Refused,
        Some("ENOTFOUND" | "EAI_AGAIN") => ReachabilityOutcome::Dns,
        Some("ETIMEDOUT") => ReachabilityOutcome::Timeout,
        _ if timed_out => ReachabilityOutcome::Timeout,
        _ => ReachabilityOutcome::Network,
    }
}

pub fn classify_base_url_kind(base_url: Option<&str>) -> BaseUrlKind {
    let Some(value) = base_url.filter(|value| !value.is_empty()) else {
        return BaseUrlKind::Unknown;
    };
    let Some(after_scheme) = value.split_once("://").map(|(_, rest)| rest) else {
        return BaseUrlKind::Unknown;
    };
    let authority = after_scheme.split('/').next().unwrap_or_default();
    let host = if authority.starts_with('[') {
        authority.split(']').next().unwrap_or_default().trim_start_matches('[')
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    match host {
        "127.0.0.1" | "localhost" | "::1" => BaseUrlKind::Loopback,
        "" => BaseUrlKind::Unknown,
        _ => BaseUrlKind::PodProxy,
    }
}
