pub const DNS_PROBE_TIMEOUT_MS: u64 = 2_000;
pub const DNS_PROBE_MIN_INTERVAL_MS: u64 = 60_000;
pub const GENERAL_CONTROL_HOSTNAME: &str = "api2.cursor.sh";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsProbeResult {
    Resolved,
    Timeout,
    NotFound,
    TemporaryFailure,
    Error,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsDiagnosis {
    ResolvedBeforeProbe,
    SystemPathFailure,
    IndependentPathFailure,
    EndpointFailure,
    CursorVmFailure,
    GeneralDnsFailure,
    Inconclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsCluster {
    Dev4,
    Us8,
}

impl DnsCluster {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dev4 => "dev4",
            Self::Us8 => "us8",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsTarget {
    pub endpoint_hostname: String,
    pub wildcard_hostname: String,
    pub cluster: DnsCluster,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayDnsDiagnostic {
    pub cluster: DnsCluster,
    pub trigger: DnsProbeResult,
    pub diagnosis: DnsDiagnosis,
    pub system_exact: DnsProbeResult,
    pub independent_exact: DnsProbeResult,
    pub independent_wildcard: DnsProbeResult,
    pub independent_general: DnsProbeResult,
}

pub fn dns_target_from_base_url(base_url: Option<&str>, wildcard_label: &str) -> Option<DnsTarget> {
    if wildcard_label.is_empty()
        || wildcard_label.len() > 63
        || !wildcard_label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return None;
    }
    let value = base_url?;
    let remainder = value.strip_prefix("https://")?;
    let authority = remainder.split('/').next()?;
    if authority.contains('@') || authority.contains(':') {
        return None;
    }
    let labels = authority.split('.').collect::<Vec<_>>();
    if labels.len() != 4
        || labels[0].is_empty()
        || labels[2] != "cursorvm"
        || labels[3] != "com"
    {
        return None;
    }
    let cluster = match labels[1] {
        "dev4" => DnsCluster::Dev4,
        "us8" => DnsCluster::Us8,
        _ => return None,
    };
    Some(DnsTarget {
        endpoint_hostname: authority.to_string(),
        wildcard_hostname: format!("{wildcard_label}.{}.cursorvm.com", cluster.as_str()),
        cluster,
    })
}

pub fn classify_probe_error(errno: Option<&str>, timed_out: bool) -> DnsProbeResult {
    if timed_out {
        return DnsProbeResult::Timeout;
    }
    match errno {
        Some("ENOTFOUND" | "ENODATA") => DnsProbeResult::NotFound,
        Some("EAI_AGAIN" | "ESERVFAIL" | "EREFUSED") => DnsProbeResult::TemporaryFailure,
        Some("ETIMEOUT" | "ETIMEDOUT") => DnsProbeResult::Timeout,
        _ => DnsProbeResult::Error,
    }
}

pub fn classify_dns_diagnosis(
    system_exact: DnsProbeResult,
    independent_exact: DnsProbeResult,
    independent_wildcard: DnsProbeResult,
    independent_general: DnsProbeResult,
) -> DnsDiagnosis {
    use DnsProbeResult::Resolved;

    if system_exact == Resolved && independent_exact == Resolved {
        DnsDiagnosis::ResolvedBeforeProbe
    } else if system_exact != Resolved && independent_exact == Resolved {
        DnsDiagnosis::SystemPathFailure
    } else if system_exact == Resolved && independent_exact != Resolved {
        DnsDiagnosis::IndependentPathFailure
    } else if independent_exact != Resolved && independent_wildcard == Resolved {
        DnsDiagnosis::EndpointFailure
    } else if independent_wildcard != Resolved && independent_general == Resolved {
        DnsDiagnosis::CursorVmFailure
    } else if independent_general != Resolved {
        DnsDiagnosis::GeneralDnsFailure
    } else {
        DnsDiagnosis::Inconclusive
    }
}

#[derive(Debug, Default)]
pub struct GatewayDnsDiagnosticReporter {
    last_probe_ms: Option<u64>,
    episode_active: bool,
    probe_in_flight: bool,
}

impl GatewayDnsDiagnosticReporter {
    pub fn should_probe(&self, now_ms: u64) -> bool {
        self.last_probe_ms
            .is_none_or(|last| now_ms.saturating_sub(last) >= DNS_PROBE_MIN_INTERVAL_MS)
    }

    pub fn record_probe(&mut self, now_ms: u64) {
        self.last_probe_ms = Some(now_ms);
    }

    pub fn episode_active(&self) -> bool {
        self.episode_active
    }

    pub fn probe_in_flight(&self) -> bool {
        self.probe_in_flight
    }

    pub fn observe_with_probes<SystemLookup, IndependentLookup>(
        &mut self,
        now_ms: u64,
        outcome: &str,
        cause_summary: Option<&str>,
        base_url: Option<&str>,
        wildcard_label: &str,
        mut system_lookup: SystemLookup,
        mut independent_lookup: IndependentLookup,
    ) -> Option<GatewayDnsDiagnostic>
    where
        SystemLookup: FnMut(&str) -> DnsProbeResult,
        IndependentLookup: FnMut(&str) -> DnsProbeResult,
    {
        if outcome == "ok" {
            self.episode_active = false;
            return None;
        }
        if outcome != "dns" || self.episode_active || self.probe_in_flight || !self.should_probe(now_ms) {
            return None;
        }
        let target = dns_target_from_base_url(base_url, wildcard_label)?;
        self.episode_active = true;
        self.probe_in_flight = true;
        self.record_probe(now_ms);

        let system_exact = system_lookup(&target.endpoint_hostname);
        let independent_exact = independent_lookup(&target.endpoint_hostname);
        let independent_wildcard = independent_lookup(&target.wildcard_hostname);
        let independent_general = independent_lookup(GENERAL_CONTROL_HOSTNAME);
        self.probe_in_flight = false;

        let trigger = match cause_summary {
            Some(detail) if detail.contains("ENOTFOUND") => DnsProbeResult::NotFound,
            Some(detail) if detail.contains("EAI_AGAIN") => DnsProbeResult::TemporaryFailure,
            _ => DnsProbeResult::Unknown,
        };
        Some(GatewayDnsDiagnostic {
            cluster: target.cluster,
            trigger,
            diagnosis: classify_dns_diagnosis(
                system_exact,
                independent_exact,
                independent_wildcard,
                independent_general,
            ),
            system_exact,
            independent_exact,
            independent_wildcard,
            independent_general,
        })
    }

    pub fn diagnose(
        &mut self,
        now_ms: u64,
        cluster: DnsCluster,
        trigger: DnsProbeResult,
        system_exact: DnsProbeResult,
        independent_exact: DnsProbeResult,
        independent_wildcard: DnsProbeResult,
        independent_general: DnsProbeResult,
    ) -> Option<GatewayDnsDiagnostic> {
        if !self.should_probe(now_ms) {
            return None;
        }
        self.record_probe(now_ms);
        Some(GatewayDnsDiagnostic {
            cluster,
            trigger,
            diagnosis: classify_dns_diagnosis(
                system_exact,
                independent_exact,
                independent_wildcard,
                independent_general,
            ),
            system_exact,
            independent_exact,
            independent_wildcard,
            independent_general,
        })
    }
}
