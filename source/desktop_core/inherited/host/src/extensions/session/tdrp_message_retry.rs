//! Telegram / TDRP Message Retries, Retry Lanes, and Queue Validation.

use super::*;

struct HumanRetryLane<'a> {
    key: String,
    serial: Option<Arc<Mutex<()>>>,
    registry: &'a Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
}

impl Drop for HumanRetryLane<'_> {
    fn drop(&mut self) {
        if let Ok(mut locks) = self.registry.lock() {
            let Some(serial) = self.serial.take() else { return; };
            let same_lane = locks.get(&self.key).is_some_and(|registered| Arc::ptr_eq(registered, &serial));
            drop(serial);
            if same_lane && locks.get(&self.key).is_some_and(|registered| Arc::strong_count(registered) == 1) {
                locks.remove(&self.key);
            }
        }
    }
}

impl ProductionSessionWorkers {
    pub(crate) fn validate_human_retry_conversation(
        metadata: &serde_json::Value, conversation_id: &str, account: &str,
    ) -> Result<(), String> {
        let kind = metadata.get("conversationKind").and_then(serde_json::Value::as_str).unwrap_or("human");
        if kind == "group" || kind == "channel" {
            let meta_id = metadata.get("conversationId").and_then(serde_json::Value::as_str)
                .ok_or_else(|| "Conversation metadata missing conversationId".to_string())?;
            if meta_id != conversation_id {
                return Err("Conversation identity mismatch".into());
            }
            let expected_prefix = if kind == "group" { "group-" } else { "channel-" };
            if !conversation_id.starts_with(expected_prefix) {
                return Err("Conversation id prefix mismatch".into());
            }
            let participants = metadata.get("participantIds").and_then(serde_json::Value::as_array)
                .filter(|p| !p.is_empty())
                .ok_or_else(|| "participants must be non-empty".to_string())?;
            for p in participants {
                let s = p.as_str().ok_or_else(|| "participant must be string".to_string())?;
                if s.trim().is_empty() || s.trim() != s {
                    return Err("invalid participant string".into());
                }
            }
            return Ok(());
        }
        let participants = metadata.get("participantIds").and_then(serde_json::Value::as_array)
            .filter(|participants| participants.len() == 2)
            .ok_or_else(|| "Human retry requires exactly the original two participants".to_string())?;
        let parse = |index: usize| participants[index].as_str()
            .filter(|value| !value.is_empty() && *value == value.trim())
            .ok_or_else(|| "Human retry persisted participant identity is invalid".to_string());
        let first = parse(0)?;
        let second = parse(1)?;
        if first == second || (first != account && second != account)
            || Self::human_direct_conversation_id(first, second) != conversation_id
        {
            return Err("Human retry participants conflict with the original conversation identity".into());
        }
        Ok(())
    }

    pub(crate) fn normalize_human_reply_identity(
        existing: &[serde_json::Value],
        requested: Option<&str>,
        local_human_id: &str,
    ) -> Result<Option<String>, String> {
        let Some(reply_id) = requested.map(str::trim).filter(|id| !id.is_empty()) else {
            return Ok(None);
        };
        let local_nonce = reply_id.strip_prefix("human-message:");
        if let Some(target) = existing.iter().find(|row| {
            row.get("id").and_then(serde_json::Value::as_str) == Some(reply_id)
        }) {
            if let Some(nonce) = local_nonce {
                if nonce.is_empty()
                    || target.get("kind").and_then(serde_json::Value::as_str) != Some("message")
                    || target.get("authorId").and_then(serde_json::Value::as_str) != Some(local_human_id)
                    || target.get("authorKind").and_then(serde_json::Value::as_str) != Some("human")
                    || target.get("clientNonce").and_then(serde_json::Value::as_str) != Some(nonce)
                    || target.get("remoteMessageId").is_some()
                    || !matches!(target.get("delivery").and_then(serde_json::Value::as_str),
                        Some("pending" | "dispatching" | "sent"))
                {
                    return Err("sendHumanMessage local reply identity conflicts with its author, nonce or settlement".into());
                }
                return Ok(Some(reply_id.to_string()));
            }
            let remote_id = target.get("remoteMessageId")
                .map(fabushi_message_identity_text).transpose()
                .map_err(|error| format!("sendHumanMessage reply remote identity is invalid: {error}"))?
                .ok_or_else(|| "sendHumanMessage reply has no canonical remote identity".to_string())?;
            if reply_id != format!("human-server-message:{remote_id}")
                || target.get("kind").and_then(serde_json::Value::as_str) != Some("message")
                || target.get("delivery").and_then(serde_json::Value::as_str) != Some("sent")
                || target.get("authorKind").and_then(serde_json::Value::as_str) != Some("human")
                || !target.get("authorId").and_then(serde_json::Value::as_str)
                    .is_some_and(|author| !author.trim().is_empty())
            {
                return Err("sendHumanMessage reply canonical identity is inconsistent".into());
            }
            return Ok(Some(reply_id.to_string()));
        }
        // Older versions could rekey the parent while leaving a durable local
        // reply ID behind. Its exact nonce is evidence only for this author's
        // row in this conversation; peer provenance and other stores are never
        // aliases. Ambiguous or damaged canonical rows remain fail-closed.
        let nonce = local_nonce.filter(|nonce| !nonce.is_empty())
            .ok_or_else(|| "sendHumanMessage reply target is not in this Human conversation".to_string())?;
        let mut candidates = existing.iter().filter(|row| {
            row.get("authorId").and_then(serde_json::Value::as_str) == Some(local_human_id)
                && row.get("clientNonce").and_then(serde_json::Value::as_str) == Some(nonce)
        });
        let target = candidates.next()
            .ok_or_else(|| "sendHumanMessage reply target is not in this Human conversation".to_string())?;
        if candidates.next().is_some() {
            return Err("sendHumanMessage local reply nonce has ambiguous canonical identities".into());
        }
        let remote_id = target.get("remoteMessageId").map(fabushi_message_identity_text).transpose()
            .map_err(|error| format!("sendHumanMessage local reply remote identity is invalid: {error}"))?
            .ok_or_else(|| "sendHumanMessage local reply has no canonical remote identity".to_string())?;
        let canonical_id = format!("human-server-message:{remote_id}");
        if target.get("id").and_then(serde_json::Value::as_str) != Some(canonical_id.as_str())
            || target.get("kind").and_then(serde_json::Value::as_str) != Some("message")
            || target.get("delivery").and_then(serde_json::Value::as_str) != Some("sent")
            || target.get("authorKind").and_then(serde_json::Value::as_str) != Some("human")
        {
            return Err("sendHumanMessage local reply canonical identity is inconsistent".into());
        }
        Ok(Some(canonical_id))
    }

    /// Retry is an identity-only operation on the existing durable intent. It
    /// never accepts replacement payload or reconstructs committed resources.
    pub fn retry_human_message(
        &self, conversation_id: &str, client_nonce: &str,
    ) -> Result<serde_json::Value, String> {
        if conversation_id.is_empty() || conversation_id != conversation_id.trim()
            || client_nonce.is_empty() || client_nonce != client_nonce.trim() || client_nonce.len() > 200
        {
            return Err("retryHumanMessage requires unpadded conversationId and clientNonce (at most 200 UTF-8 bytes)".into());
        }
        let (account, issuer) = self.human_forward_scope()?;
        // Serialize only explicit retries of this one intent. The ordinary
        // conversation mutation lane remains free during the message POST.
        let retry_key = serde_json::to_string(&("human-retry", &account, &issuer, conversation_id, client_nonce))
            .map_err(|error| error.to_string())?;
        let lane = HumanRetryLane {
            serial: Some(self.human_message_lock(&retry_key)?), key: retry_key, registry: &self.human_message_locks,
        };
        let _retry_guard = lane.serial.as_ref().expect("active retry lane").lock()
            .map_err(|_| "Human retry lock poisoned".to_string())?;
        if self.human_forward_scope()? != (account.clone(), issuer.clone()) {
            return Err("Human retry account or issuer changed while awaiting admission".into());
        }
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let snapshot = {
            let serial = self.human_message_lock(conversation_id)?;
            let _staging = serial.lock().map_err(|_| "Human conversation serial lock poisoned".to_string())?;
            if self.human_forward_scope()? != (account.clone(), issuer.clone()) {
                return Err("Human retry account or issuer changed while awaiting conversation admission".into());
            }
            let metadata = owner.read_metadata().map_err(|error| error.to_string())?;
            Self::validate_human_retry_conversation(&metadata, conversation_id, &account)?;
            if !self.metadata_has_local_human(&metadata)? {
                return Err("local Human identity is not a conversation participant".into());
            }
            self.peer_human_id_from_metadata(&metadata)?;
            let entries = owner.get_transcript_entries().map_err(|error| error.to_string())?;
            let entry = Self::human_retry_entry(&entries, &account, client_nonce)?;
            if entry.get("delivery").and_then(serde_json::Value::as_str) == Some("sent") {
                return Ok(entry.clone());
            }
            entry.clone()
        };
        if self.shipping_native_messaging()?.is_none() {
            return Err("Human retry requires the Native messaging service for an unsettled intent".into());
        }
        let mut admitted_retry = false;
        let result = (|| -> Result<serde_json::Value, String> {
            if self.human_forward_scope()? != (account.clone(), issuer.clone()) {
                return Err("Human retry account or issuer changed before dispatch".into());
            }
            if self.pending_human_forward_is_batch_owned(conversation_id, &snapshot)?
                || snapshot.get("forwardContext").is_some()
            {
                let context = snapshot.get("forwardContext")
                    .filter(|context| context.get("batchVersion").and_then(serde_json::Value::as_u64) == Some(1))
                    .ok_or_else(|| "Human retry requires the original supported forward source context".to_string())?;
                let source_id = context.get("sourceConversationId").and_then(serde_json::Value::as_str)
                    .filter(|value| !value.is_empty() && *value == value.trim())
                    .ok_or_else(|| "Human retry forward source is missing or invalid".to_string())?;
                let request_id = context.get("requestId").and_then(serde_json::Value::as_str)
                    .filter(|value| !value.is_empty() && *value == value.trim())
                    .ok_or_else(|| "Human retry forward request is missing or invalid".to_string())?;
                let batch = self.find_forward_request(&account, &issuer, request_id)?
                    .ok_or_else(|| "Human retry original forward batch is unavailable".to_string())?;
                if batch.source_conversation_id != source_id || &batch.context != context
                    || !batch.destinations.iter().any(|target|
                        target.conversation_id == conversation_id && target.client_nonce == client_nonce)
                {
                    return Err("Human retry does not match its original forward source and target".into());
                }
                admitted_retry = true;
                let delivered = self.deliver_forward_batch_from_intent(source_id, request_id, Some(&[conversation_id.to_string()]), true)?;
                let target = delivered.destinations.iter().find(|target|
                    target.conversation_id == conversation_id && target.client_nonce == client_nonce)
                    .ok_or_else(|| "Human retry original forward target disappeared".to_string())?;
                if !target.completed {
                    return Err(target.error.clone().unwrap_or_else(|| "Human forward retry remains unsettled".to_string()));
                }
                let serial = self.human_message_lock(conversation_id)?;
                let _settlement = serial.lock().map_err(|_| "Human conversation serial lock poisoned".to_string())?;
                if self.human_forward_scope()? != (account.clone(), issuer.clone()) {
                    return Err("Human retry account or issuer changed before settlement".into());
                }
                let entries = owner.get_transcript_entries().map_err(|error| error.to_string())?;
                let settled = Self::human_retry_entry(&entries, &account, client_nonce)?;
                if settled.get("delivery").and_then(serde_json::Value::as_str) != Some("sent") {
                    return Err("Human forward journal completed without a canonical target settlement".into());
                }
                return Ok(settled.clone());
            }
            admitted_retry = true;
            let retry_topic_id = snapshot.get("topicId").and_then(serde_json::Value::as_str);
            self.append_human_message_from_intent(
                conversation_id,
                snapshot.get("content").and_then(serde_json::Value::as_str).expect("validated retry content"),
                client_nonce,
                snapshot.get("composedAtMs").and_then(serde_json::Value::as_f64),
                snapshot.get("replyToId").and_then(serde_json::Value::as_str),
                snapshot.get("attachments").and_then(serde_json::Value::as_array).map(Vec::as_slice).unwrap_or(&[]),
                None, retry_topic_id, None, true, false,
            )
        })();
        let result = result.and_then(|settled| {
            if self.human_forward_scope()? != (account.clone(), issuer.clone()) {
                return Err("Human retry account or issuer changed before returning settlement".into());
            }
            Ok(settled)
        });
        if let (true, Err(error)) = (admitted_retry, &result) {
            // Source-policy and provenance failures happen before the common
            // sender. Preserve them on the same pending row for reload/UI,
            // without ever resurrecting a row already settled by sync.
            let serial = self.human_message_lock(conversation_id)?;
            let _failure = serial.lock().map_err(|_| "Human conversation serial lock poisoned".to_string())?;
            let entries = owner.get_transcript_entries().map_err(|error| error.to_string())?;
            if let Ok(current) = Self::human_retry_entry(&entries, &account, client_nonce) {
                if current.get("remoteMessageId").is_none()
                    && current.get("content") == snapshot.get("content")
                    && current.get("forwardContext") == snapshot.get("forwardContext")
                    && current.get("lastDispatchError").and_then(serde_json::Value::as_str) != Some(error.as_str())
                {
                    let mut failed = current.clone();
                    failed["delivery"] = serde_json::json!("pending");
                    failed["lastDispatchError"] = serde_json::json!(error);
                    failed["lastDispatchFailedAtMs"] = serde_json::json!(
                        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as f64);
                    owner.update_transcript_entry(failed["id"].as_str().expect("validated retry identity"), &failed)
                        .map_err(|failure| format!("{error}; retry failure could not be persisted: {failure}"))?
                        .ok_or_else(|| format!("{error}; retry intent disappeared while preserving its error"))?;
                }
            }
        }
        result
    }

    pub(crate) fn validate_human_queue_origin(entry: &serde_json::Value) -> Result<Option<bool>, String> {
        let origin = match entry.get("queuedOffline") {
            None => None,
            Some(serde_json::Value::Bool(value)) => Some(*value),
            Some(_) => return Err("Human queuedOffline origin must be a boolean".into()),
        };
        if origin == Some(true) && !entry.get("composedAtMs")
            .and_then(serde_json::Value::as_f64).is_some_and(f64::is_finite)
        {
            return Err("Human offline queue origin requires a finite composedAtMs".into());
        }
        Ok(origin)
    }

    pub(crate) fn human_retry_entry<'a>(
        entries: &'a [serde_json::Value], account: &str, client_nonce: &str,
    ) -> Result<&'a serde_json::Value, String> {
        let mut matches = entries.iter().filter(|entry|
            entry.get("authorId").and_then(serde_json::Value::as_str) == Some(account)
                && entry.get("clientNonce").and_then(serde_json::Value::as_str) == Some(client_nonce));
        let entry = matches.next().ok_or_else(|| "Human retry durable local intent was not found".to_string())?;
        if matches.next().is_some() {
            return Err("Human retry nonce identifies duplicate local intents".into());
        }
        if entry.get("kind").and_then(serde_json::Value::as_str) != Some("message")
            || entry.get("role").and_then(serde_json::Value::as_str) != Some("user")
            || entry.get("authorKind").and_then(serde_json::Value::as_str) != Some("human")
        {
            return Err("Human retry intent is not a local Human message".into());
        }
        Self::validate_human_queue_origin(entry)?;
        let content = entry.get("content").and_then(serde_json::Value::as_str)
            .ok_or_else(|| "Human retry persisted content is invalid".to_string())?;
        let attachments = match entry.get("attachments") {
            None => &[][..],
            Some(serde_json::Value::Array(items)) => items.as_slice(),
            Some(_) => return Err("Human retry persisted attachments are invalid".into()),
        };
        if (content.trim().is_empty() && attachments.is_empty()) || attachments.len() > 6 {
            return Err("Human retry persisted message payload is invalid".into());
        }
        for attachment in attachments {
            for field in ["path", "name"] {
                if attachment.get(field).and_then(serde_json::Value::as_str)
                    .filter(|value| !value.is_empty() && *value == value.trim()).is_none()
                {
                    return Err(format!("Human retry persisted attachment {field} is invalid"));
                }
            }
        }
        for field in ["composedAtMs", "timestampMs"] {
            if entry.get(field).is_some_and(|value| !value.is_null()
                && !value.as_f64().is_some_and(f64::is_finite))
            {
                return Err(format!("Human retry persisted {field} is invalid"));
            }
        }
        if entry.get("replyToId").is_some_and(|value| !value.is_null()
            && !value.as_str().is_some_and(|value| !value.is_empty() && value == value.trim()))
        {
            return Err("Human retry persisted reply identity is invalid".into());
        }
        if let Some(dispatched) = entry.get("remoteDispatchAttachments") {
            let resources = serde_json::from_value::<Vec<FabushiRemoteHumanAttachment>>(dispatched.clone())
                .map_err(|error| format!("Human retry persisted dispatch resources are invalid: {error}"))?;
            for resource in &resources {
                Self::validate_human_attachment_resource(resource)?;
            }
            if resources.len() != attachments.len() || resources.iter().zip(attachments).any(|(resource, attachment)|
                resource.resource_id.trim().is_empty() || resource.resource_id != resource.resource_id.trim()
                    || attachment.get("name").and_then(serde_json::Value::as_str) != Some(resource.name.as_str()))
            {
                return Err("Human retry persisted dispatch resources conflict with the original attachments".into());
            }
        }
        if entry.get("delivery").and_then(serde_json::Value::as_str) == Some("sent") {
            let remote_id = entry.get("remoteMessageId").ok_or_else(||
                "Human retry settled intent lacks a canonical remote identity".to_string())
                .and_then(fabushi_message_identity_text)?;
            if entry.get("id").and_then(serde_json::Value::as_str) != Some(format!("human-server-message:{remote_id}").as_str()) {
                return Err("Human retry settled intent has a conflicting canonical identity".into());
            }
        } else if entry.get("remoteMessageId").is_some()
            || !matches!(entry.get("delivery").and_then(serde_json::Value::as_str), Some("pending" | "dispatching"))
            || entry.get("id").and_then(serde_json::Value::as_str) != Some(format!("human-message:{client_nonce}").as_str())
        {
            return Err("Human retry intent is not in an admitted pending or canonical state".into());
        }
        Ok(entry)
    }

}
