use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::durable_file_policy::SAND_ACK_OBLIGATIONS_FILE_NAME;
use mahayana_host_runtime::extensions::transcript::sand_ack_obligation_store::{
    AckObligation, SandAckObligationStore, parse_ack_obligations_file,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-ack-store-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn frozen_ack_store_parser_coerces_invalid_fields_and_ignores_bad_rows() {
    let parsed = parse_ack_obligations_file(Some(
        r#"{"pending":[{"agentId":"a","createdAtMs":7,"lastSendAtMs":"bad","lastInterruptAtMs":9,"coalescedCount":0,"redriveAttempts":-3},{"agentId":"","createdAtMs":1},7]}"#,
    ));
    assert_eq!(
        parsed,
        vec![AckObligation {
            agent_id: "a".into(),
            created_at_ms: 7.0,
            last_send_at_ms: 7.0,
            last_interrupt_at_ms: Some(9.0),
            coalesced_count: 1.0,
            redrive_attempts: 0.0,
        }]
    );
    assert!(parse_ack_obligations_file(Some("not-json")).is_empty());
    assert!(parse_ack_obligations_file(None).is_empty());
}

#[test]
fn durable_store_coalesces_interrupts_redrives_and_writes_atomically() {
    let root = temp_root("durable");
    let store = SandAckObligationStore::new(&root);

    let first = store.record_send("agent-a", 10.0).expect("first send");
    assert!(first.created);
    assert_eq!(first.obligation.coalesced_count, 1.0);

    let second = store.record_send("agent-a", 20.0).expect("second send");
    assert!(!second.created);
    assert_eq!(second.obligation.created_at_ms, 10.0);
    assert_eq!(second.obligation.last_send_at_ms, 20.0);
    assert_eq!(second.obligation.coalesced_count, 2.0);

    assert!(store.record_interrupt("agent-a", 21.0).expect("interrupt"));
    let redriven = store
        .record_redrive_attempt("agent-a")
        .expect("redrive")
        .expect("obligation");
    assert_eq!(redriven.last_interrupt_at_ms, Some(21.0));
    assert_eq!(redriven.redrive_attempts, 1.0);

    let file = root.join(SAND_ACK_OBLIGATIONS_FILE_NAME);
    let persisted = fs::read_to_string(&file).expect("persisted ack file");
    assert_eq!(parse_ack_obligations_file(Some(&persisted)), vec![redriven]);
    assert!(
        !std::path::PathBuf::from(format!("{}.part", file.display())).exists(),
        "atomic .part file must be renamed away"
    );

    assert!(store.clear("agent-a").expect("clear"));
    assert!(store.list().is_empty());
    let _ = fs::remove_dir_all(root);
}
