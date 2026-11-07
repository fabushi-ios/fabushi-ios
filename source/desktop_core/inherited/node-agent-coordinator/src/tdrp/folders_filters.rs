use crate::protocol::Failure;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FolderFilterRule {
    #[serde(default)]
    pub include_contacts: bool,
    #[serde(default)]
    pub include_non_contacts: bool,
    #[serde(default)]
    pub include_groups: bool,
    #[serde(default)]
    pub include_channels: bool,
    #[serde(default)]
    pub include_bots: bool,
    #[serde(default)]
    pub exclude_muted: bool,
    #[serde(default)]
    pub exclude_read: bool,
    #[serde(default)]
    pub exclude_archived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatFolder {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_index: Option<u8>,
    #[serde(default)]
    pub flags: u32,
    pub filter_rule: FolderFilterRule,
    pub pinned_chat_ids: Vec<String>,
    pub included_chat_ids: Vec<String>,
    pub excluded_chat_ids: Vec<String>,
    #[serde(default)]
    pub always_peers: Vec<String>,
    #[serde(default)]
    pub never_peers: Vec<String>,
    pub order_index: u32,
}

#[derive(Debug, Clone, Default)]
pub struct FolderFilterState {
    pub folders: BTreeMap<String, ChatFolder>,
    pub folder_order: Vec<String>,
    next_id: u32,
}

impl FolderFilterState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn list_folders(&self) -> Vec<ChatFolder> {
        self.folder_order
            .iter()
            .filter_map(|id| self.folders.get(id).cloned())
            .collect()
    }

    pub fn create_folder(
        &mut self,
        title: String,
        icon: Option<String>,
        filter_rule: FolderFilterRule,
        included_chat_ids: Vec<String>,
        excluded_chat_ids: Vec<String>,
    ) -> Result<ChatFolder, Failure> {
        if title.trim().is_empty() {
            return Err(Failure::new(
                "INVALID_FOLDER_TITLE",
                "folder title cannot be empty",
            ));
        }
        self.next_id += 1;
        let id = format!("folder_{}", self.next_id);
        let order_index = self.folder_order.len() as u32;
        let folder = ChatFolder {
            id: id.clone(),
            title,
            icon,
            color_index: None,
            flags: 0,
            filter_rule,
            pinned_chat_ids: Vec::new(),
            included_chat_ids,
            excluded_chat_ids,
            always_peers: Vec::new(),
            never_peers: Vec::new(),
            order_index,
        };
        self.folders.insert(id.clone(), folder.clone());
        self.folder_order.push(id);
        Ok(folder)
    }

    pub fn update_folder(
        &mut self,
        folder_id: &str,
        title: Option<String>,
        icon: Option<String>,
        filter_rule: Option<FolderFilterRule>,
        included_chat_ids: Option<Vec<String>>,
        excluded_chat_ids: Option<Vec<String>>,
    ) -> Result<ChatFolder, Failure> {
        let folder = self
            .folders
            .get_mut(folder_id)
            .ok_or_else(|| Failure::new("FOLDER_NOT_FOUND", format!("folder {folder_id} not found")))?;

        if let Some(t) = title {
            if t.trim().is_empty() {
                return Err(Failure::new(
                    "INVALID_FOLDER_TITLE",
                    "folder title cannot be empty",
                ));
            }
            folder.title = t;
        }
        if icon.is_some() {
            folder.icon = icon;
        }
        if let Some(rule) = filter_rule {
            folder.filter_rule = rule;
        }
        if let Some(inc) = included_chat_ids {
            folder.included_chat_ids = inc;
        }
        if let Some(exc) = excluded_chat_ids {
            folder.excluded_chat_ids = exc;
        }
        Ok(folder.clone())
    }

    pub fn delete_folder(&mut self, folder_id: &str) -> Result<bool, Failure> {
        if self.folders.remove(folder_id).is_none() {
            return Err(Failure::new(
                "FOLDER_NOT_FOUND",
                format!("folder {folder_id} not found"),
            ));
        }
        self.folder_order.retain(|id| id != folder_id);
        for (idx, id) in self.folder_order.iter().enumerate() {
            if let Some(folder) = self.folders.get_mut(id) {
                folder.order_index = idx as u32;
            }
        }
        Ok(true)
    }

    pub fn reorder_folders(&mut self, ordered_ids: Vec<String>) -> Result<Vec<String>, Failure> {
        for id in &ordered_ids {
            if !self.folders.contains_key(id) {
                return Err(Failure::new(
                    "FOLDER_NOT_FOUND",
                    format!("folder {id} not found in state"),
                ));
            }
        }
        for (idx, id) in ordered_ids.iter().enumerate() {
            if let Some(folder) = self.folders.get_mut(id) {
                folder.order_index = idx as u32;
            }
        }
        self.folder_order = ordered_ids;
        Ok(self.folder_order.clone())
    }

    pub fn pin_chat(
        &mut self,
        folder_id: &str,
        chat_id: String,
        pin: bool,
    ) -> Result<ChatFolder, Failure> {
        let folder = self
            .folders
            .get_mut(folder_id)
            .ok_or_else(|| Failure::new("FOLDER_NOT_FOUND", format!("folder {folder_id} not found")))?;

        if pin {
            if !folder.pinned_chat_ids.contains(&chat_id) {
                if folder.pinned_chat_ids.len() >= 100 {
                    return Err(Failure::new(
                        "PINNED_CHATS_LIMIT_EXCEEDED",
                        "maximum pinned chats reached for folder",
                    ));
                }
                folder.pinned_chat_ids.push(chat_id);
            }
        } else {
            folder.pinned_chat_ids.retain(|c| c != &chat_id);
        }
        Ok(folder.clone())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn matches_chat(
        &self,
        folder_id: &str,
        chat_id: &str,
        is_contact: bool,
        is_group: bool,
        is_channel: bool,
        is_bot: bool,
        is_muted: bool,
        is_read: bool,
        is_archived: bool,
    ) -> bool {
        let Some(folder) = self.folders.get(folder_id) else {
            return false;
        };

        if folder.excluded_chat_ids.iter().any(|c| c == chat_id)
            || folder.never_peers.iter().any(|c| c == chat_id)
        {
            return false;
        }
        if folder.included_chat_ids.iter().any(|c| c == chat_id)
            || folder.pinned_chat_ids.iter().any(|c| c == chat_id)
            || folder.always_peers.iter().any(|c| c == chat_id)
        {
            return true;
        }

        let f = folder.flags;
        if f != 0 {
            if (f & 0x20 != 0) && is_muted {
                return false;
            }
            if (f & 0x40 != 0) && is_read {
                return false;
            }
            if (f & 0x80 != 0) && is_archived {
                return false;
            }
            if (f & 0x01 != 0) && is_contact {
                return true;
            }
            if (f & 0x02 != 0) && !is_contact && !is_group && !is_channel && !is_bot {
                return true;
            }
            if (f & 0x04 != 0) && is_group {
                return true;
            }
            if (f & 0x08 != 0) && is_channel {
                return true;
            }
            if (f & 0x10 != 0) && is_bot {
                return true;
            }
        }

        let rule = &folder.filter_rule;
        if rule.exclude_muted && is_muted {
            return false;
        }
        if rule.exclude_read && is_read {
            return false;
        }
        if rule.exclude_archived && is_archived {
            return false;
        }

        if rule.include_contacts && is_contact {
            return true;
        }
        if rule.include_non_contacts && !is_contact && !is_group && !is_channel && !is_bot {
            return true;
        }
        if rule.include_groups && is_group {
            return true;
        }
        if rule.include_channels && is_channel {
            return true;
        }
        if rule.include_bots && is_bot {
            return true;
        }

        false
    }

    pub fn handle_route(&mut self, method: &str, args: &Value) -> Result<Value, Failure> {
        let op = method.strip_prefix("folders.").unwrap_or(method);
        match op {
            "list" => {
                let folders = self.list_folders();
                Ok(json!({ "folders": folders }))
            }
            "create" => {
                let payload = args.get("folder").unwrap_or(args);
                let title = payload
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let icon = payload
                    .get("icon")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let color_index = payload
                    .get("colorIndex")
                    .or_else(|| payload.get("color_index"))
                    .and_then(Value::as_u64)
                    .map(|v| v as u8);
                let flags = payload
                    .get("flags")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32;
                let filter_rule: FolderFilterRule = payload
                    .get("filterRule")
                    .or_else(|| payload.get("filter_rule"))
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                let included = payload
                    .get("includedChatIds")
                    .or_else(|| payload.get("included_chat_ids"))
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(ToString::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                let excluded = payload
                    .get("excludedChatIds")
                    .or_else(|| payload.get("excluded_chat_ids"))
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(ToString::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                let always = payload
                    .get("alwaysPeers")
                    .or_else(|| payload.get("always_peers"))
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(ToString::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                let never = payload
                    .get("neverPeers")
                    .or_else(|| payload.get("never_peers"))
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(ToString::to_string)
                            .collect()
                    })
                    .unwrap_or_default();

                let mut folder = self.create_folder(title, icon, filter_rule, included, excluded)?;
                folder.color_index = color_index;
                folder.flags = flags;
                folder.always_peers = always;
                folder.never_peers = never;
                if let Some(f) = self.folders.get_mut(&folder.id) {
                    *f = folder.clone();
                }
                Ok(json!({ "folder": folder }))
            }
            "update" => {
                let payload = args.get("folder").unwrap_or(args);
                let folder_id = payload
                    .get("folderId")
                    .or_else(|| payload.get("id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "folderId required"))?;
                let title = payload
                    .get("title")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let icon = payload
                    .get("icon")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let filter_rule: Option<FolderFilterRule> = payload
                    .get("filterRule")
                    .or_else(|| payload.get("filter_rule"))
                    .and_then(|v| serde_json::from_value(v.clone()).ok());
                let included = payload
                    .get("includedChatIds")
                    .or_else(|| payload.get("included_chat_ids"))
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(ToString::to_string)
                            .collect()
                    });
                let excluded = payload
                    .get("excludedChatIds")
                    .or_else(|| payload.get("excluded_chat_ids"))
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(ToString::to_string)
                            .collect()
                    });

                let mut folder =
                    self.update_folder(folder_id, title, icon, filter_rule, included, excluded)?;
                if let Some(c) = payload
                    .get("colorIndex")
                    .or_else(|| payload.get("color_index"))
                    .and_then(Value::as_u64)
                {
                    folder.color_index = Some(c as u8);
                }
                if let Some(fl) = payload.get("flags").and_then(Value::as_u64) {
                    folder.flags = fl as u32;
                }
                if let Some(arr) = payload
                    .get("alwaysPeers")
                    .or_else(|| payload.get("always_peers"))
                    .and_then(Value::as_array)
                {
                    folder.always_peers = arr.iter().filter_map(Value::as_str).map(ToString::to_string).collect();
                }
                if let Some(arr) = payload
                    .get("neverPeers")
                    .or_else(|| payload.get("never_peers"))
                    .and_then(Value::as_array)
                {
                    folder.never_peers = arr.iter().filter_map(Value::as_str).map(ToString::to_string).collect();
                }
                if let Some(f) = self.folders.get_mut(folder_id) {
                    *f = folder.clone();
                }
                Ok(json!({ "folder": folder }))
            }
            "delete" => {
                let folder_id = args
                    .get("folderId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "folderId required"))?;
                let deleted = self.delete_folder(folder_id)?;
                Ok(json!({ "deleted": deleted }))
            }
            "reorder" => {
                let order = args
                    .get("orderedIds")
                    .and_then(Value::as_array)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "orderedIds required"))?
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect();
                let new_order = self.reorder_folders(order)?;
                Ok(json!({ "orderedIds": new_order }))
            }
            "pinChat" => {
                let folder_id = args
                    .get("folderId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "folderId required"))?;
                let chat_id = args
                    .get("chatId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "chatId required"))?
                    .to_string();
                let pin = args.get("pin").and_then(Value::as_bool).unwrap_or(true);
                let folder = self.pin_chat(folder_id, chat_id, pin)?;
                Ok(json!({ "folder": folder }))
            }
            other => Err(Failure::new(
                "UNKNOWN_FOLDER_METHOD",
                format!("unrecognized folder method {other}"),
            )),
        }
    }
}
