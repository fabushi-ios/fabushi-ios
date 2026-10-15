use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::transcript_mutation_events::{publish_transcript_mutation, TranscriptMutation};

use super::agent_content_search::{bounded_limit, required_text, scope_key};
use super::search_index_db::SEARCH_INDEX_FILENAME;
use super::search_index_service::SearchIndexService;
use super::search_index_writer::{IndexEntry, SearchIndexJob};

pub const CONTENT_SEARCH_EXTENSION_ID: &str = "content-search";
pub const SOURCE_INDEX_FILENAME: &str = "search-source.db";

pub struct MobileContentSearchOwner {
    service: SearchIndexService,
}

impl MobileContentSearchOwner {
    pub fn start(app_data_dir: PathBuf) -> Result<Self, String> {
        let root = app_data_dir.join("content-search");
        let service = SearchIndexService::start(
            root.join(SEARCH_INDEX_FILENAME),
            root.join(SOURCE_INDEX_FILENAME),
        )
        .map_err(|error| error.to_string())?;
        Ok(Self { service })
    }

    pub fn dispatch_json(&self, input: &str) -> Option<String> {
        let request: Value = serde_json::from_str(input).ok()?;
        let method = request.get("method")?.as_str()?;
        if !method.starts_with("feature.contentSearch.") {
            return None;
        }
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let params = request.get("params").and_then(Value::as_object).cloned().unwrap_or_default();
        let result = self.handle(method, &params);
        Some(match result {
            Ok(value) => json!({"id": id, "ok": true, "result": value}).to_string(),
            Err(message) => json!({"id": id, "ok": false, "error": message}).to_string(),
        })
    }

    pub fn observe_host_dispatch(&self, input: &str, output: &str) {
        let Ok(request) = serde_json::from_str::<Value>(input) else { return };
        if request.get("method").and_then(Value::as_str) != Some("feature.messaging.execute") {
            return;
        }
        let Some(envelope) = request.get("params").and_then(Value::as_object)
            .and_then(|params| params.get("envelope")).and_then(Value::as_object) else { return };
        let Some(actor_id) = envelope.get("context").and_then(Value::as_object)
            .and_then(|context| context.get("actorId")).and_then(Value::as_str)
            .map(str::trim).filter(|value| !value.is_empty()) else { return };
        let Ok(response) = serde_json::from_str::<Value>(output) else { return };
        if response.get("ok").and_then(Value::as_bool) != Some(true) { return }
        let Some(envelopes) = response.get("result").and_then(Value::as_object)
            .and_then(|result| result.get("envelopes")).and_then(Value::as_array) else { return };

        for envelope in envelopes {
            let Some(event) = envelope.get("event").and_then(Value::as_object) else { continue };
            match event.get("type").and_then(Value::as_str) {
                Some("syncBatch") => {
                    let mut grouped = BTreeMap::<String, Vec<IndexEntry>>::new();
                    for message in event.get("messages").and_then(Value::as_array).into_iter().flatten() {
                        let Some((conversation_id, entry)) = normalize_messaging_message(actor_id, message) else { continue };
                        grouped.entry(conversation_id).or_default().push(entry);
                    }
                    for (conversation_id, entries) in grouped {
                        let Ok(scope_id) = scope_key(actor_id, &conversation_id) else { continue };
                        if self.service.execute(SearchIndexJob::UpsertEntries {
                            scope_id: scope_id.clone(), entries: entries.clone(),
                        }).is_ok() {
                            publish_upsert_mutation(&scope_id, &entries);
                        }
                    }
                }
                Some("messageAdded") | Some("messageChanged") => {
                    let Some(message) = event.get("message") else { continue };
                    let Some((conversation_id, entry)) = normalize_messaging_message(actor_id, message) else { continue };
                    let Ok(scope_id) = scope_key(actor_id, &conversation_id) else { continue };
                    let entries = vec![entry];
                    if self.service.execute(SearchIndexJob::UpsertEntries {
                        scope_id: scope_id.clone(), entries: entries.clone(),
                    }).is_ok() {
                        publish_upsert_mutation(&scope_id, &entries);
                    }
                }
                Some("messagesDeleted") => {
                    let Some(conversation_id) = event.get("conversationId").and_then(Value::as_str) else { continue };
                    let Ok(scope_id) = scope_key(actor_id, conversation_id) else { continue };
                    for entry_id in event.get("messageIds").and_then(Value::as_array)
                        .into_iter().flatten().filter_map(Value::as_str)
                    {
                        if self.service.execute(SearchIndexJob::DeleteEntry {
                            scope_id: scope_id.clone(), entry_id: entry_id.to_owned(),
                        }).is_ok() {
                            publish_delete_mutation(&scope_id, entry_id);
                        }
                    }
                }
                Some("conversationParticipantChanged")
                    if event.get("removedActorId").and_then(Value::as_str) == Some(actor_id) =>
                {
                    let Some(conversation_id) = event.get("conversation").and_then(Value::as_object)
                        .and_then(|conversation| conversation.get("id")).and_then(Value::as_str) else { continue };
                    let Ok(scope_id) = scope_key(actor_id, conversation_id) else { continue };
                    if self.service.execute(SearchIndexJob::ClearScope { scope_id: scope_id.clone() }).is_ok() {
                        publish_clear_mutation(&scope_id);
                    }
                }
                _ => {}
            }
        }
    }

    pub fn dispose(&self) {
        self.service.dispose();
    }

    fn handle(&self, method: &str, params: &Map<String, Value>) -> Result<Value, String> {
        match method {
            "feature.contentSearch.replaceScope" => {
                let scope_id = scoped_params(params)?;
                let entries = parse_entries(params)?;
                self.service.execute(SearchIndexJob::ReplaceScope {
                    scope_id: scope_id.clone(), entries: entries.clone(),
                }).map_err(|error| error.to_string())?;
                publish_upsert_mutation(&scope_id, &entries);
                Ok(json!({"ok": true, "indexed": entries.len()}))
            }
            "feature.contentSearch.upsertEntries" => {
                let scope_id = scoped_params(params)?;
                let entries = parse_entries(params)?;
                self.service.execute(SearchIndexJob::UpsertEntries {
                    scope_id: scope_id.clone(), entries: entries.clone(),
                }).map_err(|error| error.to_string())?;
                publish_upsert_mutation(&scope_id, &entries);
                Ok(json!({"ok": true, "indexed": entries.len()}))
            }
            "feature.contentSearch.deleteEntry" => {
                let scope_id = scoped_params(params)?;
                let entry_id = required_text(params, "entryId")?;
                self.service.execute(SearchIndexJob::DeleteEntry {
                    scope_id: scope_id.clone(), entry_id: entry_id.clone(),
                }).map_err(|error| error.to_string())?;
                publish_delete_mutation(&scope_id, &entry_id);
                Ok(json!({"ok": true}))
            }
            "feature.contentSearch.clearScope" => {
                let scope_id = scoped_params(params)?;
                self.service.execute(SearchIndexJob::ClearScope { scope_id: scope_id.clone() })
                    .map_err(|error| error.to_string())?;
                publish_clear_mutation(&scope_id);
                Ok(json!({"ok": true}))
            }
            "feature.contentSearch.reindexScope" => {
                let scope_id = scoped_params(params)?;
                self.service.execute(SearchIndexJob::ReindexScope { scope_id })
                    .map_err(|error| error.to_string())
            }
            "feature.contentSearch.reconcile" => self.service.execute(SearchIndexJob::Reconcile)
                .map_err(|error| error.to_string()),
            "feature.contentSearch.searchMessages" => {
                let scope_id = scoped_params(params)?;
                let query = required_text(params, "query")?;
                self.service.execute(SearchIndexJob::SearchMessages {
                    scope_id, query, limit: bounded_limit(params),
                }).map_err(|error| error.to_string())
            }
            "feature.contentSearch.searchMedia" => {
                let scope_id = scoped_params(params)?;
                let query = params.get("query").and_then(Value::as_str).unwrap_or_default().to_owned();
                self.service.execute(SearchIndexJob::SearchMedia {
                    scope_id, query, limit: bounded_limit(params),
                }).map_err(|error| error.to_string())
            }
            "feature.contentSearch.status" => {
                let worker = self.service.execute(SearchIndexJob::Status)
                    .unwrap_or_else(|error| json!({"ready": false, "error": error.to_string()}));
                Ok(json!({
                    "extensionId": CONTENT_SEARCH_EXTENSION_ID,
                    "service": self.service.status(),
                    "worker": worker,
                }))
            }
            _ => Err(format!("unknown content-search method {method}")),
        }
    }
}

impl Drop for MobileContentSearchOwner {
    fn drop(&mut self) {
        self.dispose();
    }
}

fn scoped_params(params: &Map<String, Value>) -> Result<String, String> {
    scope_key(&required_text(params, "accountKey")?, &required_text(params, "conversationId")?)
}

fn parse_entries(params: &Map<String, Value>) -> Result<Vec<IndexEntry>, String> {
    params.get("entries").and_then(Value::as_array)
        .ok_or_else(|| "entries must be an array".to_string())?
        .iter().cloned()
        .map(|value| IndexEntry::from_value(value).map_err(|error| error.to_string()))
        .collect()
}

fn publish_upsert_mutation(scope_id: &str, entries: &[IndexEntry]) {
    let mut fields = Map::new();
    fields.insert("agentId".into(), Value::String(scope_id.to_owned()));
    fields.insert("entries".into(), Value::Array(
        entries.iter().filter_map(|entry| serde_json::to_value(entry).ok()).collect()
    ));
    publish_transcript_mutation(TranscriptMutation { kind: "entries-upserted".into(), fields });
}

fn publish_delete_mutation(scope_id: &str, entry_id: &str) {
    let mut fields = Map::new();
    fields.insert("agentId".into(), Value::String(scope_id.to_owned()));
    fields.insert("entryId".into(), Value::String(entry_id.to_owned()));
    publish_transcript_mutation(TranscriptMutation { kind: "entry-deleted".into(), fields });
}

fn publish_clear_mutation(scope_id: &str) {
    let mut fields = Map::new();
    fields.insert("agentId".into(), Value::String(scope_id.to_owned()));
    publish_transcript_mutation(TranscriptMutation { kind: "conversation-cleared".into(), fields });
}

fn normalize_messaging_message(actor_id: &str, value: &Value) -> Option<(String, IndexEntry)> {
    let raw = value.as_object()?;
    let id = raw.get("id")?.as_str()?.trim();
    let conversation_id = raw.get("conversationId")?.as_str()?.trim();
    if id.is_empty() || conversation_id.is_empty() { return None }
    let sender_id = raw.get("senderId").and_then(Value::as_str).unwrap_or_default();
    let timestamp_ms = raw.get("createdAtMs").and_then(Value::as_i64).unwrap_or_default();
    let content = raw.get("content").and_then(Value::as_object);
    let content_type = content.and_then(|content| content.get("type"))
        .and_then(Value::as_str).unwrap_or("unknown");
    let data = content.and_then(|content| content.get("data")).and_then(Value::as_object);
    let (body, file_name, mime_type) = messaging_content_projection(content_type, data, raw);
    let mut fields = BTreeMap::new();
    fields.insert("role".into(), Value::String(
        if sender_id == actor_id { "user" } else { "assistant" }.into()
    ));
    if let Some(body) = body { fields.insert("body".into(), Value::String(body)); }
    if let Some(file_name) = file_name { fields.insert("fileName".into(), Value::String(file_name)); }
    if let Some(mime_type) = mime_type { fields.insert("mimeType".into(), Value::String(mime_type)); }
    Some((conversation_id.to_owned(), IndexEntry {
        id: id.to_owned(), kind: "message".into(), timestamp_ms, fields,
    }))
}

fn messaging_content_projection(
    content_type: &str,
    data: Option<&Map<String, Value>>,
    raw: &Map<String, Value>,
) -> (Option<String>, Option<String>, Option<String>) {
    let empty = Map::new();
    let data = data.unwrap_or(&empty);
    let nested_text = data.get("text").and_then(Value::as_object)
        .and_then(|value| value.get("text")).and_then(Value::as_str).map(str::to_owned);
    let direct_media = data.get("media").and_then(Value::as_object);
    let attachment = raw.get("attachments").and_then(Value::as_array)
        .and_then(|rows| rows.first()).and_then(Value::as_object)
        .or_else(|| data.get("attachments").and_then(Value::as_array)
            .and_then(|rows| rows.first()).and_then(Value::as_object));
    let media = direct_media
        .or_else(|| attachment.and_then(|row| row.get("media").and_then(Value::as_object)))
        .or(attachment);
    let file_name = media.and_then(|media| media.get("fileName").or_else(|| media.get("name")))
        .and_then(Value::as_str).map(str::to_owned);
    let mime_type = media.and_then(|media| media.get("mimeType"))
        .and_then(Value::as_str).map(str::to_owned);
    let body = match content_type {
        "text" => nested_text,
        "photo" => Some(file_name.clone().map(|name| format!("image {name}")).unwrap_or_else(|| "image".into())),
        "video" => Some(file_name.clone().map(|name| format!("video {name}")).unwrap_or_else(|| "video".into())),
        "document" => Some(file_name.clone().map(|name| format!("file {name}")).unwrap_or_else(|| "file".into())),
        "voice" => Some("voice message".into()),
        "audio" => Some(file_name.clone().map(|name| format!("audio {name}")).unwrap_or_else(|| "audio".into())),
        "contact" => data.get("displayName").and_then(Value::as_str).map(|name| format!("contact {name}")),
        "poll" => data.get("question").and_then(Value::as_object)
            .and_then(|question| question.get("text")).and_then(Value::as_str)
            .map(|question| format!("poll {question}")),
        _ => file_name.clone(),
    };
    (body, file_name, mime_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_message_projection_preserves_media_and_role() {
        let value = json!({
            "id": "m1", "conversationId": "c1", "senderId": "actor-a", "createdAtMs": 42,
            "content": {"type": "document", "data": {"media": {
                "fileName": "notes.pdf", "mimeType": "application/pdf"
            }}}
        });
        let (conversation, entry) = normalize_messaging_message("actor-a", &value).unwrap();
        assert_eq!(conversation, "c1");
        assert_eq!(entry.fields["role"], "user");
        assert_eq!(entry.fields["fileName"], "notes.pdf");
        assert_eq!(entry.fields["mimeType"], "application/pdf");
    }
}
