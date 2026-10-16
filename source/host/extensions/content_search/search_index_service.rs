use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use super::search_index_db::{remove_search_index_files, SearchDbError};
use super::search_index_worker::SearchIndexWorker;
use super::search_index_writer::SearchIndexJob;

pub const MAX_INDEX_REBUILDS: usize = 3;
pub const MAX_WORKER_RESPAWNS: usize = 3;
pub const MAX_FAILED_JOB_RECONCILES: usize = 3;
pub const SEARCH_INDEX_DISPOSE_TIMEOUT_MS: u64 = 2_000;

struct ServiceState {
    worker: Option<SearchIndexWorker>,
    rebuilds: usize,
    respawns: usize,
    failed_job_reconciles: usize,
    unavailable: bool,
    disposed: bool,
}

pub struct SearchIndexService {
    index_path: PathBuf,
    source_path: PathBuf,
    state: Mutex<ServiceState>,
}

impl SearchIndexService {
    pub fn start(index_path: impl Into<PathBuf>, source_path: impl Into<PathBuf>) -> Result<Self, SearchDbError> {
        let service = Self {
            index_path: index_path.into(),
            source_path: source_path.into(),
            state: Mutex::new(ServiceState {
                worker: None,
                rebuilds: 0,
                respawns: 0,
                failed_job_reconciles: 0,
                unavailable: false,
                disposed: false,
            }),
        };
        service.execute(SearchIndexJob::Reconcile)?;
        Ok(service)
    }

    pub fn execute(&self, job: SearchIndexJob) -> Result<Value, SearchDbError> {
        let mut state = self.state.lock()
            .map_err(|_| SearchDbError::Unavailable("search-index service lock poisoned".into()))?;
        if state.disposed {
            return Err(SearchDbError::Unavailable("search-index service disposed".into()));
        }
        if state.unavailable {
            return Err(SearchDbError::Unavailable("search-index service unavailable".into()));
        }

        for _ in 0..16 {
            if state.worker.is_none() {
                match SearchIndexWorker::spawn(self.index_path.clone(), self.source_path.clone()) {
                    Ok(worker) => state.worker = Some(worker),
                    Err(SearchDbError::Corrupt(message) | SearchDbError::Rebuild(message)) => {
                        self.rebuild_locked(&mut state, &message)?;
                        continue;
                    }
                    Err(error) => {
                        self.respawn_locked(&mut state, &error.to_string())?;
                        continue;
                    }
                }
            }

            let result = state.worker.as_ref().expect("worker installed").run(job.clone());
            match result {
                Ok(value) => {
                    state.failed_job_reconciles = 0;
                    return Ok(value);
                }
                Err(SearchDbError::Corrupt(message) | SearchDbError::Rebuild(message)) => {
                    self.rebuild_locked(&mut state, &message)?;
                }
                Err(SearchDbError::Unavailable(message)) => {
                    self.respawn_locked(&mut state, &message)?;
                    self.reconcile_locked(&mut state)?;
                }
                Err(SearchDbError::Other(message)) => {
                    state.failed_job_reconciles += 1;
                    if state.failed_job_reconciles > MAX_FAILED_JOB_RECONCILES {
                        state.unavailable = true;
                        return Err(SearchDbError::Unavailable(format!(
                            "content-search failed-job reconcile limit exceeded: {message}"
                        )));
                    }
                    self.reconcile_locked(&mut state)?;
                }
            }
        }
        state.unavailable = true;
        Err(SearchDbError::Unavailable("content-search recovery attempts exhausted".into()))
    }

    pub fn status(&self) -> Value {
        let Ok(state) = self.state.lock() else {
            return json!({"ready": false, "unavailable": true});
        };
        json!({
            "ready": !state.unavailable && !state.disposed,
            "unavailable": state.unavailable,
            "disposed": state.disposed,
            "indexRebuilds": state.rebuilds,
            "workerRespawns": state.respawns,
            "failedJobReconciles": state.failed_job_reconciles,
        })
    }

    pub fn dispose(&self) {
        let Ok(mut state) = self.state.lock() else { return };
        if state.disposed { return }
        state.disposed = true;
        if let Some(worker) = state.worker.take() {
            let _ = worker.stop(Duration::from_millis(SEARCH_INDEX_DISPOSE_TIMEOUT_MS));
        }
    }

    fn rebuild_locked(&self, state: &mut ServiceState, reason: &str) -> Result<(), SearchDbError> {
        state.rebuilds += 1;
        if state.rebuilds > MAX_INDEX_REBUILDS {
            state.unavailable = true;
            return Err(SearchDbError::Unavailable(format!(
                "content-search corruption rebuild limit exceeded: {reason}"
            )));
        }
        if let Some(worker) = state.worker.take() {
            let _ = worker.stop(Duration::from_millis(SEARCH_INDEX_DISPOSE_TIMEOUT_MS));
        }
        remove_search_index_files(&self.index_path)?;
        state.worker = Some(SearchIndexWorker::spawn(
            self.index_path.clone(),
            self.source_path.clone(),
        )?);
        self.reconcile_locked(state)
    }

    fn respawn_locked(&self, state: &mut ServiceState, reason: &str) -> Result<(), SearchDbError> {
        state.respawns += 1;
        if state.respawns > MAX_WORKER_RESPAWNS {
            state.unavailable = true;
            return Err(SearchDbError::Unavailable(format!(
                "content-search worker respawn limit exceeded: {reason}"
            )));
        }
        if let Some(worker) = state.worker.take() {
            let _ = worker.stop(Duration::from_millis(SEARCH_INDEX_DISPOSE_TIMEOUT_MS));
        }
        state.worker = Some(SearchIndexWorker::spawn(
            self.index_path.clone(),
            self.source_path.clone(),
        )?);
        Ok(())
    }

    fn reconcile_locked(&self, state: &mut ServiceState) -> Result<(), SearchDbError> {
        let Some(worker) = state.worker.as_ref() else {
            return Err(SearchDbError::Unavailable("content-search worker is missing".into()));
        };
        worker.run(SearchIndexJob::Reconcile).map(|_| ())
    }

    #[cfg(test)]
    pub fn index_path(&self) -> &Path {
        &self.index_path
    }
}

impl Drop for SearchIndexService {
    fn drop(&mut self) {
        self.dispose();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::search_index_writer::IndexEntry;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "fabushi-content-search-service-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()
        ))
    }

    fn entry(id: &str, body: &str) -> IndexEntry {
        IndexEntry {
            id: id.into(),
            kind: "message".into(),
            timestamp_ms: 1,
            fields: BTreeMap::from([
                ("role".into(), Value::String("user".into())),
                ("body".into(), Value::String(body.into())),
            ]),
        }
    }

    #[test]
    fn durable_source_reconciles_after_index_corruption() {
        let root = root();
        let index = root.join("search-index.db");
        let source = root.join("search-source.db");
        {
            let service = SearchIndexService::start(&index, &source).unwrap();
            service.execute(SearchIndexJob::ReplaceScope {
                scope_id: "account-a\u{1f}conversation-1".into(),
                entries: vec![entry("m1", "Global Dharma survives rebuild")],
            }).unwrap();
            service.dispose();
        }
        fs::write(&index, b"not-a-sqlite-database").unwrap();
        let service = SearchIndexService::start(&index, &source).unwrap();
        let value = service.execute(SearchIndexJob::SearchMessages {
            scope_id: "account-a\u{1f}conversation-1".into(),
            query: "Dharma".into(),
            limit: 20,
        }).unwrap();
        assert_eq!(value["results"].as_array().unwrap().len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn account_scopes_do_not_leak_results() {
        let root = root();
        let service = SearchIndexService::start(root.join("search-index.db"), root.join("search-source.db")).unwrap();
        service.execute(SearchIndexJob::ReplaceScope {
            scope_id: "account-a\u{1f}conversation-1".into(),
            entries: vec![entry("m1", "private alpha")],
        }).unwrap();
        service.execute(SearchIndexJob::ReplaceScope {
            scope_id: "account-b\u{1f}conversation-1".into(),
            entries: vec![entry("m2", "private beta")],
        }).unwrap();
        let value = service.execute(SearchIndexJob::SearchMessages {
            scope_id: "account-b\u{1f}conversation-1".into(),
            query: "alpha".into(),
            limit: 20,
        }).unwrap();
        assert!(value["results"].as_array().unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }
}
