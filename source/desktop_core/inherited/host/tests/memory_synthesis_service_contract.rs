use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mahayana_host_runtime::extensions::memory::memory_service::{FileMemoryStore, MemoryKind};
use mahayana_host_runtime::extensions::memory::memory_synthesis_service::{
    MEMORY_SYNTHESIS_DEADLINE_MS, MEMORY_SYNTHESIS_DEBOUNCE_MS,
    MEMORY_SYNTHESIS_POLL_INTERVAL_MS, MemoryEvidence, MemorySynthesisOptions,
    MemorySynthesisService, SynthesisOutcome, SynthesisReportOutcome,
    bounded_evidence_text, parse_memory_synthesis_changes, synthesis_request_json,
    synthesis_system_prompt, uses_known_evidence, verification_request_json,
    verification_system_prompt,
};
use serde_json::json;

fn temp_store() -> (std::path::PathBuf, Arc<FileMemoryStore>) {
    let root = std::env::temp_dir().join(format!(
        "fabushi-memory-synthesis-service-{}",
        uuid::Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&root).unwrap();
    let store = Arc::new(FileMemoryStore::with_synthesis_metadata(
        root.join("memory"),
        true,
    ));
    (root, store)
}

#[test]
fn parser_rejects_unknown_evidence_and_clock_only_creation() {
    let raw = json!({"changes":[{
        "action":"create",
        "content":"Prefers concise replies",
        "kind":"profile",
        "sourceEvidenceIds":["ev-1"]
    }]});
    let changes = parse_memory_synthesis_changes(&raw).unwrap();
    let known = ["ev-1".to_string()].into_iter().collect();
    assert!(uses_known_evidence(&known, &changes, false));
    assert!(!uses_known_evidence(&Default::default(), &changes, false));

    let clock = json!({"changes":[{
        "action":"create",
        "content":"A new unsupported fact",
        "kind":"log",
        "sourceEvidenceIds":["clock"]
    }]});
    let clock_changes = parse_memory_synthesis_changes(&clock).unwrap();
    assert!(!uses_known_evidence(
        &Default::default(),
        &clock_changes,
        true
    ));
}

#[test]
fn proposal_and_verification_commit_through_synthesis_target() {
    let (root, store) = temp_store();
    let targets = Arc::new(Mutex::new(HashMap::from([(
        "agent-1".to_string(),
        store.clone(),
    )])));
    let list_targets = {
        let targets = targets.clone();
        Arc::new(move || {
            targets
                .lock()
                .unwrap()
                .iter()
                .map(|(id, store)| {
                    (
                        id.clone(),
                        store.clone()
                            as Arc<dyn mahayana_host_runtime::extensions::memory::memory_synthesis_service::SynthesisTarget>,
                    )
                })
                .collect()
        })
    };
    let get_target = {
        let targets = targets.clone();
        Arc::new(move |id: &str| {
            targets
                .lock()
                .unwrap()
                .get(id)
                .cloned()
                .map(|store| {
                    store
                        as Arc<dyn mahayana_host_runtime::extensions::memory::memory_synthesis_service::SynthesisTarget>
                })
        })
    };
    let proposes = Arc::new(AtomicUsize::new(0));
    let proposes_for_call = proposes.clone();
    let mut options = MemorySynthesisOptions::new(
        list_targets,
        get_target,
        Arc::new(move |_request, _cancel| {
            proposes_for_call.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"changes":[{
                "action":"create",
                "content":"Prefers concise replies",
                "kind":"profile",
                "sourceEvidenceIds":["ev-1"]
            }]}))
        }),
    );
    options.verify = Some(Arc::new(|_request, _cancel| Ok(true)));
    options.debounce = Duration::from_millis(1);
    options.deadline = Duration::from_secs(1);
    options.poll_interval = Duration::from_secs(60);
    options.retry_initial = Duration::from_millis(1);
    options.retry_max = Duration::from_millis(2);

    let service = MemorySynthesisService::new(options);
    service.start();
    service.record_turn(
        "agent-1",
        Some("ev-1".into()),
        "Please keep replies concise",
        "Understood",
        1_700_000_000_000,
    );
    let outcomes = service.run_now();
    assert_eq!(outcomes, vec![SynthesisOutcome::Committed]);
    assert_eq!(proposes.load(Ordering::SeqCst), 1);
    let memories = store.prepare_synthesis().memories;
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0].content, "Prefers concise replies");
    service.dispose();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn verifier_rejection_does_not_commit_and_retryable_failures_stop_at_bound() {
    let (root, store) = temp_store();
    let target = store.clone()
        as Arc<dyn mahayana_host_runtime::extensions::memory::memory_synthesis_service::SynthesisTarget>;
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_for_call = attempts.clone();
    let mut options = MemorySynthesisOptions::new(
        Arc::new({
            let target = target.clone();
            move || vec![("agent".into(), target.clone())]
        }),
        Arc::new({
            let target = target.clone();
            move |_id| Some(target.clone())
        }),
        Arc::new(move |_request, _cancel| {
            attempts_for_call.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"changes":[{
                "action":"create",
                "content":"Needs verification",
                "kind":"log",
                "sourceEvidenceIds":["ev"]
            }]}))
        }),
    );
    options.verify = Some(Arc::new(|_request, _cancel| Ok(false)));
    options.debounce = Duration::from_secs(60);
    options.deadline = Duration::from_secs(1);
    options.poll_interval = Duration::from_secs(60);
    options.retry_attempts = 1;
    let service = MemorySynthesisService::new(options);
    service.start();
    service.record_turn("agent", Some("ev".into()), "u", "a", 1);
    assert_eq!(service.run_now(), vec![SynthesisOutcome::Rejected]);
    assert_eq!(store.count_memories(), 0);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    service.dispose();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn evidence_bounding_keeps_both_ends() {
    let long = format!("{}{}", "a".repeat(9_000), "z".repeat(9_000));
    let bounded = bounded_evidence_text(&long);
    assert!(bounded.starts_with('a'));
    assert!(bounded.ends_with('z'));
    assert!(bounded.contains("[...middle omitted...]"));
}


#[test]
fn frozen_synthesis_prompts_and_wire_payloads_are_preserved() {
    use mahayana_host_runtime::extensions::memory::memory_service::{
        MemoryKind, MemoryOrigin, SynthesisMemory,
    };
    use mahayana_host_runtime::extensions::memory::memory_synthesis_service::{
        MemoryChange, SynthesisProposalRequest, SynthesisVerificationRequest,
    };

    assert!(synthesis_system_prompt().starts_with("<<SAND_MEMORY_SYNTHESIS_V1>>"));
    assert!(verification_system_prompt()
        .starts_with("<<SAND_MEMORY_SYNTHESIS_VERIFICATION_V1>>"));

    let proposal = SynthesisProposalRequest {
        today: "2026-09-29".into(),
        current_memories: vec![SynthesisMemory {
            id: "m1".into(),
            content: "Prefers concise replies".into(),
            created_at: 123,
            kind: MemoryKind::Profile,
            origin: MemoryOrigin::Explicit,
        }],
        new_evidence: vec![MemoryEvidence {
            id: "ev1".into(),
            occurred_at: 456,
            user: "Please stay concise".into(),
            assistant: "Understood".into(),
        }],
    };
    let proposal_json: serde_json::Value =
        serde_json::from_str(&synthesis_request_json(&proposal)).unwrap();
    assert_eq!(proposal_json["today"], "2026-09-29");
    assert_eq!(proposal_json["currentMemories"][0]["origin"], "explicit");
    assert_eq!(proposal_json["newEvidence"][0]["id"], "ev1");

    let verification = SynthesisVerificationRequest {
        today: proposal.today,
        current_memories: proposal.current_memories,
        evidence: proposal.new_evidence,
        proposed_changes: vec![MemoryChange::Create {
            content: "Prefers concise replies".into(),
            kind: MemoryKind::Profile,
            source_evidence_ids: vec!["ev1".into()],
        }],
    };
    let verification_json: serde_json::Value =
        serde_json::from_str(&verification_request_json(&verification)).unwrap();
    assert_eq!(verification_json["proposedChanges"][0]["action"], "create");
    assert_eq!(
        verification_json["proposedChanges"][0]["sourceEvidenceIds"][0],
        "ev1"
    );
}


#[test]
fn frozen_production_scheduling_constants_are_enforced() {
    assert_eq!(MEMORY_SYNTHESIS_DEBOUNCE_MS, 15_000);
    assert_eq!(MEMORY_SYNTHESIS_DEADLINE_MS, 90_000);
    assert_eq!(MEMORY_SYNTHESIS_POLL_INTERVAL_MS, 3_600_000);
}

#[test]
fn synthesis_deadline_cancels_verification_and_reports_terminal_failure() {
    let (root, store) = temp_store();
    let target = store.clone()
        as Arc<dyn mahayana_host_runtime::extensions::memory::memory_synthesis_service::SynthesisTarget>;
    let saw_cancel = Arc::new(AtomicBool::new(false));
    let saw_cancel_for_verify = Arc::clone(&saw_cancel);
    let reports = Arc::new(Mutex::new(Vec::new()));
    let reports_for_callback = Arc::clone(&reports);

    let mut options = MemorySynthesisOptions::new(
        Arc::new({
            let target = target.clone();
            move || vec![("agent".into(), target.clone())]
        }),
        Arc::new({
            let target = target.clone();
            move |_id| Some(target.clone())
        }),
        Arc::new(|_request, _cancel| {
            Ok(json!({"changes":[{
                "action":"create",
                "content":"Deadline protected memory",
                "kind":"log",
                "sourceEvidenceIds":["ev"]
            }]}))
        }),
    );
    options.verify = Some(Arc::new(move |_request, cancel| {
        while !cancel.is_cancelled() {
            std::thread::sleep(Duration::from_millis(1));
        }
        saw_cancel_for_verify.store(true, Ordering::SeqCst);
        Err("cancelled".into())
    }));
    options.deadline = Duration::from_millis(25);
    options.retry_attempts = 1;
    options.report = Some(Arc::new(move |report| {
        reports_for_callback.lock().unwrap().push(report);
    }));

    let service = MemorySynthesisService::new(options);
    service.start();
    service.record_turn("agent", Some("ev".into()), "u", "a", 1);
    assert_eq!(service.run_now(), vec![SynthesisOutcome::Failed]);
    for _ in 0..100 {
        if saw_cancel.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(saw_cancel.load(Ordering::SeqCst));
    let reports = reports.lock().unwrap();
    assert_eq!(reports.last().unwrap().outcome, SynthesisReportOutcome::Failed);
    assert_eq!(store.count_memories(), 0);
    service.dispose();
    let _ = fs::remove_dir_all(root);
}
