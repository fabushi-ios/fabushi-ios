use crate::search::schema::{MessageSearchFields, SearchDocument};
use std::time::Duration;
use tantivy::{IndexReader, IndexWriter, TantivyDocument, Term};
use tokio::sync::{mpsc, oneshot};

/// Commands sent to the background Tantivy writer actor.
pub enum IndexCommand {
    IndexMessage(SearchDocument),
    DeleteMessage {
        message_id: String,
    },
    Commit {
        reply: Option<oneshot::Sender<()>>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

/// Dedicated asynchronous background task that handles Tantivy writes with batched commits.
/// Eliminates `DirectoryLockBusy` contention from multi-threaded HTTP handlers.
pub struct TantivyWriterTask {
    fields: MessageSearchFields,
    writer: IndexWriter,
    reader: IndexReader,
    receiver: mpsc::Receiver<IndexCommand>,
}

impl TantivyWriterTask {
    pub fn new(
        fields: MessageSearchFields,
        writer: IndexWriter,
        reader: IndexReader,
        receiver: mpsc::Receiver<IndexCommand>,
    ) -> Self {
        Self {
            fields,
            writer,
            reader,
            receiver,
        }
    }

    /// Spawns the writer actor onto the Tokio runtime.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(self.run())
    }

    /// Background event loop handling commands and batched commits.
    pub async fn run(mut self) {
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        let mut pending_commits = 0usize;

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if pending_commits > 0 {
                        if let Err(e) = self.writer.commit() {
                            tracing::error!("Tantivy periodic commit failed: {e}");
                        } else {
                            pending_commits = 0;
                            let _ = self.reader.reload();
                        }
                    }
                }
                cmd = self.receiver.recv() => {
                    match cmd {
                        Some(IndexCommand::IndexMessage(doc)) => {
                            // Atomic upsert: delete existing doc with message_id then insert
                            let term = Term::from_field_text(self.fields.f_message_id, &doc.message_id);
                            self.writer.delete_term(term);

                            let mut tantivy_doc = TantivyDocument::default();
                            tantivy_doc.add_text(self.fields.f_message_id, &doc.message_id);
                            tantivy_doc.add_text(self.fields.f_chat_id, &doc.chat_id);
                            tantivy_doc.add_text(self.fields.f_sender_id, &doc.sender_id);
                            tantivy_doc.add_i64(self.fields.f_date, doc.date);
                            tantivy_doc.add_text(self.fields.f_body, &doc.content);
                            if let Some(ref m_type) = doc.media_type {
                                tantivy_doc.add_text(self.fields.f_media_type, m_type);
                            }

                            if let Err(e) = self.writer.add_document(tantivy_doc) {
                                tracing::error!("Tantivy add_document failed: {e}");
                            } else {
                                pending_commits += 1;
                                if pending_commits >= 100 {
                                    if let Err(e) = self.writer.commit() {
                                        tracing::error!("Tantivy batch commit failed: {e}");
                                    } else {
                                        pending_commits = 0;
                                        let _ = self.reader.reload();
                                    }
                                }
                            }
                        }
                        Some(IndexCommand::DeleteMessage { message_id }) => {
                            let term = Term::from_field_text(self.fields.f_message_id, &message_id);
                            self.writer.delete_term(term);
                            pending_commits += 1;
                            if pending_commits >= 100 {
                                if let Err(e) = self.writer.commit() {
                                    tracing::error!("Tantivy batch commit on delete failed: {e}");
                                } else {
                                    pending_commits = 0;
                                    let _ = self.reader.reload();
                                }
                            }
                        }
                        Some(IndexCommand::Commit { reply }) => {
                            if pending_commits > 0 {
                                if let Err(e) = self.writer.commit() {
                                    tracing::error!("Tantivy explicit commit failed: {e}");
                                } else {
                                    pending_commits = 0;
                                    let _ = self.reader.reload();
                                }
                            }
                            if let Some(r) = reply {
                                let _ = r.send(());
                            }
                        }
                        Some(IndexCommand::Shutdown { reply }) => {
                            if let Err(e) = self.writer.commit() {
                                tracing::error!("Tantivy commit during shutdown failed: {e}");
                            } else {
                                let _ = self.reader.reload();
                            }
                            let _ = reply.send(());
                            break;
                        }
                        None => break,
                    }
                }
            }
        }
    }
}
