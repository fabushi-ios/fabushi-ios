use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptMirrorRoute {
    Journal,
    Legacy,
}

pub trait TranscriptJournalPort<Checkpoint, Store>: Send + Sync {
    fn owns_conversation(&self, conversation_id: &str) -> Result<bool, String>;
    fn claim_conversation(&self, conversation_id: &str) -> Result<(), String>;
    fn recover(
        &self,
        conversation_id: &str,
        checkpoint: &Checkpoint,
        blob_store: &Store,
    ) -> Result<(), String>;
    fn prepare_checkpoint(
        &self,
        conversation_id: &str,
        checkpoint: &Checkpoint,
        blob_store: &Store,
        finalize_checkpoint: bool,
    ) -> Result<(), String>;
    fn commit_checkpoint(&self, conversation_id: &str) -> Result<(), String>;
    fn abort_checkpoint(&self, conversation_id: &str) -> Result<(), String>;
    fn skip_checkpoint(
        &self,
        conversation_id: &str,
        checkpoint: &Checkpoint,
        blob_store: &Store,
    ) -> Result<(), String>;
}

pub trait LegacyTranscriptMirrorPort<Checkpoint, Store>: Send + Sync {
    fn write(
        &self,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteSlot {
    Selecting,
    Ready(TranscriptMirrorRoute),
}

pub type JournalEnabledReader =
    Arc<dyn Fn() -> Result<bool, String> + Send + Sync + 'static>;

pub struct RoutedTranscriptMirror<Checkpoint, Store>
where
    Checkpoint: Clone,
    Store: Clone,
{
    journal: Arc<dyn TranscriptJournalPort<Checkpoint, Store>>,
    legacy: Arc<dyn LegacyTranscriptMirrorPort<Checkpoint, Store>>,
    is_journal_enabled: JournalEnabledReader,
    routes: Mutex<HashMap<String, RouteSlot>>,
    route_changed: Condvar,
    legacy_pending: Mutex<HashMap<String, LegacyPending<Checkpoint, Store>>>,
}

impl<Checkpoint, Store> RoutedTranscriptMirror<Checkpoint, Store>
where
    Checkpoint: Clone,
    Store: Clone,
{
    pub fn new(
        journal: Arc<dyn TranscriptJournalPort<Checkpoint, Store>>,
        legacy: Arc<dyn LegacyTranscriptMirrorPort<Checkpoint, Store>>,
        is_journal_enabled: JournalEnabledReader,
    ) -> Self {
        Self {
            journal,
            legacy,
            is_journal_enabled,
            routes: Mutex::new(HashMap::new()),
            route_changed: Condvar::new(),
            legacy_pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn route(&self, conversation_id: &str) -> Result<TranscriptMirrorRoute, String> {
        loop {
            let mut routes = self
                .routes
                .lock()
                .map_err(|_| "transcript mirror route cache is poisoned".to_string())?;
            match routes.get(conversation_id).copied() {
                Some(RouteSlot::Ready(route)) => return Ok(route),
                Some(RouteSlot::Selecting) => {
                    routes = self
                        .route_changed
                        .wait(routes)
                        .map_err(|_| "transcript mirror route cache is poisoned".to_string())?;
                    drop(routes);
                    continue;
                }
                None => {
                    routes.insert(conversation_id.to_string(), RouteSlot::Selecting);
                    drop(routes);
                    break;
                }
            }
        }

        let selected = self.select_route(conversation_id);
        let mut routes = self
            .routes
            .lock()
            .map_err(|_| "transcript mirror route cache is poisoned".to_string())?;
        match selected {
            Ok(route) => {
                routes.insert(
                    conversation_id.to_string(),
                    RouteSlot::Ready(route),
                );
                self.route_changed.notify_all();
                Ok(route)
            }
            Err(error) => {
                routes.remove(conversation_id);
                self.route_changed.notify_all();
                Err(error)
            }
        }
    }

    fn select_route(&self, conversation_id: &str) -> Result<TranscriptMirrorRoute, String> {
        if self.journal.owns_conversation(conversation_id)? {
            return Ok(TranscriptMirrorRoute::Journal);
        }
        if !(self.is_journal_enabled)()? {
            return Ok(TranscriptMirrorRoute::Legacy);
        }
        self.journal.claim_conversation(conversation_id)?;
        Ok(TranscriptMirrorRoute::Journal)
    }

    pub fn recover(
        &self,
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
        &self,
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
            self.legacy_pending
                .lock()
                .map_err(|_| "transcript mirror legacy pending map is poisoned".to_string())?
                .insert(
                    conversation_id.to_string(),
                    LegacyPending {
                        checkpoint: checkpoint.clone(),
                        blob_store: blob_store.clone(),
                    },
                );
        }
        Ok(())
    }

    pub fn commit_checkpoint(
        &self,
        conversation_id: &str,
        state_blob_id: &[u8],
    ) -> Result<(), String> {
        if self.route(conversation_id)? == TranscriptMirrorRoute::Journal {
            return self.journal.commit_checkpoint(conversation_id);
        }

        let pending = self
            .legacy_pending
            .lock()
            .map_err(|_| "transcript mirror legacy pending map is poisoned".to_string())?
            .remove(conversation_id);
        if let Some(pending) = pending {
            // Frozen Grok contract: the legacy mirror is observational. Its
            // failure cannot roll back the already durable Agent checkpoint.
            let _ = self.legacy.write(
                conversation_id,
                &pending.checkpoint,
                &pending.blob_store,
                state_blob_id,
            );
        }
        Ok(())
    }

    pub fn abort_checkpoint(&self, conversation_id: &str) -> Result<(), String> {
        if self.route(conversation_id)? == TranscriptMirrorRoute::Journal {
            return self.journal.abort_checkpoint(conversation_id);
        }
        self.legacy_pending
            .lock()
            .map_err(|_| "transcript mirror legacy pending map is poisoned".to_string())?
            .remove(conversation_id);
        Ok(())
    }

    pub fn skip_checkpoint(
        &self,
        conversation_id: &str,
        checkpoint: &Checkpoint,
        blob_store: &Store,
    ) -> Result<(), String> {
        let Some((selected, recover_owned_journal)) =
            self.route_for_skip(conversation_id)?
        else {
            self.legacy_pending
                .lock()
                .map_err(|_| "transcript mirror legacy pending map is poisoned".to_string())?
                .remove(conversation_id);
            return Ok(());
        };

        match selected {
            TranscriptMirrorRoute::Journal => {
                if recover_owned_journal {
                    self.journal
                        .recover(conversation_id, checkpoint, blob_store)?;
                }
                self.journal
                    .skip_checkpoint(conversation_id, checkpoint, blob_store)
            }
            TranscriptMirrorRoute::Legacy => {
                self.legacy_pending
                    .lock()
                    .map_err(|_| "transcript mirror legacy pending map is poisoned".to_string())?
                    .remove(conversation_id);
                Ok(())
            }
        }
    }

    fn route_for_skip(
        &self,
        conversation_id: &str,
    ) -> Result<Option<(TranscriptMirrorRoute, bool)>, String> {
        loop {
            let mut routes = self
                .routes
                .lock()
                .map_err(|_| "transcript mirror route cache is poisoned".to_string())?;
            match routes.get(conversation_id).copied() {
                Some(RouteSlot::Ready(route)) => return Ok(Some((route, false))),
                Some(RouteSlot::Selecting) => {
                    routes = self
                        .route_changed
                        .wait(routes)
                        .map_err(|_| "transcript mirror route cache is poisoned".to_string())?;
                    drop(routes);
                    continue;
                }
                None => {
                    routes.insert(conversation_id.to_string(), RouteSlot::Selecting);
                    drop(routes);
                    break;
                }
            }
        }

        let owned = self.journal.owns_conversation(conversation_id);
        let mut routes = self
            .routes
            .lock()
            .map_err(|_| "transcript mirror route cache is poisoned".to_string())?;
        match owned {
            Ok(true) => {
                routes.insert(
                    conversation_id.to_string(),
                    RouteSlot::Ready(TranscriptMirrorRoute::Journal),
                );
                self.route_changed.notify_all();
                Ok(Some((TranscriptMirrorRoute::Journal, true)))
            }
            Ok(false) => {
                routes.remove(conversation_id);
                self.route_changed.notify_all();
                Ok(None)
            }
            Err(error) => {
                routes.remove(conversation_id);
                self.route_changed.notify_all();
                Err(error)
            }
        }
    }

    pub fn cached_route(&self, conversation_id: &str) -> Option<TranscriptMirrorRoute> {
        self.routes
            .lock()
            .ok()
            .and_then(|routes| match routes.get(conversation_id).copied() {
                Some(RouteSlot::Ready(route)) => Some(route),
                _ => None,
            })
    }

    pub fn legacy_pending_count(&self) -> usize {
        self.legacy_pending
            .lock()
            .map(|pending| pending.len())
            .unwrap_or_default()
    }
}
