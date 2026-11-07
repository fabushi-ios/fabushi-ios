use mahayana_host_runtime::extensions::transcript::ack_obligations::{
    AckObligations, AckRedrivePreparation, AckRedriveTrigger, MAX_ACK_REDRIVES,
    build_ack_redrive_send_args,
};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-ack-redrive-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn redrive_preparation_bumps_attempts_and_builds_hidden_recovery_send() {
    let root = temp_root("ready");
    fs::create_dir_all(&root).expect("root");
    let obligations = AckObligations::new(&root);
    obligations.record_send("agent-a", 10.0).expect("record");

    let bumped = match obligations.prepare_redrive("agent-a", true).expect("prepare") {
        AckRedrivePreparation::Ready(obligation) => obligation,
        other => panic!("expected ready, got {other:?}"),
    };
    assert_eq!(bumped.redrive_attempts, 1.0);
    let args = build_ack_redrive_send_args(
        "agent-a",
        "agent:agent-a:main",
        &bumped,
        AckRedriveTrigger::Boot,
        1234,
    );
    assert_eq!(args["agentId"], "agent-a");
    assert_eq!(args["appendUserMessage"], false);
    assert_eq!(args["awaitTurn"], false);
    assert_eq!(args["hidden"], true);
    assert_eq!(args["skipAckObligation"], true);
    assert_eq!(args["requestSource"], "handoff-resume");
    assert_eq!(args["ackRedriveTrigger"], "boot");
    assert!(args["prompt"].as_str().is_some_and(|prompt| prompt.contains("SendMessage")));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn redrive_marks_deleted_and_max_attempt_obligations_lost() {
    let root = temp_root("lost");
    fs::create_dir_all(&root).expect("root");
    let obligations = AckObligations::new(&root);
    obligations.record_send("deleted", 1.0).expect("record deleted");
    assert!(matches!(
        obligations.prepare_redrive("deleted", false).expect("deleted prepare"),
        AckRedrivePreparation::LostAgentDeleted(_)
    ));
    assert!(obligations.store().get("deleted").is_none());

    obligations.record_send("maxed", 2.0).expect("record maxed");
    for _ in 0..MAX_ACK_REDRIVES {
        assert!(matches!(
            obligations.prepare_redrive("maxed", true).expect("ready"),
            AckRedrivePreparation::Ready(_)
        ));
    }
    assert!(matches!(
        obligations.prepare_redrive("maxed", true).expect("max prepare"),
        AckRedrivePreparation::LostMaxRedrives(_)
    ));
    assert!(obligations.store().get("maxed").is_none());

    let _ = fs::remove_dir_all(root);
}
