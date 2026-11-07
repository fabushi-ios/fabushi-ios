use std::collections::BTreeMap;

use mahayana_host_runtime::extensions::action_audit::action_audit_backend::{
    DASHBOARD_RECORD_SAND_AUDIT_EVENTS_PATH, encode_audit_event,
    encode_record_sand_audit_events_request,
};
use mahayana_host_runtime::extensions::action_audit::action_audit_service::{
    AuditAction, AuditEvent,
};

fn read_varint(bytes: &[u8], cursor: &mut usize) -> u64 {
    let mut out = 0_u64;
    let mut shift = 0_u32;
    loop {
        let byte = bytes[*cursor];
        *cursor += 1;
        out |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return out;
        }
        shift += 7;
    }
}

#[test]
fn dashboard_endpoint_and_request_encode_repeated_audit_events() {
    assert_eq!(
        DASHBOARD_RECORD_SAND_AUDIT_EVENTS_PATH,
        "/aiserver.v1.DashboardService/RecordSandAuditEvents"
    );
    let event = AuditEvent {
        event_id: "event-1".into(),
        occurred_at_ms: 1234,
        agent_id: "agent-1".into(),
        turn_id: "turn-1".into(),
        box_id: "box-1".into(),
        action: AuditAction::McpToolCall {
            tool_call_id: "call-1".into(),
            server_identifier: "server-1".into(),
            server_name: Some("Server".into()),
            tool_name: "tool".into(),
            transport: "stdio".into(),
            status: "ok".into(),
            duration_ms: 42.4,
        },
    };
    let encoded_event = encode_audit_event(&event);
    let request = encode_record_sand_audit_events_request(&[event.clone(), event]);
    let mut cursor = 0;
    assert_eq!(read_varint(&request, &mut cursor), 0x0a);
    let first_len = read_varint(&request, &mut cursor) as usize;
    assert_eq!(&request[cursor..cursor + first_len], encoded_event.as_slice());
    cursor += first_len;
    assert_eq!(read_varint(&request, &mut cursor), 0x0a);
}

#[test]
fn proto_projection_covers_shell_browser_and_computer_oneofs() {
    let actions = [
        AuditAction::ShellCommand {
            command: "rm x".into(),
            shell_kind: "host".into(),
            target: "/tmp".into(),
            allowed: Some(false),
            blocked_reason: Some("review".into()),
            classification_reasons: vec!["destructive".into()],
        },
        AuditAction::BrowserNavigation {
            url: "https://example.com/a".into(),
            page_title: "Example".into(),
        },
        AuditAction::ComputerUseSession {
            tool_call_id: Some("tool-1".into()),
            action_count: 2,
            action_counts: BTreeMap::from([("click".into(), 2)]),
            duration_ms: 9.6,
            screenshot_count: 1,
        },
    ];
    for (index, action) in actions.into_iter().enumerate() {
        let bytes = encode_audit_event(&AuditEvent {
            event_id: format!("e-{index}"),
            occurred_at_ms: 1,
            agent_id: "a".into(),
            turn_id: String::new(),
            box_id: String::new(),
            action,
        });
        assert!(!bytes.is_empty());
        let expected_oneof_key = [0x3a_u8, 0x42, 0x4a][index];
        assert!(
            bytes.contains(&expected_oneof_key),
            "missing oneof key {expected_oneof_key:#x} in {bytes:?}"
        );
    }
}
