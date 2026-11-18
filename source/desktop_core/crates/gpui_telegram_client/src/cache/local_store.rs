use crate::cache::sqlite_cache::{CachedMediaEntry, SqliteClientCache};
use gpui_telegram_core::error::Result;
use gpui_telegram_core::types::{ChatId, MessageId, UserId};
use gpui_telegram_proto::contracts::{ChatDetailDto, ChatFolderDto, DraftDto};
use gpui_telegram_proto::message::{MessageContent, MessageItem, ReactionCount};
use gpui_telegram_proto::{PollCard, TodoCard};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Default)]
struct LocalCacheState {
    owner_scope: Option<(String, u64)>,
    messages: HashMap<ChatId, Vec<MessageItem>>,
    message_reactions: HashMap<(ChatId, MessageId), Vec<ReactionCount>>,
    chats: HashMap<ChatId, ChatDetailDto>,
    drafts: HashMap<ChatId, DraftDto>,
    folders: Vec<ChatFolderDto>,
    chat_ttls: HashMap<ChatId, u32>,
    pinned_ids: HashMap<ChatId, Vec<MessageId>>,
    read_inbox_max: HashMap<ChatId, MessageId>,
    read_outbox_max: HashMap<ChatId, MessageId>,
    media_entries: HashMap<String, CachedMediaEntry>,
    mem_media_bytes: HashMap<String, Vec<u8>>,
}

#[derive(Debug, Default, Clone)]
pub struct LocalStore {
    state: Arc<RwLock<LocalCacheState>>,
    sqlite: Option<Arc<SqliteClientCache>>,
}

impl LocalStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open_with_dir(base_dir: impl AsRef<Path>) -> Result<Self> {
        let db = Arc::new(SqliteClientCache::open(base_dir.as_ref())?);
        let owner_scope = db.load_owner_scope();
        let mut messages = db.load_messages()?;
        let mut message_reactions = db.load_message_reactions()?;
        for (&cid, list) in messages.iter_mut() {
            for m in list.iter_mut() {
                if let Some(rx) = message_reactions.get(&(cid, m.id)) {
                    m.reactions = rx.clone();
                } else if !m.reactions.is_empty() {
                    message_reactions.insert((cid, m.id), m.reactions.clone());
                }
            }
        }
        let chats = db.load_chats()?;
        let drafts = db.load_drafts()?;
        let folders = db.load_folders()?;
        let chat_ttls = db.load_chat_ttls()?;
        let (read_inbox_max, read_outbox_max) = db.load_read_states()?;
        let media_entries = db.load_media_index()?;
        let state = LocalCacheState {
            owner_scope,
            messages,
            message_reactions,
            chats,
            drafts,
            folders,
            chat_ttls,
            pinned_ids: HashMap::new(),
            read_inbox_max,
            read_outbox_max,
            media_entries,
            mem_media_bytes: HashMap::new(),
        };
        Ok(Self {
            state: Arc::new(RwLock::new(state)),
            sqlite: Some(db),
        })
    }

    pub(crate) fn sqlite_cache(&self) -> Option<Arc<SqliteClientCache>> {
        self.sqlite.clone()
    }

    pub fn max_outbox_nonce(&self) -> u64 {
        self.sqlite
            .as_ref()
            .map(|db| db.max_outbox_nonce())
            .unwrap_or(0)
    }

    pub fn cache_dir(&self) -> Option<PathBuf> {
        self.sqlite.as_ref().map(|db| db.base_dir().to_path_buf())
    }

    /// Binds `(server_url, user_id)` as the owner of this cache, automatically clearing any
    /// prior account's private messages, `is_chosen_by_me` reactions, chats, drafts, and read cursors
    /// if a different account or server previously owned the SQLite store.
    pub fn bind_account_scope(&self, server_url: &str, user_id: u64) {
        if user_id == 0 {
            return;
        }
        let norm_srv = server_url.trim().trim_end_matches('/').to_string();
        let should_clear = {
            let guard = self.state.read();
            match &guard.owner_scope {
                Some((prev_srv, prev_uid)) => {
                    *prev_uid != user_id
                        || (!norm_srv.is_empty() && !prev_srv.is_empty() && *prev_srv != norm_srv)
                }
                None => false,
            }
        };
        if should_clear {
            self.clear_all();
        }
        self.state.write().owner_scope = Some((norm_srv.clone(), user_id));
        if let Some(db) = &self.sqlite {
            db.save_owner_scope(&norm_srv, user_id);
        }
    }

    pub fn find_chat_id_for_message(&self, msg_id: MessageId) -> Option<ChatId> {
        if msg_id.0 <= 0 {
            return None;
        }
        let guard = self.state.read();
        for (&cid, list) in &guard.messages {
            if cid.0 > 0 && list.iter().any(|m| m.id == msg_id) {
                return Some(cid);
            }
        }
        for &(cid, mid) in guard.message_reactions.keys() {
            if cid.0 > 0 && mid == msg_id {
                return Some(cid);
            }
        }
        None
    }

    pub fn clear_all(&self) {
        {
            let mut guard = self.state.write();
            guard.owner_scope = None;
            guard.messages.clear();
            guard.message_reactions.clear();
            guard.chats.clear();
            guard.drafts.clear();
            guard.folders.clear();
            guard.chat_ttls.clear();
            guard.pinned_ids.clear();
            guard.read_inbox_max.clear();
            guard.read_outbox_max.clear();
        }
        if let Some(db) = &self.sqlite {
            db.clear_all();
        }
    }

    pub fn insert_message(&self, mut message: MessageItem) {
        let mut stale_ids = Vec::new();
        let synced_reactions: Option<Vec<ReactionCount>>;
        {
            let mut guard = self.state.write();
            let guard = &mut *guard;
            if !message.reactions.is_empty() {
                guard
                    .message_reactions
                    .insert((message.chat_id, message.id), message.reactions.clone());
                synced_reactions = Some(message.reactions.clone());
            } else if let Some(cached_rx) = guard
                .message_reactions
                .get(&(message.chat_id, message.id))
                .cloned()
            {
                message.reactions = cached_rx;
                synced_reactions = None;
            } else {
                synced_reactions = None;
            }
            let list = guard.messages.entry(message.chat_id).or_default();
            list.retain(|existing| {
                let id_match = existing.id == message.id;
                let nonce_match = message.client_nonce.is_some()
                    && existing.client_nonce == message.client_nonce
                    && existing.id != message.id;
                if nonce_match {
                    stale_ids.push(existing.id);
                }
                !id_match && !nonce_match
            });
            for &old_id in &stale_ids {
                if let Some(prev_rx) = guard.message_reactions.remove(&(message.chat_id, old_id)) {
                    if message.reactions.is_empty() {
                        message.reactions = prev_rx.clone();
                        guard
                            .message_reactions
                            .insert((message.chat_id, message.id), prev_rx);
                    }
                }
            }
            list.push(message.clone());
            list.sort_by_key(|m| (m.date.timestamp_millis(), m.id.0));
        }
        if let Some(db) = &self.sqlite {
            for old_id in stale_ids {
                db.delete_message(message.chat_id, old_id);
            }
            db.upsert_message(&message);
            if let Some(rx) = synced_reactions {
                db.upsert_message_reactions(message.chat_id, message.id, &rx);
            } else if !message.reactions.is_empty() {
                db.upsert_message_reactions(message.chat_id, message.id, &message.reactions);
            }
        }
    }

    pub fn upsert_messages(&self, items: Vec<MessageItem>) {
        for item in items {
            self.insert_message(item);
        }
    }

    pub fn ack_client_nonce(&self, client_nonce: u64, assigned_id: MessageId) {
        if client_nonce == 0 || assigned_id.0 <= 0 {
            return;
        }
        let mut updated: Option<(ChatId, MessageId, MessageItem)> = None;
        {
            let mut guard = self.state.write();
            let mut migrated_rx: Option<(ChatId, MessageId, Vec<ReactionCount>)> = None;
            for (&chat_id, list) in guard.messages.iter_mut() {
                if let Some(item) = list.iter_mut().find(|m| m.client_nonce == Some(client_nonce)) {
                    let old_id = item.id;
                    item.id = assigned_id;
                    let clone = item.clone();
                    list.sort_by_key(|m| (m.date.timestamp_millis(), m.id.0));
                    migrated_rx = Some((chat_id, old_id, clone.reactions.clone()));
                    updated = Some((chat_id, old_id, clone));
                    break;
                }
            }
            if let Some((chat_id, old_id, item_rx)) = migrated_rx {
                if old_id != assigned_id {
                    let prev = guard.message_reactions.remove(&(chat_id, old_id));
                    let next_rx = prev.unwrap_or(item_rx);
                    if !next_rx.is_empty() {
                        guard.message_reactions.insert((chat_id, assigned_id), next_rx);
                    }
                }
            }
        }
        if let (Some(db), Some((chat_id, old_id, msg))) = (&self.sqlite, updated) {
            if old_id != msg.id {
                db.delete_message(chat_id, old_id);
            }
            db.upsert_message(&msg);
            if !msg.reactions.is_empty() {
                db.upsert_message_reactions(chat_id, msg.id, &msg.reactions);
            }
        }
    }

    pub fn edit_message(&self, edited: MessageItem) {
        self.insert_message(edited);
    }

    pub fn delete_messages(&self, chat_id: ChatId, message_ids: &[MessageId]) {
        if message_ids.is_empty() {
            return;
        }
        {
            let mut guard = self.state.write();
            if let Some(list) = guard.messages.get_mut(&chat_id) {
                list.retain(|m| !message_ids.contains(&m.id));
            }
            if let Some(pins) = guard.pinned_ids.get_mut(&chat_id) {
                pins.retain(|id| !message_ids.contains(id));
            }
            for &id in message_ids {
                guard.message_reactions.remove(&(chat_id, id));
            }
        }
        if let Some(db) = &self.sqlite {
            for &id in message_ids {
                db.delete_message(chat_id, id);
            }
        }
    }

    pub fn clear_chat_messages(&self, chat_id: ChatId) {
        let ids: Vec<MessageId> = {
            let mut guard = self.state.write();
            guard.pinned_ids.remove(&chat_id);
            guard.message_reactions.retain(|&(cid, _), _| cid != chat_id);
            guard
                .messages
                .remove(&chat_id)
                .unwrap_or_default()
                .into_iter()
                .map(|m| m.id)
                .collect()
        };
        if let Some(db) = &self.sqlite {
            for id in ids {
                db.delete_message(chat_id, id);
            }
            db.clear_chat_reactions(chat_id);
        }
    }

    pub fn remove_chat(&self, chat_id: ChatId) {
        self.clear_chat_messages(chat_id);
        let mut guard = self.state.write();
        guard.chats.remove(&chat_id);
        guard.drafts.remove(&chat_id);
    }

    pub fn get_reactions(&self, chat_id: ChatId, message_id: MessageId) -> Option<Vec<ReactionCount>> {
        let guard = self.state.read();
        if let Some(rx) = guard.message_reactions.get(&(chat_id, message_id)) {
            return Some(rx.clone());
        }
        guard
            .messages
            .get(&chat_id)
            .and_then(|list| list.iter().find(|m| m.id == message_id))
            .filter(|m| !m.reactions.is_empty())
            .map(|m| m.reactions.clone())
    }

    pub fn apply_viewer_reaction_mutation(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
        emoji: &str,
        action: Option<&str>,
    ) -> Vec<ReactionCount> {
        let mut current = self.get_reactions(chat_id, message_id).unwrap_or_default();
        let mode = action.unwrap_or("toggle");
        match mode {
            "add" => {
                if let Some(existing) = current.iter_mut().find(|r| r.emoji == emoji) {
                    if !existing.is_chosen_by_me {
                        existing.count = existing.count.saturating_add(1);
                        existing.is_chosen_by_me = true;
                    }
                } else {
                    current.push(ReactionCount::new(emoji.to_string(), 1, true));
                }
            }
            "remove" => {
                if let Some(pos) = current.iter().position(|r| r.emoji == emoji) {
                    if current[pos].is_chosen_by_me {
                        current[pos].count = current[pos].count.saturating_sub(1);
                        current[pos].is_chosen_by_me = false;
                        if current[pos].count == 0 {
                            current.remove(pos);
                        }
                    }
                }
            }
            _ => {
                if let Some(pos) = current.iter().position(|r| r.emoji == emoji) {
                    if current[pos].is_chosen_by_me {
                        current[pos].count = current[pos].count.saturating_sub(1);
                        current[pos].is_chosen_by_me = false;
                        if current[pos].count == 0 {
                            current.remove(pos);
                        }
                    } else {
                        current[pos].count = current[pos].count.saturating_add(1);
                        current[pos].is_chosen_by_me = true;
                    }
                } else {
                    current.push(ReactionCount::new(emoji.to_string(), 1, true));
                }
            }
        }
        self.update_reactions(chat_id, message_id, current.clone());
        current
    }

    pub fn update_reactions(&self, chat_id: ChatId, message_id: MessageId, reactions: Vec<ReactionCount>) {
        let updated = {
            let mut guard = self.state.write();
            if reactions.is_empty() {
                guard.message_reactions.remove(&(chat_id, message_id));
            } else {
                guard
                    .message_reactions
                    .insert((chat_id, message_id), reactions.clone());
            }
            guard.messages.get_mut(&chat_id).and_then(|list| {
                list.iter_mut().find(|m| m.id == message_id).map(|m| {
                    m.reactions = reactions.clone();
                    m.clone()
                })
            })
        };
        if let Some(db) = &self.sqlite {
            db.upsert_message_reactions(chat_id, message_id, &reactions);
            if let Some(msg) = updated {
                db.upsert_message(&msg);
            }
        }
    }

    pub fn update_reactions_for_actor(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
        actor_user_id: UserId,
        viewer_user_id: Option<UserId>,
        reactions: Vec<ReactionCount>,
    ) {
        let is_other_actor = match viewer_user_id {
            Some(viewer) => actor_user_id.0 != 0 && actor_user_id != viewer,
            None => false,
        };
        if !is_other_actor {
            self.update_reactions(chat_id, message_id, reactions);
            return;
        }
        let (merged_reactions, updated_msg) = {
            let mut guard = self.state.write();
            let my_chosen: std::collections::HashSet<String> = guard
                .message_reactions
                .get(&(chat_id, message_id))
                .cloned()
                .or_else(|| {
                    guard
                        .messages
                        .get(&chat_id)
                        .and_then(|list| list.iter().find(|m| m.id == message_id))
                        .map(|m| m.reactions.clone())
                })
                .unwrap_or_default()
                .into_iter()
                .filter(|r| r.is_chosen_by_me)
                .map(|r| r.emoji)
                .collect();
            let merged: Vec<ReactionCount> = reactions
                .into_iter()
                .map(|mut r| {
                    r.is_chosen_by_me = my_chosen.contains(&r.emoji);
                    r
                })
                .collect();
            if merged.is_empty() {
                guard.message_reactions.remove(&(chat_id, message_id));
            } else {
                guard
                    .message_reactions
                    .insert((chat_id, message_id), merged.clone());
            }
            let msg_opt = guard.messages.get_mut(&chat_id).and_then(|list| {
                list.iter_mut().find(|m| m.id == message_id).map(|m| {
                    m.reactions = merged.clone();
                    m.clone()
                })
            });
            (merged, msg_opt)
        };
        if let Some(db) = &self.sqlite {
            db.upsert_message_reactions(chat_id, message_id, &merged_reactions);
            if let Some(msg) = updated_msg {
                db.upsert_message(&msg);
            }
        }
    }

    pub fn update_poll(&self, chat_id: ChatId, message_id: MessageId, poll: PollCard) {
        let updated = {
            let mut guard = self.state.write();
            guard.messages.get_mut(&chat_id).and_then(|list| {
                list.iter_mut().find(|m| m.id == message_id).map(|m| {
                    m.content = MessageContent::Poll(poll);
                    m.clone()
                })
            })
        };
        if let (Some(db), Some(msg)) = (&self.sqlite, updated) {
            db.upsert_message(&msg);
        }
    }

    pub fn update_todo(&self, chat_id: ChatId, message_id: MessageId, todo: TodoCard) {
        let updated = {
            let mut guard = self.state.write();
            guard.messages.get_mut(&chat_id).and_then(|list| {
                list.iter_mut().find(|m| m.id == message_id).map(|m| {
                    m.content = MessageContent::Todo(todo);
                    m.clone()
                })
            })
        };
        if let (Some(db), Some(msg)) = (&self.sqlite, updated) {
            db.upsert_message(&msg);
        }
    }

    pub fn get_messages(&self, chat_id: ChatId) -> Result<Vec<MessageItem>> {
        Ok(self.state.read().messages.get(&chat_id).cloned().unwrap_or_default())
    }

    pub fn upsert_chat(&self, chat: ChatDetailDto) {
        let cid = ChatId(chat.chat_id);
        self.state.write().chats.insert(cid, chat.clone());
        if let Some(db) = &self.sqlite {
            db.upsert_chat(&chat);
        }
    }

    pub fn list_chats(&self) -> Vec<ChatDetailDto> {
        self.state.read().chats.values().cloned().collect()
    }

    pub fn get_chat(&self, chat_id: ChatId) -> Option<ChatDetailDto> {
        self.state.read().chats.get(&chat_id).cloned()
    }

    pub fn set_draft(&self, chat_id: ChatId, draft: Option<DraftDto>) {
        {
            let mut guard = self.state.write();
            if let Some(ref d) = draft {
                guard.drafts.insert(chat_id, d.clone());
            } else {
                guard.drafts.remove(&chat_id);
            }
        }
        if let Some(db) = &self.sqlite {
            db.set_draft(chat_id, draft.as_ref());
        }
    }

    pub fn get_draft(&self, chat_id: ChatId) -> Option<DraftDto> {
        self.state.read().drafts.get(&chat_id).cloned()
    }

    pub fn set_folders(&self, folders: Vec<ChatFolderDto>) {
        self.state.write().folders = folders.clone();
        if let Some(db) = &self.sqlite {
            db.replace_folders(&folders);
        }
    }

    pub fn get_folders(&self) -> Vec<ChatFolderDto> {
        self.state.read().folders.clone()
    }

    pub fn upsert_folder(&self, folder: ChatFolderDto) {
        if folder.title.trim().is_empty() {
            self.remove_folder(folder.id);
            return;
        }
        {
            let mut guard = self.state.write();
            if let Some(existing) = guard.folders.iter_mut().find(|f| f.id == folder.id) {
                *existing = folder.clone();
            } else {
                guard.folders.push(folder.clone());
            }
        }
        if let Some(db) = &self.sqlite {
            db.upsert_folder(&folder);
        }
    }

    pub fn remove_folder(&self, folder_id: i64) {
        self.state.write().folders.retain(|f| f.id != folder_id);
        if let Some(db) = &self.sqlite {
            db.delete_folder(folder_id);
        }
    }

    pub fn set_chat_ttl(&self, chat_id: ChatId, ttl_seconds: u32) {
        {
            let mut guard = self.state.write();
            if ttl_seconds == 0 {
                guard.chat_ttls.remove(&chat_id);
            } else {
                guard.chat_ttls.insert(chat_id, ttl_seconds);
            }
        }
        if let Some(db) = &self.sqlite {
            db.upsert_chat_ttl(chat_id, ttl_seconds);
        }
    }

    pub fn get_chat_ttl(&self, chat_id: ChatId) -> u32 {
        self.state
            .read()
            .chat_ttls
            .get(&chat_id)
            .copied()
            .unwrap_or(0)
    }

    pub fn purge_expired_messages(&self, now_ms: i64) -> Vec<(ChatId, Vec<MessageId>)> {
        let mut purged: Vec<(ChatId, Vec<MessageId>)> = Vec::new();
        {
            let mut guard = self.state.write();
            let guard = &mut *guard;
            for (&chat_id, list) in guard.messages.iter_mut() {
                let chat_ttl_secs = guard.chat_ttls.get(&chat_id).copied().unwrap_or(0);
                let mut expired_ids = Vec::new();
                list.retain(|msg| {
                    let msg_expires_at = msg.expires_at_ms.or_else(|| {
                        let ttl = msg.ttl_seconds.unwrap_or(chat_ttl_secs);
                        if ttl > 0 {
                            Some(msg.date.timestamp_millis().saturating_add(ttl as i64 * 1000))
                        } else {
                            None
                        }
                    });
                    if let Some(exp) = msg_expires_at {
                        if exp <= now_ms {
                            expired_ids.push(msg.id);
                            return false;
                        }
                    }
                    true
                });
                if !expired_ids.is_empty() {
                    for &mid in &expired_ids {
                        guard.message_reactions.remove(&(chat_id, mid));
                    }
                    purged.push((chat_id, expired_ids));
                }
            }
        }
        if let Some(db) = &self.sqlite {
            for (chat_id, ids) in &purged {
                for &mid in ids {
                    db.delete_message(*chat_id, mid);
                }
            }
        }
        purged
    }

    pub fn set_pinned_messages(&self, chat_id: ChatId, pinned_ids: Vec<MessageId>) {
        self.state.write().pinned_ids.insert(chat_id, pinned_ids);
    }

    pub fn get_pinned_messages(&self, chat_id: ChatId) -> Vec<MessageId> {
        self.state.read().pinned_ids.get(&chat_id).cloned().unwrap_or_default()
    }

    pub fn mark_read_inbox(&self, chat_id: ChatId, max_id: MessageId, unread_count: Option<u32>) {
        let (inbox, outbox, chat_opt) = {
            let mut guard = self.state.write();
            let entry = guard.read_inbox_max.entry(chat_id).or_insert(max_id);
            if max_id.0 > entry.0 {
                *entry = max_id;
            }
            let inbox = entry.0;
            let outbox = guard.read_outbox_max.get(&chat_id).map(|m| m.0).unwrap_or(0);
            let chat_opt = guard.chats.get_mut(&chat_id).map(|c| {
                c.read_inbox_max_id = c.read_inbox_max_id.max(max_id.0);
                if let Some(u) = unread_count {
                    c.unread_count = u;
                }
                c.clone()
            });
            (inbox, outbox, chat_opt)
        };
        if let Some(db) = &self.sqlite {
            db.upsert_read_state(chat_id, inbox, outbox);
            if let Some(c) = chat_opt {
                db.upsert_chat(&c);
            }
        }
    }

    pub fn mark_read_outbox(&self, chat_id: ChatId, max_id: MessageId) {
        let (inbox, outbox, chat_opt) = {
            let mut guard = self.state.write();
            let entry = guard.read_outbox_max.entry(chat_id).or_insert(max_id);
            if max_id.0 > entry.0 {
                *entry = max_id;
            }
            let outbox = entry.0;
            let inbox = guard.read_inbox_max.get(&chat_id).map(|m| m.0).unwrap_or(0);
            let chat_opt = guard.chats.get_mut(&chat_id).map(|c| {
                c.read_outbox_max_id = c.read_outbox_max_id.max(max_id.0);
                c.clone()
            });
            (inbox, outbox, chat_opt)
        };
        if let Some(db) = &self.sqlite {
            db.upsert_read_state(chat_id, inbox, outbox);
            if let Some(c) = chat_opt {
                db.upsert_chat(&c);
            }
        }
    }

    pub fn read_inbox_max_id(&self, chat_id: ChatId) -> Option<MessageId> {
        self.state.read().read_inbox_max.get(&chat_id).copied()
    }

    pub fn read_outbox_max_id(&self, chat_id: ChatId) -> Option<MessageId> {
        self.state.read().read_outbox_max.get(&chat_id).copied()
    }

    pub fn media_cas_path(&self, file_id: &str) -> Option<PathBuf> {
        self.state
            .read()
            .media_entries
            .get(file_id)
            .map(|e| e.local_path.clone())
            .filter(|p| p.exists())
    }

    pub fn put_media_cas(&self, file_id: &str, bytes: &[u8], mime_type: Option<&str>) -> Result<Option<PathBuf>> {
        if let Some(db) = &self.sqlite {
            let entry = db.write_media_cas(file_id, bytes, mime_type)?;
            let path = entry.local_path.clone();
            self.state.write().media_entries.insert(file_id.to_string(), entry);
            return Ok(Some(path));
        }
        self.state
            .write()
            .mem_media_bytes
            .insert(file_id.to_string(), bytes.to_vec());
        Ok(None)
    }

    pub fn get_media_cas(&self, file_id: &str) -> Option<Vec<u8>> {
        if let Some(path) = self.media_cas_path(file_id) {
            if let Ok(bytes) = std::fs::read(&path) {
                return Some(bytes);
            }
        }
        self.state.read().mem_media_bytes.get(file_id).cloned()
    }
}
