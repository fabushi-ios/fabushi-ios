use crate::search::actor::{IndexCommand, TantivyWriterTask};
use crate::search::schema::{MessageSearchFields, SearchDocument, SearchHit, SearchQueryParams};
use gpui_telegram_core::error::{Result, TelegramError};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, QueryParser, RangeQuery, TermQuery};
use tantivy::schema::{IndexRecordOption, Value};
use tantivy::{Index, IndexReader, ReloadPolicy, SnippetGenerator, TantivyDocument, Term};
use tokio::sync::{mpsc, oneshot};

struct TantivyWriterGuard {
    tx: Mutex<Option<mpsc::Sender<IndexCommand>>>,
    join: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Drop for TantivyWriterGuard {
    fn drop(&mut self) {
        let tx_opt = self.tx.lock().ok().and_then(|mut g| g.take());
        if let Some(tx) = tx_opt {
            let (reply_tx, _reply_rx) = oneshot::channel();
            match tx.try_send(IndexCommand::Shutdown { reply: reply_tx }) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(cmd)) => {
                    if tokio::runtime::Handle::try_current().is_err() {
                        let _ = tx.blocking_send(cmd);
                    } else {
                        let mut cmd = cmd;
                        for _ in 0..50 {
                            std::thread::sleep(std::time::Duration::from_millis(2));
                            match tx.try_send(cmd) {
                                Ok(()) => break,
                                Err(mpsc::error::TrySendError::Full(ret)) => cmd = ret,
                                Err(_) => break,
                            }
                        }
                    }
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {}
            }
            drop(tx);
        }
        if let Ok(mut g) = self.join.lock() {
            if let Some(handle) = g.take() {
                if std::thread::current().id() != handle.thread().id() {
                    let _ = handle.join();
                }
            }
        }
    }
}

/// High-throughput Tantivy full-text search engine.
///
/// Holds an async writer actor channel for batched disk commits and a thread-safe
/// `IndexReader` with `ReloadPolicy::OnCommitWithDelay` for sub-millisecond query execution.
#[derive(Clone)]
pub struct TantivyIndexer {
    fields: MessageSearchFields,
    index: Index,
    reader: IndexReader,
    writer_guard: Arc<TantivyWriterGuard>,
}

impl std::fmt::Debug for TantivyIndexer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TantivyIndexer").finish_non_exhaustive()
    }
}

impl TantivyIndexer {
    /// Initializes an ephemeral in-memory Tantivy index (ideal for unit tests).
    pub fn in_ram() -> Result<Self> {
        let fields = MessageSearchFields::new();
        let index = Index::create_in_ram(fields.schema.clone());
        Self::init_with_index(fields, index)
    }

    /// Initializes or opens a persistent Tantivy index at the given directory path.
    pub fn open_or_create(dir_path: impl AsRef<Path>) -> Result<Self> {
        let fields = MessageSearchFields::new();
        let path = dir_path.as_ref();
        if !path.exists() {
            std::fs::create_dir_all(path)
                .map_err(|e| TelegramError::Internal(format!("Failed to create index dir: {e}")))?;
        }

        let index = match Index::open_in_dir(path) {
            Ok(idx) => idx,
            Err(_) => Index::create_in_dir(path, fields.schema.clone())
                .map_err(|e| TelegramError::Internal(format!("Failed to create Tantivy index: {e}")))?,
        };

        Self::init_with_index(fields, index)
    }

    /// Common initialization helper that starts the reader and writer actor.
    fn init_with_index(fields: MessageSearchFields, index: Index) -> Result<Self> {
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e| TelegramError::Internal(format!("Failed to create Tantivy reader: {e}")))?;

        // Allocate 50MB for writer buffer; retry briefly if a closing writer handle is still releasing its dir lock.
        let mut last_err = None;
        let mut writer_opt = None;
        for attempt in 0..25 {
            match index.writer(50_000_000) {
                Ok(w) => {
                    writer_opt = Some(w);
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    if attempt < 24 {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                }
            }
        }
        let writer = writer_opt.ok_or_else(|| {
            TelegramError::Internal(format!(
                "Failed to create Tantivy writer: {}",
                last_err.map(|e| e.to_string()).unwrap_or_default()
            ))
        })?;

        let (tx, rx) = mpsc::channel(1000);
        let actor = TantivyWriterTask::new(fields.clone(), writer, reader.clone(), rx);
        let join = actor.spawn();

        Ok(Self {
            fields,
            index,
            reader,
            writer_guard: Arc::new(TantivyWriterGuard {
                tx: Mutex::new(Some(tx)),
                join: Mutex::new(join),
            }),
        })
    }

    fn writer_sender(&self) -> Result<mpsc::Sender<IndexCommand>> {
        self.writer_guard
            .tx
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .ok_or_else(|| TelegramError::Internal("Tantivy writer actor closed".into()))
    }

    pub fn doc_count(&self) -> u64 {
        self.reader.searcher().num_docs()
    }

    pub fn index_message_nonblocking(&self, doc: SearchDocument) {
        if let Ok(tx) = self.writer_sender() {
            let _ = tx.try_send(IndexCommand::IndexMessage(doc));
        }
    }

    pub fn delete_message_nonblocking(&self, message_id: impl Into<String>) {
        if let Ok(tx) = self.writer_sender() {
            let _ = tx.try_send(IndexCommand::DeleteMessage {
                message_id: message_id.into(),
            });
        }
    }

    pub fn delete_chat_message_nonblocking(
        &self,
        chat_id: impl Into<String>,
        message_id: impl Into<String>,
    ) {
        if let Ok(tx) = self.writer_sender() {
            let _ = tx.try_send(IndexCommand::DeleteChatMessage {
                chat_id: chat_id.into(),
                message_id: message_id.into(),
            });
        }
    }

    pub fn commit_nonblocking(&self) {
        if let Ok(tx) = self.writer_sender() {
            let _ = tx.try_send(IndexCommand::Commit { reply: None });
        }
    }

    /// Queues a message for background indexing.
    pub async fn index_message(&self, doc: SearchDocument) -> Result<()> {
        self.writer_sender()?
            .send(IndexCommand::IndexMessage(doc))
            .await
            .map_err(|_| TelegramError::Internal("Tantivy writer actor channel closed".into()))
    }

    /// Synchronously queues a message for background indexing without panicking inside Tokio.
    pub fn index_message_sync(&self, doc: SearchDocument) -> Result<()> {
        let tx = self.writer_sender()?;
        if tokio::runtime::Handle::try_current().is_err() {
            return tx
                .blocking_send(IndexCommand::IndexMessage(doc))
                .map_err(|_| TelegramError::Internal("Tantivy writer actor channel closed".into()));
        }
        let mut cmd = IndexCommand::IndexMessage(doc);
        for _ in 0..500 {
            match tx.try_send(cmd) {
                Ok(()) => return Ok(()),
                Err(mpsc::error::TrySendError::Full(returned)) => {
                    cmd = returned;
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    return Err(TelegramError::Internal("Tantivy writer actor channel closed".into()));
                }
            }
        }
        Err(TelegramError::Internal(
            "Tantivy writer actor backpressure timeout".into(),
        ))
    }

    /// Queues a message deletion.
    pub async fn delete_message(&self, message_id: impl Into<String>) -> Result<()> {
        self.writer_sender()?
            .send(IndexCommand::DeleteMessage {
                message_id: message_id.into(),
            })
            .await
            .map_err(|_| TelegramError::Internal("Tantivy writer actor channel closed".into()))
    }

    /// Queues deletion of all indexed messages belonging to `chat_id`.
    pub async fn delete_chat(&self, chat_id: impl Into<String>) -> Result<()> {
        self.writer_sender()?
            .send(IndexCommand::DeleteChat {
                chat_id: chat_id.into(),
            })
            .await
            .map_err(|_| TelegramError::Internal("Tantivy writer actor channel closed".into()))
    }

    /// Triggers an immediate commit on the writer actor and waits for completion.
    pub async fn commit(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.writer_sender()?
            .send(IndexCommand::Commit { reply: Some(tx) })
            .await
            .map_err(|_| TelegramError::Internal("Tantivy writer actor channel closed".into()))?;

        rx.await
            .map_err(|_| TelegramError::Internal("Tantivy commit response dropped".into()))?;

        // Explicitly reload reader to guarantee instant visibility after manual commit
        self.reader
            .reload()
            .map_err(|e| TelegramError::Internal(format!("Failed to reload reader after commit: {e}")))?;

        Ok(())
    }

    /// Explicitly reloads the index reader to see all committed documents immediately.
    pub fn reload(&self) -> Result<()> {
        self.reader
            .reload()
            .map_err(|e| TelegramError::Internal(format!("Failed to reload Tantivy reader: {e}")))
    }

    /// Searches messages matching the given query parameters, returning scored hits and snippets.
    pub fn search_hits(&self, params: &SearchQueryParams) -> Result<Vec<SearchHit>> {
        let searcher = self.reader.searcher();
        let query_parser = QueryParser::for_index(&self.index, vec![self.fields.f_body]);
        let parsed_query = query_parser
            .parse_query(&params.query)
            .map_err(|e| TelegramError::Validation(format!("Invalid search query: {e}")))?;

        let mut snippet_gen = SnippetGenerator::create(&searcher, &*parsed_query, self.fields.f_body)
            .map_err(|e| TelegramError::Internal(format!("Snippet generator creation failed: {e}")))?;
        snippet_gen.set_max_num_chars(160);

        let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![(Occur::Must, parsed_query)];

        if let Some(ref chat_id) = params.chat_id {
            if chat_id.trim().is_empty() {
                return Err(TelegramError::Validation(
                    "chat_id filter cannot be empty".into(),
                ));
            }
            let term = Term::from_field_text(self.fields.f_chat_id, chat_id);
            clauses.push((Occur::Must, Box::new(TermQuery::new(term, IndexRecordOption::Basic))));
        }

        if let Some(ref sender_id) = params.sender_id {
            if !sender_id.is_empty() {
                let term = Term::from_field_text(self.fields.f_sender_id, sender_id);
                clauses.push((Occur::Must, Box::new(TermQuery::new(term, IndexRecordOption::Basic))));
            }
        }

        if let Some(ref media_type) = params.media_type {
            if !media_type.is_empty() {
                let term = Term::from_field_text(self.fields.f_media_type, media_type);
                clauses.push((Occur::Must, Box::new(TermQuery::new(term, IndexRecordOption::Basic))));
            }
        }

        if params.from_date.is_some() || params.to_date.is_some() {
            let lower = params.from_date.unwrap_or(i64::MIN);
            let upper = params.to_date.unwrap_or(i64::MAX);
            let upper_exclusive = if upper == i64::MAX { upper } else { upper + 1 };
            let range = RangeQuery::new_i64("date".to_string(), lower..upper_exclusive);
            clauses.push((Occur::Must, Box::new(range)));
        }

        let boolean_query = BooleanQuery::new(clauses);
        let top_docs = searcher
            .search(
                &boolean_query,
                &TopDocs::with_limit(params.limit.max(1).min(100)),
            )
            .map_err(|e| TelegramError::Internal(format!("Search execution failed: {e}")))?;

        let mut hits = Vec::with_capacity(top_docs.len());
        for (score, doc_addr) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_addr)
                .map_err(|e| TelegramError::Internal(format!("Doc retrieval failed: {e}")))?;

            let message_id = doc
                .get_first(self.fields.f_message_id)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let chat_id = doc
                .get_first(self.fields.f_chat_id)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let sender_id = doc
                .get_first(self.fields.f_sender_id)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let date = doc
                .get_first(self.fields.f_date)
                .and_then(|v| v.as_i64())
                .unwrap_or_default();

            let snippet = snippet_gen.snippet_from_doc(&doc);
            // Convert <b> tags from Tantivy snippet to <mark> for Telegram rich text highlights
            let snippet_html = snippet
                .to_html()
                .replace("<b>", "<mark>")
                .replace("</b>", "</mark>");

            hits.push(SearchHit {
                message_id,
                chat_id,
                sender_id,
                date,
                snippet_html,
                score,
            });
        }

        Ok(hits)
    }

    /// Searches messages scoped strictly to `allowed_chat_ids` inside the Tantivy `BooleanQuery`
    /// so unauthorized chats never starve `TopDocs::with_limit`.
    pub fn search_hits_for_chats(
        &self,
        query: &str,
        allowed_chat_ids: &[i64],
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        if allowed_chat_ids.is_empty() || query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let searcher = self.reader.searcher();
        let query_parser = QueryParser::for_index(&self.index, vec![self.fields.f_body]);
        let parsed_query = query_parser
            .parse_query(query)
            .map_err(|e| TelegramError::Validation(format!("Invalid search query: {e}")))?;

        let mut snippet_gen = SnippetGenerator::create(&searcher, &*parsed_query, self.fields.f_body)
            .map_err(|e| TelegramError::Internal(format!("Snippet generator creation failed: {e}")))?;
        snippet_gen.set_max_num_chars(160);

        let chat_clauses: Vec<(Occur, Box<dyn Query>)> = allowed_chat_ids
            .iter()
            .map(|cid| {
                let term = Term::from_field_text(self.fields.f_chat_id, &cid.to_string());
                (
                    Occur::Should,
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic)) as Box<dyn Query>,
                )
            })
            .collect();
        let chat_filter = BooleanQuery::new(chat_clauses);
        let boolean_query = BooleanQuery::new(vec![
            (Occur::Must, parsed_query),
            (Occur::Must, Box::new(chat_filter)),
        ]);

        let top_docs = searcher
            .search(&boolean_query, &TopDocs::with_limit(limit.clamp(1, 200)))
            .map_err(|e| TelegramError::Internal(format!("Search execution failed: {e}")))?;

        let mut hits = Vec::with_capacity(top_docs.len());
        for (score, doc_addr) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_addr)
                .map_err(|e| TelegramError::Internal(format!("Doc retrieval failed: {e}")))?;
            let message_id = doc
                .get_first(self.fields.f_message_id)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let chat_id = doc
                .get_first(self.fields.f_chat_id)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let sender_id = doc
                .get_first(self.fields.f_sender_id)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let date = doc
                .get_first(self.fields.f_date)
                .and_then(|v| v.as_i64())
                .unwrap_or_default();
            let snippet = snippet_gen.snippet_from_doc(&doc);
            let snippet_html = snippet
                .to_html()
                .replace("<b>", "<mark>")
                .replace("</b>", "</mark>");
            hits.push(SearchHit {
                message_id,
                chat_id,
                sender_id,
                date,
                snippet_html,
                score,
            });
        }
        Ok(hits)
    }

    /// Legacy / simplified search interface for backward compatibility.
    pub fn search(&self, query: &str, chat_id: Option<&str>) -> Result<Vec<SearchDocument>> {
        let mut params = SearchQueryParams::new(query);
        if let Some(cid) = chat_id {
            params = params.with_chat_id(cid);
        }

        let searcher = self.reader.searcher();
        let query_parser = QueryParser::for_index(&self.index, vec![self.fields.f_body]);
        let parsed_query = query_parser
            .parse_query(&params.query)
            .map_err(|e| TelegramError::Validation(format!("Invalid search query: {e}")))?;

        let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![(Occur::Must, parsed_query)];

        if let Some(ref cid) = params.chat_id {
            if !cid.is_empty() {
                let term = Term::from_field_text(self.fields.f_chat_id, cid);
                clauses.push((Occur::Must, Box::new(TermQuery::new(term, IndexRecordOption::Basic))));
            }
        }

        let boolean_query = BooleanQuery::new(clauses);
        let top_docs = searcher
            .search(&boolean_query, &TopDocs::with_limit(100))
            .map_err(|e| TelegramError::Internal(format!("Search execution failed: {e}")))?;

        let mut docs = Vec::with_capacity(top_docs.len());
        for (_score, doc_addr) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_addr)
                .map_err(|e| TelegramError::Internal(format!("Doc retrieval failed: {e}")))?;

            let message_id = doc
                .get_first(self.fields.f_message_id)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let chat_id = doc
                .get_first(self.fields.f_chat_id)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let sender_id = doc
                .get_first(self.fields.f_sender_id)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let date = doc
                .get_first(self.fields.f_date)
                .and_then(|v| v.as_i64())
                .unwrap_or_default();
            let content = doc
                .get_first(self.fields.f_body)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let media_type = doc
                .get_first(self.fields.f_media_type)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            docs.push(SearchDocument {
                message_id,
                chat_id,
                sender_id,
                date,
                content,
                media_type,
            });
        }

        Ok(docs)
    }
}

impl Default for TantivyIndexer {
    fn default() -> Self {
        Self::in_ram().expect("Default in-memory Tantivy indexer must initialize")
    }
}
