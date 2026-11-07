use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Number, Value};

pub const SAND_GROUP_FILENAME: &str = "group.json";
pub const GROUP_CONFIG_VERSION: f64 = 1.0;
pub const GROUP_MAX_MEMBERS: usize = 6;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteGroupMember {
    pub owner_auth_id: String,
    pub agent_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_data_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SandGroupConfig {
    pub version: f64,
    pub member_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_members: Option<Vec<RemoteGroupMember>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared_room_id: Option<String>,
}

pub fn get_sand_group_path(agent_dir: impl AsRef<Path>) -> PathBuf {
    agent_dir.as_ref().join(SAND_GROUP_FILENAME)
}

pub fn normalize_member_ids(raw: &Value) -> Vec<String> {
    let Some(values) = raw.as_array() else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut ids = Vec::new();
    for value in values {
        let Some(raw_id) = value.as_str() else {
            continue;
        };
        let id = raw_id.trim();
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue;
        }
        ids.push(id.to_string());
        if ids.len() >= GROUP_MAX_MEMBERS {
            break;
        }
    }
    ids
}

pub fn normalize_remote_members(raw: &Value) -> Vec<RemoteGroupMember> {
    let Some(values) = raw.as_array() else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut members = Vec::new();
    for value in values {
        let Some(candidate) = value.as_object() else {
            continue;
        };
        let Some(owner_auth_id) = candidate.get("ownerAuthId").and_then(Value::as_str) else {
            continue;
        };
        let Some(agent_id) = candidate.get("agentId").and_then(Value::as_str) else {
            continue;
        };
        let owner_auth_id = owner_auth_id.trim();
        let agent_id = agent_id.trim();
        if owner_auth_id.is_empty() || agent_id.is_empty() {
            continue;
        }
        let key = format!("{owner_auth_id}\0{agent_id}");
        if !seen.insert(key) {
            continue;
        }
        let name = candidate
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("Agent")
            .to_string();
        let avatar_data_url = candidate
            .get("avatarDataUrl")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        members.push(RemoteGroupMember {
            owner_auth_id: owner_auth_id.to_string(),
            agent_id: agent_id.to_string(),
            name,
            avatar_data_url,
        });
        if members.len() >= GROUP_MAX_MEMBERS {
            break;
        }
    }
    members
}

pub fn read_sand_group_config(agent_dir: impl AsRef<Path>) -> Option<SandGroupConfig> {
    let raw = fs::read_to_string(get_sand_group_path(agent_dir)).ok()?;
    let parsed = serde_json::from_str::<Value>(&raw).ok()?;
    let object = parsed.as_object()?;
    let member_ids = normalize_member_ids(object.get("memberIds").unwrap_or(&Value::Null));
    let remote_members =
        normalize_remote_members(object.get("remoteMembers").unwrap_or(&Value::Null));
    let shared_room_id = object
        .get("sharedRoomId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if member_ids.is_empty() && remote_members.is_empty() && shared_room_id.is_none() {
        return None;
    }
    Some(SandGroupConfig {
        version: object
            .get("version")
            .and_then(Value::as_f64)
            .unwrap_or(GROUP_CONFIG_VERSION),
        member_ids,
        remote_members: (!remote_members.is_empty()).then_some(remote_members),
        shared_room_id,
    })
}

pub fn write_sand_group_config(
    agent_dir: impl AsRef<Path>,
    config: &SandGroupConfig,
) -> io::Result<()> {
    let path = get_sand_group_path(agent_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let member_ids = normalize_member_ids(&serde_json::to_value(&config.member_ids).unwrap_or_default());
    let remote_members_value =
        serde_json::to_value(config.remote_members.as_deref().unwrap_or_default()).unwrap_or_default();
    let remote_members = normalize_remote_members(&remote_members_value);
    let mut object = Map::new();
    object.insert("version".into(), json_number(config.version));
    object.insert("memberIds".into(), serde_json::json!(member_ids));
    if !remote_members.is_empty() {
        object.insert("remoteMembers".into(), serde_json::json!(remote_members));
    }
    if let Some(shared_room_id) = config
        .shared_room_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        object.insert(
            "sharedRoomId".into(),
            Value::String(shared_room_id.to_string()),
        );
    }
    write_pretty_json(&path, &Value::Object(object))
}

pub fn is_sand_group_dir(agent_dir: impl AsRef<Path>) -> bool {
    read_sand_group_config(agent_dir).is_some()
}

fn json_number(value: f64) -> Value {
    if value.is_finite()
        && value >= 0.0
        && value.fract() == 0.0
        && value <= u64::MAX as f64
    {
        Value::Number(Number::from(value as u64))
    } else {
        Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or_else(|| Value::Number(Number::from(GROUP_CONFIG_VERSION as u64)))
    }
}

fn write_pretty_json(path: &Path, value: &Value) -> io::Result<()> {
    let mut raw = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    raw.push('\n');
    fs::write(path, raw)
}
