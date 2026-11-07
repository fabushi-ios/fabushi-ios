use crate::search::actor::{IndexCommand, TantivyWriterTask};
use crate::search::schema::{MessageSearchFields, SearchDocument, SearchHit, SearchQueryParams};
use gpui_telegram_core::error::{Result, TelegramError};
use std::path::Path;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, QueryParser, RangeQuery, TermQuery};
use tantivy::schema::{IndexRecordOption, Value};
use tantivy::{Index, IndexReader, ReloadPolicy, SnippetGenerator, TantivyDocument, Term};
use tokio::sync::{mpsc, oneshot};

/// High-throughput Tantivy full-text search engine.
///
/// Holds an async writer actor channel for batched disk commits and a thread-safe
/// `IndexReader` with `ReloadPolicy::OnCommitWithDelay` for sub-millisecond query execution.
#[derive(Clone)]
pub struct TantivyIndexer {
    fields: MessageSearchFields,
    index: Index,
    reader: IndexReader,
    writer_tx: mpsc::Sender<IndexCommand>,
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

        // Allocate 50MB for writer buffer
        let writer = index
            .writer(50_000_000)
            .map_err(|e| TelegramError::Internal(format!("Failed to create Tantivy writer: {e}")))?;

        let (tx, rx) = mpsc::channel(1000);
        let actor = TantivyWriterTask::new(fields.clone(), writer, reader.clone(), rx);
        actor.spawn();

        Ok(Self {
            fields,
            index,
            reader,
            writer_tx: tx,
        })
    }

    /// Queues a message for background indexing.
    pub async fn index_message(&self, doc: SearchDocument) -> Result<()> {
        self.writer_tx
            .send(IndexCommand::IndexMessage(doc))
            .await
            .map_err(|_| TelegramError::Internal("Tantivy writer actor channel closed".into()))
    }

    /// Synchronously queues a message for background indexing.
    pub fn index_message_sync(&self, doc: SearchDocument) -> Result<()> {
        self.writer_tx
            .blocking_send(IndexCommand::IndexMessage(doc))
            .map_err(|_| TelegramError::Internal("Tantivy writer actor channel closed".into()))
    }

    /// Queues a message deletion.
    pub async fn delete_message(&self, message_id: impl Into<String>) -> Result<()> {
        self.writer_tx
            .send(IndexCommand::DeleteMessage {
                message_id: message_id.into(),
            })
            .await
            .map_err(|_| TelegramError::Internal("Tantivy writer actor channel closed".into()))
    }

    /// Triggers an immediate commit on the writer actor and waits for completion.
    pub async fn commit(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.writer_tx
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
            if !chat_id.is_empty() {
                let term = Term::from_field_text(self.fields.f_chat_id, chat_id);
                clauses.push((Occur::Must, Box::new(TermQuery::new(term, IndexRecordOption::Basic))));
            }
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
