use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use mahayana_host_runtime::transcript_mirror::transcript_mirror_router::{
    JournalEnabledReader, LegacyTranscriptMirrorPort, RoutedTranscriptMirror,
    TranscriptJournalPort, TranscriptMirrorRoute,
};

#[derive(Default)]
struct FakeJournal {
    owns: AtomicBool,
    claims: AtomicUsize,
    recovers: AtomicUsize,
    prepares: AtomicUsize,
    commits: AtomicUsize,
    aborts: AtomicUsize,
    skips: AtomicUsize,
    claim_delay_ms: AtomicUsize,
}

impl TranscriptJournalPort<String, String> for FakeJournal {
    fn owns_conversation(&self, _conversation_id: &str) -> Result<bool, String> {
        Ok(self.owns.load(Ordering::SeqCst))
    }

    fn claim_conversation(&self, _conversation_id: &str) -> Result<(), String> {
        self.claims.fetch_add(1, Ordering::SeqCst);
        let delay = self.claim_delay_ms.load(Ordering::SeqCst);
        if delay > 0 {
            thread::sleep(Duration::from_millis(delay as u64));
        }
        self.owns.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn recover(
        &self,
        _conversation_id: &str,
        _checkpoint: &String,
        _blob_store: &String,
    ) -> Result<(), String> {
        self.recovers.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn prepare_checkpoint(
        &self,
        _conversation_id: &str,
        _checkpoint: &String,
        _blob_store: &String,
        _finalize_checkpoint: bool,
    ) -> Result<(), String> {
        self.prepares.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn commit_checkpoint(&self, _conversation_id: &str) -> Result<(), String> {
        self.commits.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn abort_checkpoint(&self, _conversation_id: &str) -> Result<(), String> {
        self.aborts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn skip_checkpoint(
        &self,
        _conversation_id: &str,
        _checkpoint: &String,
        _blob_store: &String,
    ) -> Result<(), String> {
        self.skips.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Default)]
struct FakeLegacy {
    writes: AtomicUsize,
    seen: Mutex<Vec<(String, String, Vec<u8>)>>,
    fail: AtomicBool,
}

impl LegacyTranscriptMirrorPort<String, String> for FakeLegacy {
    fn write(
        &self,
        conversation_id: &str,
        checkpoint: &String,
        _blob_store: &String,
        state_blob_id: &[u8],
    ) -> Result<(), String> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().expect("seen").push((
            conversation_id.to_string(),
            checkpoint.clone(),
            state_blob_id.to_vec(),
        ));
        if self.fail.load(Ordering::SeqCst) {
            Err("legacy failed".into())
        } else {
            Ok(())
        }
    }
}

fn routed(
    journal: Arc<FakeJournal>,
    legacy: Arc<FakeLegacy>,
    enabled: Arc<AtomicBool>,
) -> RoutedTranscriptMirror<String, String> {
    let reader: JournalEnabledReader = Arc::new(move || Ok(enabled.load(Ordering::SeqCst)));
    RoutedTranscriptMirror::new(journal, legacy, reader)
}

#[test]
fn legacy_route_is_sticky_after_feature_flag_changes() {
    let journal = Arc::new(FakeJournal::default());
    let legacy = Arc::new(FakeLegacy::default());
    let enabled = Arc::new(AtomicBool::new(false));
    let router = routed(Arc::clone(&journal), legacy, Arc::clone(&enabled));

    assert_eq!(
        router.route("conversation-a").expect("legacy route"),
        TranscriptMirrorRoute::Legacy
    );
    enabled.store(true, Ordering::SeqCst);
    assert_eq!(
        router.route("conversation-a").expect("sticky route"),
        TranscriptMirrorRoute::Legacy
    );
    assert_eq!(journal.claims.load(Ordering::SeqCst), 0);
}

#[test]
fn concurrent_first_route_claims_journal_only_once() {
    let journal = Arc::new(FakeJournal::default());
    journal.claim_delay_ms.store(75, Ordering::SeqCst);
    let legacy = Arc::new(FakeLegacy::default());
    let enabled = Arc::new(AtomicBool::new(true));
    let router = Arc::new(routed(
        Arc::clone(&journal),
        legacy,
        enabled,
    ));

    let mut handles = Vec::new();
    for _ in 0..8 {
        let router = Arc::clone(&router);
        handles.push(thread::spawn(move || router.route("conversation-a")));
    }
    for handle in handles {
        assert_eq!(
            handle.join().expect("route thread").expect("route"),
            TranscriptMirrorRoute::Journal
        );
    }
    assert_eq!(journal.claims.load(Ordering::SeqCst), 1);
    assert_eq!(
        router.cached_route("conversation-a"),
        Some(TranscriptMirrorRoute::Journal)
    );
}

#[test]
fn existing_journal_ownership_overrides_disabled_flag_without_claim() {
    let journal = Arc::new(FakeJournal::default());
    journal.owns.store(true, Ordering::SeqCst);
    let router = routed(
        Arc::clone(&journal),
        Arc::new(FakeLegacy::default()),
        Arc::new(AtomicBool::new(false)),
    );
    assert_eq!(
        router.route("conversation-owned").expect("owned route"),
        TranscriptMirrorRoute::Journal
    );
    assert_eq!(journal.claims.load(Ordering::SeqCst), 0);
}

#[test]
fn legacy_checkpoint_uses_latest_pending_state_and_write_failure_is_observational() {
    let journal = Arc::new(FakeJournal::default());
    let legacy = Arc::new(FakeLegacy::default());
    legacy.fail.store(true, Ordering::SeqCst);
    let router = routed(
        journal,
        Arc::clone(&legacy),
        Arc::new(AtomicBool::new(false)),
    );

    router
        .prepare_checkpoint(
            "conversation-a",
            &"checkpoint-1".into(),
            &"store".into(),
            false,
            true,
        )
        .expect("prepare first");
    router
        .prepare_checkpoint(
            "conversation-a",
            &"checkpoint-2".into(),
            &"store".into(),
            true,
            true,
        )
        .expect("prepare latest");
    assert_eq!(router.legacy_pending_count(), 1);

    router
        .commit_checkpoint("conversation-a", &[9, 8, 7])
        .expect("legacy failure must not fail durable commit");
    assert_eq!(router.legacy_pending_count(), 0);
    assert_eq!(legacy.writes.load(Ordering::SeqCst), 1);
    assert_eq!(
        legacy.seen.lock().expect("seen").as_slice(),
        &[(
            "conversation-a".to_string(),
            "checkpoint-2".to_string(),
            vec![9, 8, 7],
        )]
    );
}

#[test]
fn skip_without_marker_never_claims_even_when_journal_flag_is_enabled() {
    let journal = Arc::new(FakeJournal::default());
    let router = routed(
        Arc::clone(&journal),
        Arc::new(FakeLegacy::default()),
        Arc::new(AtomicBool::new(true)),
    );
    router
        .skip_checkpoint(
            "conversation-a",
            &"checkpoint".into(),
            &"store".into(),
        )
        .expect("skip");
    assert_eq!(journal.claims.load(Ordering::SeqCst), 0);
    assert_eq!(journal.recovers.load(Ordering::SeqCst), 0);
    assert_eq!(journal.skips.load(Ordering::SeqCst), 0);
    assert_eq!(router.cached_route("conversation-a"), None);
}

#[test]
fn skip_recovers_owned_journal_before_skipping_it() {
    let journal = Arc::new(FakeJournal::default());
    journal.owns.store(true, Ordering::SeqCst);
    let router = routed(
        Arc::clone(&journal),
        Arc::new(FakeLegacy::default()),
        Arc::new(AtomicBool::new(false)),
    );
    router
        .skip_checkpoint(
            "conversation-a",
            &"checkpoint".into(),
            &"store".into(),
        )
        .expect("skip owned journal");
    assert_eq!(journal.recovers.load(Ordering::SeqCst), 1);
    assert_eq!(journal.skips.load(Ordering::SeqCst), 1);
    assert_eq!(
        router.cached_route("conversation-a"),
        Some(TranscriptMirrorRoute::Journal)
    );
}
