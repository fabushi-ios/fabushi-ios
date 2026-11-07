use std::path::Path;

use mahayana_host_runtime::extensions::transcript::prompt_acceptance_ledger::{
    AcceptanceLookup, PromptAcceptanceError, PromptAcceptanceLedger, SendInput,
};
use mahayana_host_runtime::extensions::transcript::send_pipeline::{
    SendBegin, SendEchoIdentity, SendPipelineState,
};

fn input(agent: &str, prompt: &str) -> SendInput {
    SendInput {
        agent_id: Some(agent.to_owned()),
        prompt: prompt.to_owned(),
        rich_text: None,
        reply_to_id: None,
        is_fork: false,
        attachment_paths: Vec::new(),
        attachment_names: Vec::new(),
    }
}

fn ledger() -> PromptAcceptanceLedger {
    PromptAcceptanceLedger::new(None::<&Path>)
}

#[test]
fn identical_concurrent_nonce_is_coalesced_without_second_dispatch() {
    let mut state = SendPipelineState::default();
    let mut ledger = ledger();
    let send = input("agent-a", "hello");
    assert!(matches!(
        state.begin_send(&mut ledger, &send, Some("nonce")),
        Ok(SendBegin::Dispatch { .. })
    ));
    assert!(matches!(
        state.begin_send(&mut ledger, &send, Some("nonce")),
        Ok(SendBegin::Coalesced { client_nonce }) if client_nonce == "nonce"
    ));
    assert_eq!(state.in_flight_count(), 1);
    assert!(matches!(ledger.lookup("host", "nonce"), AcceptanceLookup::NotFound));
}

#[test]
fn concurrent_nonce_cannot_coalesce_different_agent_or_prompt() {
    let mut state = SendPipelineState::default();
    let mut ledger = ledger();
    let original = input("agent-a", "hello");
    state.begin_send(&mut ledger, &original, Some("nonce"))
        .expect("admit original");
    for conflicting in [
        input("agent-b", "hello"),
        input("agent-a", "altered prompt"),
    ] {
        assert!(matches!(
            state.begin_send(&mut ledger, &conflicting, Some("nonce")),
            Err(PromptAcceptanceError::DigestMismatch { client_nonce }) if client_nonce == "nonce"
        ));
    }
    assert_eq!(state.in_flight_count(), 1);
}

#[test]
fn conflicting_reply_and_attachments_are_rejected_without_mutating_admission() {
    let mut state = SendPipelineState::default();
    let mut ledger = ledger();
    let original = input("agent-a", "hello");
    state.begin_send(&mut ledger, &original, Some("same"))
        .expect("admit original");
    let mut different_reply = original.clone();
    different_reply.reply_to_id = Some("other-thread".into());
    let mut different_attachment = original.clone();
    different_attachment.attachment_paths.push("/tmp/other.txt".into());
    for candidate in [different_reply, different_attachment] {
        assert!(matches!(
            state.begin_send(&mut ledger, &candidate, Some("same")),
            Err(PromptAcceptanceError::DigestMismatch { .. })
        ));
    }
    assert!(state.is_in_flight("same"));
    assert_eq!(state.in_flight_count(), 1);
}

#[test]
fn accepted_nonce_stays_durable_and_failed_in_flight_nonce_can_retry() {
    let mut state = SendPipelineState::default();
    let mut ledger = ledger();
    let original = input("agent-a", "first");
    state.begin_send(&mut ledger, &original, Some("accepted"))
        .expect("first admission");
    state.record_pending_acceptance(
        &mut ledger, "accepted",
        SendEchoIdentity { agent_id: "agent-a".into(), echo_entry_id: Some("entry-1".into()) },
    ).expect("record durable pending identity");
    state.mark_send_accepted(&mut ledger, Some("accepted"));
    state.finish_send(&mut ledger, Some("accepted"), true);
    assert!(matches!(
        state.begin_send(&mut ledger, &original, Some("accepted")),
        Ok(SendBegin::DuplicateNoop { .. })
    ));
    assert!(matches!(
        state.begin_send(&mut ledger, &input("agent-b", "first"), Some("accepted")),
        Err(PromptAcceptanceError::DigestMismatch { .. })
    ));
    state.begin_send(&mut ledger, &original, Some("retry"))
        .expect("new in-flight nonce");
    state.finish_send(&mut ledger, Some("retry"), false);
    assert!(!state.is_in_flight("retry"));
    assert!(matches!(
        state.begin_send(&mut ledger, &input("agent-b", "new"), Some("retry")),
        Ok(SendBegin::Dispatch { .. })
    ));
}
