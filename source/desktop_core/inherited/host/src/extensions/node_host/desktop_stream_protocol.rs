//! Strict node desktop/portal stream command input admission.
//!
//! This parser owns only the worker-command contract. It does not read a
//! password file, probe RFB, open a socket, or publish a Gateway stream.

use serde_json::Value;
use std::path::PathBuf;

pub const NODE_DESKTOP_ATTACH_PATH: &str = "/node-desktop/attach";
pub const NODE_PORTAL_ATTACH_PATH: &str = "/node-portal/attach";
pub const STREAM_REQUEST_MAX_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeWorkerDesktopStreamRequest {
    pub ticket: String,
    pub attach_path: String,
    pub port: u16,
    pub password_file_path: Option<PathBuf>,
    pub username: Option<String>,
}

pub fn parse_node_worker_desktop_stream_input(
    raw: Option<&str>,
) -> Result<NodeWorkerDesktopStreamRequest, String> {
    let value = parse_request(raw, "node worker desktop")?;
    let object = value
        .as_object()
        .ok_or_else(|| "INVALID_REQUEST: invalid node worker desktop stream request".to_string())?;
    require_keys(object, &["ticket", "attachPath", "port"], &["passwordFilePath", "username"])?;
    let ticket = object
        .get("ticket")
        .and_then(Value::as_str)
        .filter(|value| is_ticket(value))
        .ok_or_else(|| "INVALID_REQUEST: invalid node worker desktop stream request".to_string())?;
    let attach_path = object
        .get("attachPath")
        .and_then(Value::as_str)
        .filter(|value| *value == format!("{NODE_DESKTOP_ATTACH_PATH}?ticket={ticket}"))
        .ok_or_else(|| "INVALID_REQUEST: invalid node worker desktop stream request".to_string())?;
    let port = object
        .get("port")
        .and_then(Value::as_u64)
        .filter(|value| (1..=u16::MAX as u64).contains(value))
        .map(|value| value as u16)
        .ok_or_else(|| "INVALID_REQUEST: invalid node worker desktop stream request".to_string())?;
    let password_file_path = optional_absolute_path(object.get("passwordFilePath"))?;
    let username = object
        .get("username")
        .map(|value| {
            let username = value.as_str().unwrap_or_default();
            if username.is_empty() || username.len() > 63 || username.contains('\0') {
                return Err("INVALID_REQUEST: invalid desktop username".to_string());
            }
            if password_file_path.is_none() {
                return Err("INVALID_REQUEST: desktop username requires a password file".to_string());
            }
            Ok(username.to_string())
        })
        .transpose()?;
    Ok(NodeWorkerDesktopStreamRequest {
        ticket: ticket.to_string(),
        attach_path: attach_path.to_string(),
        port,
        password_file_path,
        username,
    })
}

pub fn parse_node_worker_portal_stream_input(
    raw: Option<&str>,
) -> Result<(String, String, u16), String> {
    let value = parse_request(raw, "node worker portal stream")?;
    let object = value
        .as_object()
        .ok_or_else(|| "INVALID_REQUEST: invalid node worker portal stream request".to_string())?;
    require_keys(object, &["ticket", "attachPath", "port"], &[])?;
    let ticket = object
        .get("ticket")
        .and_then(Value::as_str)
        .filter(|value| is_ticket(value))
        .ok_or_else(|| "INVALID_REQUEST: invalid node worker portal stream request".to_string())?;
    let attach_path = format!("{NODE_PORTAL_ATTACH_PATH}?ticket={ticket}");
    if object.get("attachPath").and_then(Value::as_str) != Some(attach_path.as_str()) {
        return Err("INVALID_REQUEST: invalid node worker portal stream request".into());
    }
    let port = object
        .get("port")
        .and_then(Value::as_u64)
        .filter(|value| (1..=u16::MAX as u64).contains(value))
        .map(|value| value as u16)
        .ok_or_else(|| "INVALID_REQUEST: invalid node worker portal stream request".to_string())?;
    Ok((ticket.to_string(), attach_path, port))
}

fn parse_request(raw: Option<&str>, label: &str) -> Result<Value, String> {
    let raw = raw.filter(|value| !value.is_empty()).ok_or_else(|| {
        format!("INVALID_REQUEST: invalid {label} request")
    })?;
    if raw.len() > STREAM_REQUEST_MAX_BYTES {
        return Err(format!("INVALID_REQUEST: invalid {label} request"));
    }
    serde_json::from_str(raw).map_err(|_| format!("INVALID_REQUEST: malformed {label} request"))
}

fn require_keys(
    object: &serde_json::Map<String, Value>,
    required: &[&str],
    optional: &[&str],
) -> Result<(), String> {
    if required.iter().any(|key| !object.contains_key(*key))
        || object
            .keys()
            .any(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err("INVALID_REQUEST: unsupported stream request fields".into());
    }
    Ok(())
}

fn optional_absolute_path(value: Option<&Value>) -> Result<Option<PathBuf>, String> {
    let Some(value) = value else { return Ok(None) };
    let path = value
        .as_str()
        .filter(|value| !value.is_empty() && !value.contains('\0'))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| "INVALID_REQUEST: passwordFilePath must be absolute".to_string())?;
    Ok(Some(path))
}

fn is_ticket(value: &str) -> bool {
    value.len() == 48 && value.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::{parse_node_worker_desktop_stream_input, parse_node_worker_portal_stream_input};

    const TICKET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn desktop_stream_requires_exact_ticket_path_and_port() {
        let raw = format!(
            r#"{{"ticket":"{TICKET}","attachPath":"/node-desktop/attach?ticket={TICKET}","port":5900}}"#
        );
        let request = parse_node_worker_desktop_stream_input(Some(&raw)).expect("valid request");
        assert_eq!(request.port, 5900);
        assert!(parse_node_worker_desktop_stream_input(Some(
            r#"{"ticket":"ABC","attachPath":"/node-desktop/attach?ticket=ABC","port":5900}"#
        ))
        .is_err());
    }

    #[test]
    fn portal_stream_rejects_unknown_fields_and_invalid_port() {
        let raw = format!(
            r#"{{"ticket":"{TICKET}","attachPath":"/node-portal/attach?ticket={TICKET}","port":65535}}"#
        );
        assert_eq!(parse_node_worker_portal_stream_input(Some(&raw)).expect("valid request").2, 65535);
        assert!(parse_node_worker_portal_stream_input(Some(
            r#"{"ticket":"0123456789abcdef0123456789abcdef0123456789abcdef","attachPath":"/node-portal/attach?ticket=0123456789abcdef0123456789abcdef0123456789abcdef","port":1,"extra":true}"#
        ))
        .is_err());
    }
}
