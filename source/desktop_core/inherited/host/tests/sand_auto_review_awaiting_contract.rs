use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::auto_review::sand_auto_review_awaiting::{
    SAND_AUTO_REVIEW_AWAITING_TAB_ID, SandAutoReviewAwaitingBridge,
    SandAutoReviewAwaitingSink, SandAutoReviewAwaitingState,
    build_sand_auto_review_awaiting_reason,
};
use mahayana_host_runtime::runner::sand_auto_review::{
    SandAutoReviewApproval, SandAutoReviewApprovalStatus,
    SandAutoReviewEvent, SandAutoReviewExpiryCause, SandAutoReviewSurface,
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum SinkEvent {
    Set {
        agent_id: String,
        state: SandAutoReviewAwaitingState,
    },
    Clear {
        agent_id: String,
        tab_id: String,
    },
}

#[derive(Clone, Default)]
struct RecordingSink {
    events: Arc<Mutex<Vec<SinkEvent>>>,
}

impl SandAutoReviewAwaitingSink for RecordingSink {
    fn try_set_for_tab(
        &self,
        agent_id: &str,
        state: SandAutoReviewAwaitingState,
    ) {
        self.events.lock().expect("sink events").push(SinkEvent::Set {
            agent_id: agent_id.into(),
            state,
        });
    }

    fn clear_for_tab(&self, agent_id: &str, tab_id: &str) {
        self.events.lock().expect("sink events").push(SinkEvent::Clear {
            agent_id: agent_id.into(),
            tab_id: tab_id.into(),
        });
    }
}

fn approval(id: &str, agent_id: &str, summary: &str) -> SandAutoReviewApproval {
    SandAutoReviewApproval {
        id: id.into(),
        agent_id: agent_id.into(),
        session_key: None,
        surface: SandAutoReviewSurface::HostShell,
        fingerprint: format!("fingerprint-{id}"),
        reason: format!("reason-{id}"),
        summary: summary.into(),
        command: None,
        proposed_rule: None,
        user_message_epoch: 1,
        host_generation: "host-generation".into(),
        created_at_ms: 10,
        expires_at_ms: None,
        status: SandAutoReviewApprovalStatus::Pending,
    }
}

#[test]
fn awaiting_reason_matches_frozen_copy() {
    assert_eq!(
        build_sand_auto_review_awaiting_reason("Run a sensitive command"),
        "Approval needed: Run a sensitive command"
    );
}

#[test]
fn newest_pending_reason_wins_and_completion_falls_back() {
    let sink = RecordingSink::default();
    let events = Arc::clone(&sink.events);
    let mut bridge = SandAutoReviewAwaitingBridge::with_now(
        sink,
        Arc::new(|| 1234),
    );

    let first = approval("one", "agent-a", "First");
    let second = approval("two", "agent-a", "Second");

    bridge.handle_event(&SandAutoReviewEvent::Created(first.clone()));
    bridge.handle_event(&SandAutoReviewEvent::Created(second.clone()));

    assert_eq!(bridge.pending_count("agent-a"), 2);
    assert_eq!(
        bridge.pending_awaiting_state("agent-a"),
        Some(SandAutoReviewAwaitingState {
            tab_id: SAND_AUTO_REVIEW_AWAITING_TAB_ID.into(),
            reason: "Approval needed: Second".into(),
            since: 1234,
        })
    );

    bridge.handle_event(&SandAutoReviewEvent::Resolved(second));
    assert_eq!(bridge.pending_count("agent-a"), 1);
    assert_eq!(
        bridge.pending_awaiting_state("agent-a")
            .expect("fallback pending")
            .reason,
        "Approval needed: First"
    );

    bridge.handle_event(&SandAutoReviewEvent::Expired {
        approval: first,
        cause: SandAutoReviewExpiryCause::Ttl,
    });
    assert_eq!(bridge.pending_count("agent-a"), 0);
    assert_eq!(bridge.pending_awaiting_state("agent-a"), None);

    let recorded = events.lock().expect("sink events");
    assert_eq!(
        recorded.last(),
        Some(&SinkEvent::Clear {
            agent_id: "agent-a".into(),
            tab_id: SAND_AUTO_REVIEW_AWAITING_TAB_ID.into(),
        })
    );
}

#[test]
fn duplicate_created_updates_reason_without_reordering() {
    let sink = RecordingSink::default();
    let mut bridge = SandAutoReviewAwaitingBridge::with_now(
        sink,
        Arc::new(|| 500),
    );

    let first = approval("one", "agent-a", "First");
    let second = approval("two", "agent-a", "Second");
    let updated_first = approval("one", "agent-a", "First updated");

    bridge.handle_event(&SandAutoReviewEvent::Created(first));
    bridge.handle_event(&SandAutoReviewEvent::Created(second.clone()));
    bridge.handle_event(&SandAutoReviewEvent::Created(updated_first));

    assert_eq!(
        bridge.pending_awaiting_state("agent-a")
            .expect("pending state")
            .reason,
        "Approval needed: Second",
        "Map.set on an existing id must not move it to the end"
    );

    bridge.handle_event(&SandAutoReviewEvent::Resolved(second));
    assert_eq!(
        bridge.pending_awaiting_state("agent-a")
            .expect("updated first remains")
            .reason,
        "Approval needed: First updated"
    );
}

#[test]
fn unknown_completion_does_not_clear_existing_state() {
    let sink = RecordingSink::default();
    let events = Arc::clone(&sink.events);
    let mut bridge = SandAutoReviewAwaitingBridge::with_now(
        sink,
        Arc::new(|| 42),
    );

    bridge.handle_event(&SandAutoReviewEvent::Created(approval(
        "known",
        "agent-a",
        "Known",
    )));
    let before = events.lock().expect("sink events").len();

    bridge.handle_event(&SandAutoReviewEvent::Resolved(approval(
        "unknown",
        "agent-a",
        "Unknown",
    )));

    assert_eq!(bridge.pending_count("agent-a"), 1);
    assert_eq!(events.lock().expect("sink events").len(), before);
}
