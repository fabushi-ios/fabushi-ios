use crate::protocol::Failure;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ChatAdminRights {
    #[serde(default)]
    pub change_info: bool,
    #[serde(default)]
    pub post_messages: bool,
    #[serde(default)]
    pub edit_messages: bool,
    #[serde(default)]
    pub delete_messages: bool,
    #[serde(default)]
    pub ban_users: bool,
    #[serde(default)]
    pub invite_users: bool,
    #[serde(default)]
    pub pin_messages: bool,
    #[serde(default)]
    pub manage_topics: bool,
    #[serde(default)]
    pub manage_calls: bool,
    #[serde(default)]
    pub add_new_admins: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BannedMember {
    pub user_id: String,
    pub banned_by: String,
    pub until_date_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RestrictedPermissions {
    #[serde(default)]
    pub send_messages: bool,
    #[serde(default)]
    pub send_media: bool,
    #[serde(default)]
    pub send_polls: bool,
    #[serde(default)]
    pub embed_links: bool,
    #[serde(default)]
    pub add_web_page_previews: bool,
    #[serde(default)]
    pub invite_users: bool,
    #[serde(default)]
    pub pin_messages: bool,
    #[serde(default)]
    pub change_info: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum AuditLogAction {
    AdminRightsChanged { target_id: String },
    MemberBanned { target_id: String },
    MemberRestricted { target_id: String },
    MemberUnbanned { target_id: String },
    MessageDeleted { message_id: String },
    TopicCreated { topic_id: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditLogEntry {
    pub entry_id: String,
    pub actor_id: String,
    pub chat_id: String,
    pub action: AuditLogAction,
    pub timestamp_ms: i64,
}

#[derive(Debug, Clone, Default)]
pub struct ChatAdminModerationState {
    pub admin_rights: BTreeMap<(String, String), ChatAdminRights>,
    pub banned_members: BTreeMap<(String, String), BannedMember>,
    pub restricted_members: BTreeMap<(String, String), (RestrictedPermissions, i64)>,
    pub audit_log: Vec<AuditLogEntry>,
    next_audit_id: u32,
}

impl ChatAdminModerationState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_admin_rights(
        &mut self,
        chat_id: &str,
        actor_id: &str,
        target_id: &str,
        rights: ChatAdminRights,
        now_ms: i64,
    ) -> Result<ChatAdminRights, Failure> {
        let key = (chat_id.to_string(), target_id.to_string());
        self.admin_rights.insert(key, rights.clone());

        self.next_audit_id += 1;
        self.audit_log.push(AuditLogEntry {
            entry_id: format!("audit_{}", self.next_audit_id),
            actor_id: actor_id.to_string(),
            chat_id: chat_id.to_string(),
            action: AuditLogAction::AdminRightsChanged {
                target_id: target_id.to_string(),
            },
            timestamp_ms: now_ms,
        });
        Ok(rights)
    }

    pub fn ban_member(
        &mut self,
        chat_id: &str,
        actor_id: &str,
        target_id: &str,
        duration_seconds: Option<u64>,
        reason: Option<String>,
        now_ms: i64,
    ) -> Result<BannedMember, Failure> {
        let until_date_ms = match duration_seconds {
            Some(secs) if secs > 0 => now_ms + (secs as i64 * 1000),
            _ => 0, // 0 indicates permanent ban
        };

        let banned = BannedMember {
            user_id: target_id.to_string(),
            banned_by: actor_id.to_string(),
            until_date_ms,
            reason,
        };
        let key = (chat_id.to_string(), target_id.to_string());
        self.banned_members.insert(key, banned.clone());
        self.restricted_members.remove(&(chat_id.to_string(), target_id.to_string()));

        self.next_audit_id += 1;
        self.audit_log.push(AuditLogEntry {
            entry_id: format!("audit_{}", self.next_audit_id),
            actor_id: actor_id.to_string(),
            chat_id: chat_id.to_string(),
            action: AuditLogAction::MemberBanned {
                target_id: target_id.to_string(),
            },
            timestamp_ms: now_ms,
        });
        Ok(banned)
    }

    pub fn restrict_member(
        &mut self,
        chat_id: &str,
        actor_id: &str,
        target_id: &str,
        permissions: RestrictedPermissions,
        duration_seconds: Option<u64>,
        now_ms: i64,
    ) -> Result<RestrictedPermissions, Failure> {
        let until_date_ms = match duration_seconds {
            Some(secs) if secs > 0 => now_ms + (secs as i64 * 1000),
            _ => 0,
        };

        let key = (chat_id.to_string(), target_id.to_string());
        self.restricted_members
            .insert(key, (permissions.clone(), until_date_ms));

        self.next_audit_id += 1;
        self.audit_log.push(AuditLogEntry {
            entry_id: format!("audit_{}", self.next_audit_id),
            actor_id: actor_id.to_string(),
            chat_id: chat_id.to_string(),
            action: AuditLogAction::MemberRestricted {
                target_id: target_id.to_string(),
            },
            timestamp_ms: now_ms,
        });
        Ok(permissions)
    }

    pub fn unban_member(
        &mut self,
        chat_id: &str,
        actor_id: &str,
        target_id: &str,
        now_ms: i64,
    ) -> Result<bool, Failure> {
        let key = (chat_id.to_string(), target_id.to_string());
        let was_banned = self.banned_members.remove(&key).is_some();
        let was_restricted = self.restricted_members.remove(&key).is_some();

        if !was_banned && !was_restricted {
            return Err(Failure::new("MEMBER_NOT_BANNED", "member was not banned or restricted"));
        }

        self.next_audit_id += 1;
        self.audit_log.push(AuditLogEntry {
            entry_id: format!("audit_{}", self.next_audit_id),
            actor_id: actor_id.to_string(),
            chat_id: chat_id.to_string(),
            action: AuditLogAction::MemberUnbanned {
                target_id: target_id.to_string(),
            },
            timestamp_ms: now_ms,
        });
        Ok(true)
    }

    pub fn is_banned(&self, chat_id: &str, user_id: &str, now_ms: i64) -> bool {
        let key = (chat_id.to_string(), user_id.to_string());
        if let Some(banned) = self.banned_members.get(&key) {
            if banned.until_date_ms == 0 || banned.until_date_ms > now_ms {
                return true;
            }
        }
        false
    }

    pub fn get_audit_log(&self, chat_id: &str, limit: usize) -> Vec<AuditLogEntry> {
        self.audit_log
            .iter()
            .rev()
            .filter(|e| e.chat_id == chat_id)
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn handle_route(&mut self, method: &str, args: &Value) -> Result<Value, Failure> {
        let op = method.strip_prefix("admin.").unwrap_or(method);
        match op {
            "setRights" | "setAdminRights" => {
                let chat_id = args
                    .get("chatId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "chatId required"))?;
                let actor_id = args
                    .get("actorId")
                    .or_else(|| args.get("adminId"))
                    .and_then(Value::as_str)
                    .unwrap_or("self_admin");
                let target_id = args
                    .get("targetId")
                    .or_else(|| args.get("userId"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "targetId or userId required"))?;
                let rights: ChatAdminRights = args
                    .get("rights")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                let now_ms = args
                    .get("nowMs")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                let res = self.set_admin_rights(chat_id, actor_id, target_id, rights, now_ms)?;
                Ok(json!({ "rights": res }))
            }
            "banMember" | "ban" => {
                let chat_id = args
                    .get("chatId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "chatId required"))?;
                let actor_id = args
                    .get("actorId")
                    .or_else(|| args.get("adminId"))
                    .and_then(Value::as_str)
                    .unwrap_or("self_admin");
                let target_id = args
                    .get("targetId")
                    .or_else(|| args.get("userId"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "targetId or userId required"))?;
                let duration_seconds = args
                    .get("durationSeconds")
                    .or_else(|| args.get("duration"))
                    .and_then(Value::as_u64);
                let reason = args
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let now_ms = args
                    .get("nowMs")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                let banned = self.ban_member(
                    chat_id,
                    actor_id,
                    target_id,
                    duration_seconds,
                    reason,
                    now_ms,
                )?;
                Ok(json!({ "banned": banned }))
            }
            "restrictMember" | "restrict" => {
                let chat_id = args
                    .get("chatId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "chatId required"))?;
                let actor_id = args
                    .get("actorId")
                    .or_else(|| args.get("adminId"))
                    .and_then(Value::as_str)
                    .unwrap_or("self_admin");
                let target_id = args
                    .get("targetId")
                    .or_else(|| args.get("userId"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "targetId or userId required"))?;
                let permissions: RestrictedPermissions = args
                    .get("permissions")
                    .or_else(|| args.get("restrictions"))
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                let duration_seconds = args
                    .get("durationSeconds")
                    .or_else(|| args.get("duration"))
                    .and_then(Value::as_u64);
                let now_ms = args
                    .get("nowMs")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                let restricted = self.restrict_member(
                    chat_id,
                    actor_id,
                    target_id,
                    permissions,
                    duration_seconds,
                    now_ms,
                )?;
                Ok(json!({ "restricted": restricted }))
            }
            "unbanMember" | "unban" => {
                let chat_id = args
                    .get("chatId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "chatId required"))?;
                let actor_id = args
                    .get("actorId")
                    .or_else(|| args.get("adminId"))
                    .and_then(Value::as_str)
                    .unwrap_or("self_admin");
                let target_id = args
                    .get("targetId")
                    .or_else(|| args.get("userId"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "targetId or userId required"))?;
                let now_ms = args
                    .get("nowMs")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                let unbanned = self.unban_member(chat_id, actor_id, target_id, now_ms)?;
                Ok(json!({ "unbanned": unbanned }))
            }
            "getAuditLog" => {
                let chat_id = args
                    .get("chatId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "chatId required"))?;
                let limit = args
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(50) as usize;

                let entries = self.get_audit_log(chat_id, limit);
                Ok(json!({ "auditLog": entries }))
            }
            other => Err(Failure::new(
                "UNKNOWN_ADMIN_METHOD",
                format!("unrecognized admin method {other}"),
            )),
        }
    }
}
