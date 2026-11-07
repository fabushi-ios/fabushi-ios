use mahayana_host_runtime::extensions::telemetry::journal_outcome_telemetry::{
    JournalOutcomeReport, journal_outcome_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::memory_synthesis_telemetry::{
    MemorySynthesisReport, memory_synthesis_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::sand_error_tags::{
    SandErrorValue, sand_error_tags,
};
use mahayana_host_runtime::extensions::telemetry::webauthn_proxy_telemetry::{
    WebAuthnFailureCause, WebAuthnProxyReport, cause_error, webauthn_proxy_telemetry,
};

#[test]
fn sand_error_tags_match_the_frozen_registry_boundary() {
    let unknown = sand_error_tags(
        &SandErrorValue::new("PRIVATE-ERROR")
            .with_string("secret", "must-not-ship"),
    );
    assert_eq!(unknown["error_code"], "SAND-E0001");
    assert_eq!(unknown["error_domain"], "registry");
    assert_eq!(unknown["error_retryable"], "false");
    assert_eq!(unknown.len(), 3);

    let webauthn = sand_error_tags(
        &SandErrorValue::new("SAND-E0211")
            .with_string("domError", "NotAllowedError")
            .with_string("signErrorClass", "pin_blocked")
            .with_string("unregisteredPayload", "discard-me"),
    );
    assert_eq!(webauthn["error_code"], "SAND-E0211");
    assert_eq!(webauthn["error_domain"], "auth");
    assert_eq!(webauthn["error_retryable"], "true");
    assert_eq!(webauthn["dom_error"], "NotAllowedError");
    assert_eq!(webauthn["sign_error_class"], "pin_blocked");
    assert!(!webauthn.contains_key("unregistered_payload"));

    let bounded = sand_error_tags(
        &SandErrorValue::new("SAND-E0722")
            .with_string("tail", "contains spaces and is therefore rejected")
            .with_string("errno", "EIO"),
    );
    assert!(!bounded.contains_key("tail"));
    assert_eq!(bounded["errno"], "EIO");
}

#[test]
fn memory_and_journal_telemetry_share_the_registered_error_tags() {
    let shed = memory_synthesis_telemetry(&MemorySynthesisReport::Shed {
        cause: SandErrorValue::new("SAND-E0412"),
        item_count: 7,
    });
    assert_eq!(shed.level, Some("warn"));
    assert_eq!(shed.event, Some("sand.memory.synthesis"));
    assert_eq!(shed.metadata["outcome"], "shed");
    assert_eq!(shed.metadata["error_code"], "SAND-E0412");
    assert_eq!(shed.metadata["error_domain"], "agent");
    assert_eq!(shed.metadata["error_retryable"], "false");

    let ok = memory_synthesis_telemetry(&MemorySynthesisReport::Ok {
        duration_ms: 12.6,
        item_count: 2,
    });
    assert_eq!(ok.level, Some("info"));
    assert_eq!(ok.metadata["duration_ms"], "13");
    assert!(!ok.metadata.contains_key("error_code"));

    let rebuilt = journal_outcome_telemetry(&JournalOutcomeReport {
        outcome: "recovered".into(),
        op: "open".into(),
        conversation_id: "conversation-1".into(),
        entry_count: Some(4),
        bytes: Some(1024),
        duration_ms: 8.6,
        cause: Some(
            SandErrorValue::new("SAND-E0722")
                .with_string("tail", "torn")
                .with_string("errno", "EIO"),
        ),
    });
    assert_eq!(rebuilt.level, Some("warn"));
    assert_eq!(rebuilt.event, Some("sand.journal.outcome"));
    assert_eq!(rebuilt.metadata["tail"], "torn");
    assert_eq!(rebuilt.metadata["errno"], "EIO");
    assert_eq!(rebuilt.metadata["duration_ms"], "9");

    let missing = journal_outcome_telemetry(&JournalOutcomeReport {
        outcome: "recovered".into(),
        op: "open".into(),
        conversation_id: "conversation-1".into(),
        entry_count: None,
        bytes: None,
        duration_ms: 1.0,
        cause: Some(SandErrorValue::new("SAND-E0722").with_string("tail", "missing")),
    });
    assert_eq!(missing.level, Some("info"));

    let failed = journal_outcome_telemetry(&JournalOutcomeReport {
        outcome: "failed".into(),
        op: "append".into(),
        conversation_id: "conversation-1".into(),
        entry_count: None,
        bytes: None,
        duration_ms: 1.0,
        cause: Some(SandErrorValue::new("SAND-E0720").with_string("errno", "EIO")),
    });
    assert_eq!(failed.level, Some("error"));
}

#[test]
fn webauthn_proxy_telemetry_bounds_raw_desktop_errors_before_shipping() {
    let sign_error = cause_error(
        WebAuthnFailureCause::SignFailed,
        Some("VendorSpecificException"),
        Some("vendor_private_code"),
    );
    let tags = sand_error_tags(&sign_error);
    assert_eq!(tags["error_code"], "SAND-E0211");
    assert_eq!(tags["dom_error"], "OtherError");
    assert_eq!(tags["sign_error_class"], "other");

    let report = webauthn_proxy_telemetry(&WebAuthnProxyReport {
        outcome: "failed".into(),
        stage: "sign".into(),
        origin_class: "https".into(),
        ceremony_kind: "get".into(),
        request_id: "request-7".into(),
        elapsed_ms: 20.6,
        provider_count: Some(2),
        live_provider_count: Some(1),
        cause: Some(WebAuthnFailureCause::DesktopFailed),
        raw_dom_error_name: Some("AbortError".into()),
        raw_sign_error_class: Some("cancelled_or_timeout".into()),
    });
    assert_eq!(report.level, Some("warn"));
    assert_eq!(report.event, Some("sand.webauthn_proxy"));
    assert_eq!(report.metadata["elapsed_ms"], "21");
    assert_eq!(report.metadata["error_code"], "SAND-E0212");
    assert_eq!(report.metadata["dom_error"], "AbortError");
    assert_eq!(report.metadata["sign_error_class"], "cancelled_or_timeout");
}
