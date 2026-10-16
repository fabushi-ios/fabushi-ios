use serde_json::Value;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::search_index_db::SearchDbError;
use super::search_index_writer::{SearchIndexJob, SearchIndexWriter};

pub struct SearchIndexWorker {
    tx: mpsc::Sender<WorkerEnvelope>,
    join: Option<JoinHandle<()>>,
}

struct WorkerEnvelope {
    job: SearchIndexJob,
    reply: mpsc::Sender<Result<Value, SearchDbError>>,
}

impl SearchIndexWorker {
    pub fn spawn(index_path: PathBuf, source_path: PathBuf) -> Result<Self, SearchDbError> {
        let (tx, rx) = mpsc::channel::<WorkerEnvelope>();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name("fabushi-search-index-worker".into())
            .spawn(move || {
                let mut writer = match SearchIndexWriter::open(&index_path, &source_path) {
                    Ok(writer) => {
                        let _ = ready_tx.send(Ok(()));
                        writer
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                while let Ok(envelope) = rx.recv() {
                    let result = writer.apply(envelope.job);
                    let _ = envelope.reply.send(result);
                }
            })
            .map_err(|error| SearchDbError::Unavailable(format!("spawn search-index worker: {error}")))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { tx, join: Some(join) }),
            Ok(Err(error)) => {
                let _ = join.join();
                Err(error)
            }
            Err(error) => {
                let _ = join.join();
                Err(SearchDbError::Unavailable(format!(
                    "search-index worker startup channel closed: {error}"
                )))
            }
        }
    }

    pub fn run(&self, job: SearchIndexJob) -> Result<Value, SearchDbError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(WorkerEnvelope { job, reply: reply_tx })
            .map_err(|error| SearchDbError::Unavailable(format!("search-index worker send failed: {error}")))?;
        reply_rx
            .recv()
            .map_err(|error| SearchDbError::Unavailable(format!("search-index worker reply failed: {error}")))?
    }

    pub fn stop(mut self, timeout: Duration) -> bool {
        drop(self.tx);
        let Some(join) = self.join.take() else { return true };
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let _ = join.join();
            let _ = done_tx.send(());
        });
        done_rx.recv_timeout(timeout).is_ok()
    }
}
