use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Number, Value};

pub const SAND_REMOTE_ROOM_FILENAME: &str = "remote-room.json";
pub const REMOTE_ROOM_CONFIG_VERSION: f64 = 1.0;
pub const REMOTE_ROOM_MAX_MEMBERS: usize = 24;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRoomMember {
    pub kind: String,
    pub auth_id: String,
    pub agent_id: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SandRemoteRoomConfig {
    pub version: f64,
    pub room_id: String,
    pub host_auth_id: String,
    pub host_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_avatar_url: Option<String>,
    pub members: Vec<RemoteRoomMember>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_revoked: Option<bool>,
}

pub fn get_sand_remote_room_path(agent_dir: impl AsRef<Path>) -> PathBuf {
    agent_dir.as_ref().join(SAND_REMOTE_ROOM_FILENAME)
}

pub fn normalize_remote_room_members(raw: &Value) -> Vec<RemoteRoomMember> {
    let Some(values) = raw.as_array() else {
        return Vec::new();
    };
    let mut members = Vec::new();
    for value in values {
        let Some(candidate) = value.as_object() else {
            continue;
        };
        let Some(auth_id) = candidate.get("authId").and_then(Value::as_str) else {
            continue;
        };
        if auth_id.is_empty() {
            continue;
        }
        let kind = if candidate.get("kind").and_then(Value::as_str) == Some("agent") {
            "agent"
        } else {
            "human"
        };
        let agent_id = candidate
            .get("agentId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let display_name = candidate
            .get("displayName")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("Someone")
            .to_string();
        let avatar_url = candidate
            .get("avatarUrl")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        members.push(RemoteRoomMember {
            kind: kind.to_string(),
            auth_id: auth_id.to_string(),
            agent_id,
            display_name,
            avatar_url,
        });
        if members.len() >= REMOTE_ROOM_MAX_MEMBERS {
            break;
        }
    }
    members
}

pub fn read_sand_remote_room_config(
    agent_dir: impl AsRef<Path>,
) -> Option<SandRemoteRoomConfig> {
    let raw = fs::read_to_string(get_sand_remote_room_path(agent_dir)).ok()?;
    let parsed = serde_json::from_str::<Value>(&raw).ok()?;
    let object = parsed.as_object()?;
    let room_id = object.get("roomId").and_then(Value::as_str)?;
    let host_auth_id = object.get("hostAuthId").and_then(Value::as_str)?;
    if room_id.is_empty() || host_auth_id.is_empty() {
        return None;
    }
    Some(SandRemoteRoomConfig {
        version: object
            .get("version")
            .and_then(Value::as_f64)
            .unwrap_or(REMOTE_ROOM_CONFIG_VERSION),
        room_id: room_id.to_string(),
        host_auth_id: host_auth_id.to_string(),
        host_name: object
            .get("hostName")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("The host")
            .to_string(),
        host_avatar_url: object
            .get("hostAvatarUrl")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        members: normalize_remote_room_members(
            object.get("members").unwrap_or(&Value::Null),
        ),
        is_revoked: (object.get("isRevoked").and_then(Value::as_bool) == Some(true))
            .then_some(true),
    })
}

pub fn write_sand_remote_room_config(
    agent_dir: impl AsRef<Path>,
    config: &SandRemoteRoomConfig,
) -> io::Result<()> {
    let path = get_sand_remote_room_path(agent_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut object = Map::new();
    object.insert("version".into(), json_number(config.version));
    object.insert("roomId".into(), Value::String(config.room_id.clone()));
    object.insert(
        "hostAuthId".into(),
        Value::String(config.host_auth_id.clone()),
    );
    object.insert("hostName".into(), Value::String(config.host_name.clone()));
    if let Some(host_avatar_url) = config.host_avatar_url.as_deref() {
        object.insert(
            "hostAvatarUrl".into(),
            Value::String(host_avatar_url.to_string()),
        );
    }
    object.insert("members".into(), serde_json::json!(config.members));
    if config.is_revoked == Some(true) {
        object.insert("isRevoked".into(), Value::Bool(true));
    }
    write_pretty_json(&path, &Value::Object(object))
}

pub fn is_sand_remote_room_dir(agent_dir: impl AsRef<Path>) -> bool {
    read_sand_remote_room_config(agent_dir).is_some()
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
            .unwrap_or_else(|| Value::Number(Number::from(REMOTE_ROOM_CONFIG_VERSION as u64)))
    }
}

fn write_pretty_json(path: &Path, value: &Value) -> io::Result<()> {
    let mut raw = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    raw.push('\n');
    fs::write(path, raw)
}
