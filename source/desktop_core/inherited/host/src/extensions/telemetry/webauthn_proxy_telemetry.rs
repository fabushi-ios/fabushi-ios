use std::collections::BTreeMap;

use super::HostTelemetryProjection;
use super::sand_error_tags::{SandErrorValue, sand_error_tags};

pub const WEBAUTHN_PROXY_EVENT: &str = "sand.webauthn_proxy";
pub const KNOWN_DOM_ERROR_NAMES: &[&str] = &[
    "NotAllowedError",
    "InvalidStateError",
    "NotSupportedError",
    "SecurityError",
    "AbortError",
    "ConstraintError",
    "DataError",
    "TimeoutError",
    "NetworkError",
    "OperationError",
    "UnknownError",
];
pub const WEBAUTHN_SIGN_ERROR_CLASSES: &[&str] = &[
    "no_credentials",
    "pin_not_set",
    "pin_blocked",
    "pin_other",
    "cancelled_or_timeout",
    "unsupported_option",
    "hid_error",
    "ambiguous_credential",
    "bad_options",
    "create_unsupported",
    "platform_api",
    "signer_other",
    "helper_spawn_failed",
    "helper_no_result",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebAuthnFailureCause {
    NoProvider,
    ProviderStale,
    DispatchFailed,
    Timeout,
    ConsentDeclined,
    SignFailed,
    DesktopFailed,
}

fn branded(value: Option<&str>, known: &[&str], fallback: &str) -> Option<String> {
    value.map(|value| {
        if known.contains(&value) {
            value.to_string()
        } else {
            fallback.to_string()
        }
    })
}

pub fn cause_error(
    cause: WebAuthnFailureCause,
    raw_dom_error_name: Option<&str>,
    raw_sign_error_class: Option<&str>,
) -> SandErrorValue {
    let code = match cause {
        WebAuthnFailureCause::NoProvider => "SAND-E0207",
        WebAuthnFailureCause::ProviderStale => "SAND-E0208",
        WebAuthnFailureCause::DispatchFailed => "SAND-E0213",
        WebAuthnFailureCause::Timeout => "SAND-E0209",
        WebAuthnFailureCause::ConsentDeclined => "SAND-E0210",
        WebAuthnFailureCause::SignFailed => "SAND-E0211",
        WebAuthnFailureCause::DesktopFailed => "SAND-E0212",
    };
    let mut error = SandErrorValue::new(code);
    if matches!(
        cause,
        WebAuthnFailureCause::SignFailed | WebAuthnFailureCause::DesktopFailed
    ) {
        if let Some(value) = branded(raw_dom_error_name, KNOWN_DOM_ERROR_NAMES, "OtherError") {
            error = error.with_string("domError", value);
        }
        if let Some(value) = branded(raw_sign_error_class, WEBAUTHN_SIGN_ERROR_CLASSES, "other") {
            error = error.with_string("signErrorClass", value);
        }
    }
    error
}

#[derive(Debug, Clone, PartialEq)]
pub struct WebAuthnProxyReport {
    pub outcome: String,
    pub stage: String,
    pub origin_class: String,
    pub ceremony_kind: String,
    pub request_id: String,
    pub elapsed_ms: f64,
    pub provider_count: Option<i64>,
    pub live_provider_count: Option<i64>,
    pub cause: Option<WebAuthnFailureCause>,
    pub raw_dom_error_name: Option<String>,
    pub raw_sign_error_class: Option<String>,
}

pub fn webauthn_proxy_telemetry(report: &WebAuthnProxyReport) -> HostTelemetryProjection {
    let mut metadata = BTreeMap::from([
        ("stage".into(), report.stage.clone()),
        ("outcome".into(), report.outcome.clone()),
        ("origin_class".into(), report.origin_class.clone()),
        ("ceremony_kind".into(), report.ceremony_kind.clone()),
        ("request_id".into(), report.request_id.clone()),
        ("elapsed_ms".into(), report.elapsed_ms.round().to_string()),
    ]);
    if let Some(value) = report.provider_count {
        metadata.insert("provider_count".into(), value.to_string());
    }
    if let Some(value) = report.live_provider_count {
        metadata.insert("live_provider_count".into(), value.to_string());
    }
    if let Some(cause) = report.cause {
        metadata.extend(sand_error_tags(&cause_error(
            cause,
            report.raw_dom_error_name.as_deref(),
            report.raw_sign_error_class.as_deref(),
        )));
    }
    HostTelemetryProjection {
        level: Some(
            if matches!(report.outcome.as_str(), "failed" | "timeout") {
                "warn"
            } else {
                "info"
            },
        ),
        event: Some(WEBAUTHN_PROXY_EVENT),
        metadata,
    }
}
