use serde::{Deserialize, Serialize};
use tantivy::schema::*;

/// Representation of an indexable and searchable message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchDocument {
    pub message_id: String,
    pub chat_id: String,
    pub sender_id: String,
    pub date: i64,
    pub content: String,
    pub media_type: Option<String>,
}

impl SearchDocument {
    pub fn new(
        message_id: impl Into<String>,
        chat_id: impl Into<String>,
        sender_id: impl Into<String>,
        date: i64,
        content: impl Into<String>,
        media_type: Option<String>,
    ) -> Self {
        Self {
            message_id: message_id.into(),
            chat_id: chat_id.into(),
            sender_id: sender_id.into(),
            date,
            content: content.into(),
            media_type,
        }
    }

    /// Accessor for message body (alias for content).
    pub fn body(&self) -> &str {
        &self.content
    }
}

/// Query parameters for scoped search across chats, senders, dates, and media types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQueryParams {
    pub query: String,
    pub chat_id: Option<String>,
    pub sender_id: Option<String>,
    pub media_type: Option<String>,
    pub from_date: Option<i64>,
    pub to_date: Option<i64>,
    pub limit: usize,
}

impl Default for SearchQueryParams {
    fn default() -> Self {
        Self {
            query: String::new(),
            chat_id: None,
            sender_id: None,
            media_type: None,
            from_date: None,
            to_date: None,
            limit: 20,
        }
    }
}

impl SearchQueryParams {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            ..Default::default()
        }
    }

    pub fn with_chat_id(mut self, chat_id: impl Into<String>) -> Self {
        self.chat_id = Some(chat_id.into());
        self
    }

    pub fn with_sender_id(mut self, sender_id: impl Into<String>) -> Self {
        self.sender_id = Some(sender_id.into());
        self
    }

    pub fn with_media_type(mut self, media_type: impl Into<String>) -> Self {
        self.media_type = Some(media_type.into());
        self
    }

    pub fn with_date_range(mut self, from_date: Option<i64>, to_date: Option<i64>) -> Self {
        self.from_date = from_date;
        self.to_date = to_date;
        self
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }
}

/// A search result hit with BM25 score and rich text highlighted snippet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub message_id: String,
    pub chat_id: String,
    pub sender_id: String,
    pub date: i64,
    pub snippet_html: String,
    pub score: f32,
}

/// Tantivy Schema & Field mapping for chat messages.
#[derive(Clone)]
pub struct MessageSearchFields {
    pub schema: Schema,
    pub f_message_id: Field,
    pub f_chat_id: Field,
    pub f_sender_id: Field,
    pub f_date: Field,
    pub f_body: Field,
    pub f_media_type: Field,
}

impl MessageSearchFields {
    pub fn new() -> Self {
        let mut builder = Schema::builder();
        let f_message_id = builder.add_text_field("message_id", STRING | STORED);
        let f_chat_id = builder.add_text_field("chat_id", STRING | STORED | FAST);
        let f_sender_id = builder.add_text_field("sender_id", STRING | STORED);
        let f_date = builder.add_i64_field("date", INDEXED | FAST | STORED);
        let f_body = builder.add_text_field("body", TEXT | STORED);
        let f_media_type = builder.add_text_field("media_type", STRING | STORED | FAST);

        Self {
            schema: builder.build(),
            f_message_id,
            f_chat_id,
            f_sender_id,
            f_date,
            f_body,
            f_media_type,
        }
    }
}

impl Default for MessageSearchFields {
    fn default() -> Self {
        Self::new()
    }
}
