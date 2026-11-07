use std::cell::{Cell, RefCell};

use mahayana_host_runtime::extensions::telemetry::desktop_health_forwarder::{
    DesktopHealthForwardResult, DesktopHealthOverall, MAX_COMPONENT_RESTARTS,
    compute_desktop_health_metadata, decide_desktop_health_forward, desktop_health_overall,
    forward_desktop_health_with, normalize_desktop_component_name, normalize_desktop_down_reason,
    parse_desktop_health_snapshot,
};
use serde_json::json;

#[test]
fn normalizes_component_names_down_reasons_and_counts() {
    assert_eq!(
        normalize_desktop_component_name(&json!("d1/xvfb")).as_deref(),
        Some("primary/xvfb")
    );
    assert_eq!(
        normalize_desktop_component_name(&json!("d2/x11vnc")).as_deref(),
        Some("fork/x11vnc")
    );
    assert_eq!(
        normalize_desktop_component_name(&json!("shared/egress-proxy")).as_deref(),
        Some("shared/egress-proxy")
    );
    assert!(normalize_desktop_component_name(&json!("d0/xvfb")).is_none());
    assert!(normalize_desktop_component_name(&json!("d1/unknown")).is_none());

    assert_eq!(
        normalize_desktop_down_reason(&json!("signal-SIGKILL")).as_deref(),
        Some("signal")
    );
    assert_eq!(
        normalize_desktop_down_reason(&json!("exit--9")).as_deref(),
        Some("exit")
    );
    assert_eq!(
        normalize_desktop_down_reason(&json!("something-new")).as_deref(),
        Some("unknown")
    );
}

#[test]
fn parses_merges_and_summarizes_duplicate_components() {
    let snapshot = parse_desktop_health_snapshot(
        r#"{
          "updatedAtMs": 100,
          "revision": 7,
          "supervisionEnabled": true,
          "components": [
            {"name":"d1/xvfb","up":true,"crashloop":false,"restartsInWindow":2},
            {"name":"d1/xvfb","up":false,"crashloop":true,"restartsInWindow":99999,"downReason":"oom"},
            {"name":"d2/x11vnc","up":true,"crashloop":false,"restartsInWindow":1},
            {"name":"bad/component","up":false,"restartsInWindow":5}
          ]
        }"#,
    )
    .unwrap();

    assert_eq!(snapshot.total, 2);
    assert_eq!(snapshot.up, 1);
    assert_eq!(snapshot.down, 1);
    assert_eq!(snapshot.crashlooping, 1);
    assert_eq!(
        snapshot.components[0].restarts_in_window,
        MAX_COMPONENT_RESTARTS
    );
    assert_eq!(snapshot.components[0].down_reason.as_deref(), Some("oom"));
    assert_eq!(desktop_health_overall(&snapshot), DesktopHealthOverall::Crashloop);

    let metadata = compute_desktop_health_metadata(&snapshot);
    assert_eq!(metadata.get("overall").map(String::as_str), Some("crashloop"));
    assert_eq!(
        metadata.get("down_detail").map(String::as_str),
        Some("primary/xvfb(crashloop)")
    );
    assert_eq!(
        metadata.get("down_reason").map(String::as_str),
        Some("primary/xvfb=oom")
    );
}

#[test]
fn rejects_invalid_top_level_shape_and_unsafe_integer_metadata() {
    assert!(parse_desktop_health_snapshot("[]").is_none());
    assert!(parse_desktop_health_snapshot(r#"{"revision":-1,"updatedAtMs":1,"components":[]}"#).is_none());
    assert!(parse_desktop_health_snapshot(r#"{"revision":1,"updatedAtMs":1}"#).is_none());
}

#[test]
fn forwarding_uses_revision_then_heartbeat() {
    assert!(decide_desktop_health_forward(None, None, 1, 10, 100));
    assert!(decide_desktop_health_forward(Some(1), Some(10), 2, 20, 100));
    assert!(!decide_desktop_health_forward(Some(1), Some(10), 1, 50, 100));
    assert!(decide_desktop_health_forward(Some(1), Some(10), 1, 110, 100));
    assert!(!decide_desktop_health_forward(Some(1), Some(200), 1, 100, 50));
}

#[test]
fn forwarding_preserves_absent_parse_skip_emit_and_last_state() {
    let emitted = RefCell::new(Vec::new());
    let revision = Cell::new(0);
    let at_ms = Cell::new(0);
    assert_eq!(
        forward_desktop_health_with(
            None,
            None,
            None,
            100,
            1_000,
            |_, _| {},
            |_, _| {},
        ),
        DesktopHealthForwardResult::Absent
    );
    assert_eq!(
        forward_desktop_health_with(
            Some("bad"),
            None,
            None,
            100,
            1_000,
            |_, _| {},
            |_, _| {},
        ),
        DesktopHealthForwardResult::ParseError
    );

    let raw = r#"{"updatedAtMs":1,"revision":3,"supervisionEnabled":true,"components":[]}"#;
    assert_eq!(
        forward_desktop_health_with(
            Some(raw),
            Some(3),
            Some(100),
            200,
            1_000,
            |_, _| {},
            |_, _| {},
        ),
        DesktopHealthForwardResult::Skipped
    );
    assert_eq!(
        forward_desktop_health_with(
            Some(raw),
            None,
            None,
            200,
            1_000,
            |level, metadata| emitted.borrow_mut().push((level, metadata.clone())),
            |next_revision, next_at| {
                revision.set(next_revision);
                at_ms.set(next_at);
            },
        ),
        DesktopHealthForwardResult::Emitted
    );
    assert_eq!(revision.get(), 3);
    assert_eq!(at_ms.get(), 200);
    assert_eq!(emitted.borrow()[0].0, "info");
}
