use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptMirrorRoute {
    Journal,
    Legacy,
}

pub trait TranscriptJournalPort<Checkpoint, Store> {
    fn owns_conversation(&mut self, conversation_id: &str) -> Result<bool, String>;
    fn claim_conversation(&mut self, conversation_id: &str) -> Result<(), String>;
    fn recover(
        &mut self,
        conversation_id: &str,
        checkpoint: &Checkpoint,
        blob_store: &Store,
    ) -> Result<(), String>;
    fn prepare_checkpoint(
        &mut self,
        conversation_id: &str,
        checkpoint: &Checkpoint,
        blob_store: &Store,
        finalize_checkpoint: bool,
    ) -> Result<(), String>;
    fn commit_checkpoint(&mut self, conversation_id: &str) -> Result<(), String>;
    fn abort_checkpoint(&mut self, conversation_id: &str) -> Result<(), String>;
    fn skip_checkpoint(
        &mut self,
        conversation_id: &str,
        checkpoint: &Checkpoint,
        blob_store: &Store,
    ) -> Result<(), String>;
}

pub trait LegacyTranscriptMirrorPort<Checkpoint, Store> {
    fn write(
        &mut self,
        conversation_id: &str,
        checkpoint: &Checkpoint,
        blob_store: &Store,
        state_blob_id: &[u8],
    ) -> Result<(), String>;
}

#[derive(Debug, Clone)]
struct LegacyPending<Checkpoint, Store> {
    checkpoint: Checkpoint,
    blob_store: Store,
}

/// Rust/iOS equivalent of Grok's RoutedTranscriptMirror.
///
/// The reference caches route promises to prevent concurrent first-write races.
/// The iOS Host executes checkpoint routing on its serialized Host lane, so this
/// value owns a concrete route cache instead of emulating JavaScript promises.
pub struct RoutedTranscriptMirror<Checkpoint, Store, Journal, Legacy, FeatureFlag>
where
    Checkpoint: Clone,
    Store: Clone,
    Journal: TranscriptJournalPort<Checkpoint, Store>,
    Legacy: LegacyTranscriptMirrorPort<Checkpoint, Store>,
    FeatureFlag: FnMut() -> Result<bool, String>,
{
    journal: Journal,
    legacy: Legacy,
    is_journal_enabled: FeatureFlag,
    routes: HashMap<String, TranscriptMirrorRoute>,
    legacy_pending: HashMap<String, LegacyPending<Checkpoint, Store>>,
}

impl<Checkpoint, Store, Journal, Legacy, FeatureFlag>
    RoutedTranscriptMirror<Checkpoint, Store, Journal, Legacy, FeatureFlag>
where
    Checkpoint: Clone,
    Store: Clone,
    Journal: TranscriptJournalPort<Checkpoint, Store>,
    Legacy: LegacyTranscriptMirrorPort<Checkpoint, Store>,
    FeatureFlag: FnMut() -> Result<bool, String>,
{
    pub fn new(journal: Journal, legacy: Legacy, is_journal_enabled: FeatureFlag) -> Self {
        Self {
            journal,
            legacy,
            is_journal_enabled,
            routes: HashMap::new(),
            legacy_pending: HashMap::new(),
        }
    }

    pub fn route(&mut self, conversation_id: &str) -> Result<TranscriptMirrorRoute, String> {
        if let Some(route) = self.routes.get(conversation_id).copied() {
            return Ok(route);
        }
        let selected = if self.journal.owns_conversation(conversation_id)? {
            TranscriptMirrorRoute::Journal
        } else if !(self.is_journal_enabled)()? {
            TranscriptMirrorRoute::Legacy
        } else {
            self.journal.claim_conversation(conversation_id)?;
            TranscriptMirrorRoute::Journal
        };
        self.routes.insert(conversation_id.to_owned(), selected);
        Ok(selected)
    }

    pub fn recover(
        &mut self,
        conversation_id: &str,
        checkpoint: &Checkpoint,
        blob_store: &Store,
    ) -> Result<(), String> {
        if self.route(conversation_id)? == TranscriptMirrorRoute::Journal {
            self.journal
                .recover(conversation_id, checkpoint, blob_store)?;
        }
        Ok(())
    }

    pub fn prepare_checkpoint(
        &mut self,
        conversation_id: &str,
        checkpoint: &Checkpoint,
        blob_store: &Store,
        finalize_checkpoint: bool,
        write_legacy_checkpoint: bool,
    ) -> Result<(), String> {
        if self.route(conversation_id)? == TranscriptMirrorRoute::Journal {
            return self.journal.prepare_checkpoint(
                conversation_id,
                checkpoint,
                blob_store,
                finalize_checkpoint,
            );
        }
        if write_legacy_checkpoint {
            self.legacy_pending.insert(
                conversation_id.to_owned(),
                LegacyPending {
                    checkpoint: checkpoint.clone(),
                    blob_store: blob_store.clone(),
                },
            );
        }
        Ok(())
    }

    pub fn commit_checkpoint(
        &mut self,
        conversation_id: &str,
        state_blob_id: &[u8],
    ) -> Result<(), String> {
        if self.route(conversation_id)? == TranscriptMirrorRoute::Journal {
            return self.journal.commit_checkpoint(conversation_id);
        }
        let Some(pending) = self.legacy_pending.remove(conversation_id) else {
            return Ok(());
        };
        // The legacy mirror is observational. A failed mirror must not roll
        // back a durable checkpoint or fail the turn.
        let _ = self.legacy.write(
            conversation_id,
            &pending.checkpoint,
            &pending.blob_store,
            state_blob_id,
        );
        Ok(())
    }

    pub fn abort_checkpoint(&mut self, conversation_id: &str) -> Result<(), String> {
        if self.route(conversation_id)? == TranscriptMirrorRoute::Journal {
            return self.journal.abort_checkpoint(conversation_id);
        }
        self.legacy_pending.remove(conversation_id);
        Ok(())
    }

    pub fn skip_checkpoint(
        &mut self,
        conversation_id: &str,
        checkpoint: &Checkpoint,
        blob_store: &Store,
    ) -> Result<(), String> {
        let mut recover_owned_journal = false;
        let selected = match self.routes.get(conversation_id).copied() {
            Some(route) => route,
            None => {
                if !self.journal.owns_conversation(conversation_id)? {
                    self.legacy_pending.remove(conversation_id);
                    return Ok(());
                }
                recover_owned_journal = true;
                self.routes
                    .insert(conversation_id.to_owned(), TranscriptMirrorRoute::Journal);
                TranscriptMirrorRoute::Journal
            }
        };

        if selected == TranscriptMirrorRoute::Journal {
            if recover_owned_journal {
                self.journal
                    .recover(conversation_id, checkpoint, blob_store)?;
            }
            self.journal
                .skip_checkpoint(conversation_id, checkpoint, blob_store)?;
        } else {
            self.legacy_pending.remove(conversation_id);
        }
        Ok(())
    }

    pub fn route_for(&self, conversation_id: &str) -> Option<TranscriptMirrorRoute> {
        self.routes.get(conversation_id).copied()
    }

    pub fn legacy_pending_count(&self) -> usize {
        self.legacy_pending.len()
    }

    pub fn into_parts(self) -> (Journal, Legacy) {
        (self.journal, self.legacy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct JournalMock {
        owns: bool,
        claims: usize,
        recovers: usize,
        prepares: usize,
        commits: usize,
        aborts: usize,
        skips: usize,
    }

    impl TranscriptJournalPort<String, String> for JournalMock {
        fn owns_conversation(&mut self, _id: &str) -> Result<bool, String> {
            Ok(self.owns)
        }
        fn claim_conversation(&mut self, _id: &str) -> Result<(), String> {
            self.claims += 1;
            self.owns = true;
            Ok(())
        }
        fn recover(&mut self, _id: &str, _c: &String, _s: &String) -> Result<(), String> {
            self.recovers += 1;
            Ok(())
        }
        fn prepare_checkpoint(
            &mut self,
            _id: &str,
            _c: &String,
            _s: &String,
            _finalize: bool,
        ) -> Result<(), String> {
            self.prepares += 1;
            Ok(())
        }
        fn commit_checkpoint(&mut self, _id: &str) -> Result<(), String> {
            self.commits += 1;
            Ok(())
        }
        fn abort_checkpoint(&mut self, _id: &str) -> Result<(), String> {
            self.aborts += 1;
            Ok(())
        }
        fn skip_checkpoint(&mut self, _id: &str, _c: &String, _s: &String) -> Result<(), String> {
            self.skips += 1;
            Ok(())
        }
    }

    #[derive(Default)]
    struct LegacyMock {
        writes: usize,
        fail: bool,
    }

    impl LegacyTranscriptMirrorPort<String, String> for LegacyMock {
        fn write(
            &mut self,
            _id: &str,
            _checkpoint: &String,
            _store: &String,
            _state_blob_id: &[u8],
        ) -> Result<(), String> {
            self.writes += 1;
            if self.fail {
                Err("observational failure".into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn journal_route_is_claimed_once_and_pinned() {
        let journal = JournalMock::default();
        let legacy = LegacyMock::default();
        let mut enabled_calls = 0usize;
        let mut routed = RoutedTranscriptMirror::new(journal, legacy, || {
            enabled_calls += 1;
            Ok(true)
        });
        assert_eq!(
            routed.route("conversation-a").unwrap(),
            TranscriptMirrorRoute::Journal
        );
        assert_eq!(
            routed.route("conversation-a").unwrap(),
            TranscriptMirrorRoute::Journal
        );
        let (journal, _) = routed.into_parts();
        assert_eq!(journal.claims, 1);
        assert_eq!(enabled_calls, 1);
    }

    #[test]
    fn legacy_pending_uses_latest_final_checkpoint_and_failure_is_observational() {
        let journal = JournalMock::default();
        let legacy = LegacyMock {
            fail: true,
            ..LegacyMock::default()
        };
        let mut routed = RoutedTranscriptMirror::new(journal, legacy, || Ok(false));
        routed
            .prepare_checkpoint(
                "conversation-a",
                &"checkpoint".into(),
                &"store".into(),
                true,
                true,
            )
            .unwrap();
        assert_eq!(routed.legacy_pending_count(), 1);
        routed
            .commit_checkpoint("conversation-a", &[1, 2, 3])
            .unwrap();
        assert_eq!(routed.legacy_pending_count(), 0);
        let (_, legacy) = routed.into_parts();
        assert_eq!(legacy.writes, 1);
    }

    #[test]
    fn skip_recovers_an_owned_journal_before_skipping() {
        let journal = JournalMock {
            owns: true,
            ..JournalMock::default()
        };
        let legacy = LegacyMock::default();
        let mut routed = RoutedTranscriptMirror::new(journal, legacy, || Ok(false));
        routed
            .skip_checkpoint("conversation-a", &"checkpoint".into(), &"store".into())
            .unwrap();
        assert_eq!(
            routed.route_for("conversation-a"),
            Some(TranscriptMirrorRoute::Journal)
        );
        let (journal, _) = routed.into_parts();
        assert_eq!(journal.recovers, 1);
        assert_eq!(journal.skips, 1);
    }

    #[test]
    fn skip_without_prior_route_does_not_claim_a_new_journal() {
        let journal = JournalMock::default();
        let legacy = LegacyMock::default();
        let mut routed = RoutedTranscriptMirror::new(journal, legacy, || Ok(true));
        routed
            .skip_checkpoint("conversation-a", &"checkpoint".into(), &"store".into())
            .unwrap();
        assert_eq!(routed.route_for("conversation-a"), None);
        let (journal, _) = routed.into_parts();
        assert_eq!(journal.claims, 0);
        assert_eq!(journal.skips, 0);
    }
}
