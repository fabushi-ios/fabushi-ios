use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{Value, json};

use crate::cursor_backend::{
    CursorBackendError, resolve_sand_ghost_mode_header, send_cursor_unary,
};
use crate::extensions::auth::extension::HostAuthExtension;

pub const DASHBOARD_GET_TEAMS_PATH: &str = "/aiserver.v1.DashboardService/GetTeams";
pub const DASHBOARD_GET_TEAM_RULES_PATH: &str =
    "/aiserver.v1.DashboardService/GetTeamRules";
pub const TEAM_RULES_REQUEST_TIMEOUT_MS: u64 = 10_000;
pub const TEAM_RULE_AGENT_TYPE_ALL: u64 = 1;
pub const TEAM_RULE_AGENT_TYPE_SAND: u64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorTeamRuleType {
    Global,
    FileGlobbed { globs: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorTeamRule {
    pub full_path: String,
    pub content: String,
    pub rule_type: CursorTeamRuleType,
    pub is_required: bool,
}

impl CursorTeamRule {
    pub fn to_json(&self) -> Value {
        let rule_type = match &self.rule_type {
            CursorTeamRuleType::Global => json!({ "kind": "global" }),
            CursorTeamRuleType::FileGlobbed { globs } => {
                json!({ "kind": "fileGlobbed", "globs": globs })
            }
        };
        json!({
            "fullPath": self.full_path,
            "content": self.content,
            "type": rule_type,
            "source": "team",
            "isRequired": self.is_required,
        })
    }
}

fn decode_varint(input: &[u8], cursor: &mut usize) -> Result<u64, CursorBackendError> {
    let mut value = 0_u64;
    let mut shift = 0_u32;
    while *cursor < input.len() && shift < 64 {
        let byte = input[*cursor];
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
    Err(CursorBackendError::InvalidProto("malformed varint".into()))
}

fn read_length_delimited<'a>(
    input: &'a [u8],
    cursor: &mut usize,
) -> Result<&'a [u8], CursorBackendError> {
    let length = usize::try_from(decode_varint(input, cursor)?)
        .map_err(|_| CursorBackendError::InvalidProto("length overflow".into()))?;
    let end = cursor.saturating_add(length);
    if end > input.len() {
        return Err(CursorBackendError::InvalidProto(
            "truncated length-delimited field".into(),
        ));
    }
    let value = &input[*cursor..end];
    *cursor = end;
    Ok(value)
}

fn skip_field(
    input: &[u8],
    cursor: &mut usize,
    wire_type: u8,
) -> Result<(), CursorBackendError> {
    match wire_type {
        0 => {
            let _ = decode_varint(input, cursor)?;
        }
        1 => *cursor = cursor.saturating_add(8),
        2 => {
            let _ = read_length_delimited(input, cursor)?;
        }
        5 => *cursor = cursor.saturating_add(4),
        other => {
            return Err(CursorBackendError::InvalidProto(format!(
                "unsupported wire type {other}"
            )));
        }
    }
    if *cursor > input.len() {
        return Err(CursorBackendError::InvalidProto(
            "truncated protobuf field".into(),
        ));
    }
    Ok(())
}

fn repeated_message_fields<'a>(
    input: &'a [u8],
    wanted_field: u64,
) -> Result<Vec<&'a [u8]>, CursorBackendError> {
    let mut cursor = 0usize;
    let mut values = Vec::new();
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if field == wanted_field && wire == 2 {
            values.push(read_length_delimited(input, &mut cursor)?);
        } else {
            skip_field(input, &mut cursor, wire)?;
        }
    }
    Ok(values)
}

fn optional_varint_field(
    input: &[u8],
    wanted_field: u64,
) -> Result<Option<u64>, CursorBackendError> {
    let mut cursor = 0usize;
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if field == wanted_field && wire == 0 {
            return Ok(Some(decode_varint(input, &mut cursor)?));
        }
        skip_field(input, &mut cursor, wire)?;
    }
    Ok(None)
}

fn string_fields(
    input: &[u8],
    wanted_field: u64,
) -> Result<Vec<String>, CursorBackendError> {
    let mut cursor = 0usize;
    let mut values = Vec::new();
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if field == wanted_field && wire == 2 {
            let raw = read_length_delimited(input, &mut cursor)?;
            let value = std::str::from_utf8(raw)
                .map_err(|_| CursorBackendError::InvalidProto("string field is not UTF-8".into()))?
                .trim();
            if !value.is_empty() {
                values.push(value.to_string());
            }
        } else {
            skip_field(input, &mut cursor, wire)?;
        }
    }
    Ok(values)
}

fn first_string_field(
    input: &[u8],
    wanted_field: u64,
) -> Result<Option<String>, CursorBackendError> {
    Ok(string_fields(input, wanted_field)?.into_iter().next())
}

fn push_varint(mut value: u64, output: &mut Vec<u8>) {
    loop {
        if value < 0x80 {
            output.push(value as u8);
            return;
        }
        output.push(((value as u8) & 0x7f) | 0x80);
        value >>= 7;
    }
}

fn get_team_rules_request(team_id: u64) -> Vec<u8> {
    let mut body = vec![0x08, 0x01, 0x10];
    push_varint(team_id, &mut body);
    body
}

fn direct_team_ids(response: &[u8]) -> Result<Vec<u64>, CursorBackendError> {
    let mut ids = Vec::new();
    for team in repeated_message_fields(response, 1)? {
        let id = optional_varint_field(team, 2)?.unwrap_or_default();
        let direct = optional_varint_field(team, 36)?.unwrap_or_default() != 0;
        if id > 0 && direct {
            ids.push(id);
        }
    }
    Ok(ids)
}

fn decode_team_rules(response: &[u8]) -> Result<Vec<CursorTeamRule>, CursorBackendError> {
    let mut rules = Vec::new();
    for raw in repeated_message_fields(response, 1)? {
        let agent_type = optional_varint_field(raw, 7)?.unwrap_or_default();
        if agent_type != TEAM_RULE_AGENT_TYPE_ALL && agent_type != TEAM_RULE_AGENT_TYPE_SAND {
            continue;
        }
        let Some(full_path) = first_string_field(raw, 2)? else {
            continue;
        };
        let Some(content) = first_string_field(raw, 3)? else {
            continue;
        };
        let globs = string_fields(raw, 6)?;
        let rule_type = if globs.is_empty() {
            CursorTeamRuleType::Global
        } else {
            CursorTeamRuleType::FileGlobbed { globs }
        };
        rules.push(CursorTeamRule {
            full_path,
            content,
            rule_type,
            is_required: optional_varint_field(raw, 5)?.unwrap_or_default() != 0,
        });
    }
    Ok(rules)
}

pub fn merge_team_rules(
    groups: impl IntoIterator<Item = Vec<CursorTeamRule>>,
) -> Vec<CursorTeamRule> {
    let mut seen = BTreeSet::new();
    let mut merged = Vec::new();
    for group in groups {
        for rule in group {
            if seen.insert(rule.full_path.clone()) {
                merged.push(rule);
            }
        }
    }
    merged
}

pub fn fetch_sand_team_rules(
    backend_url: &str,
    access_token: &str,
    machine_id: &str,
) -> Result<Vec<Value>, CursorBackendError> {
    let ghost_mode = resolve_sand_ghost_mode_header(backend_url, access_token, machine_id);
    let teams = send_cursor_unary(
        backend_url,
        access_token,
        machine_id,
        DASHBOARD_GET_TEAMS_PATH,
        &[0x08, 0x01],
        TEAM_RULES_REQUEST_TIMEOUT_MS,
        ghost_mode,
    )?;
    let team_ids = direct_team_ids(&teams)?;
    if team_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut groups = Vec::with_capacity(team_ids.len());
    for team_id in team_ids {
        let response = send_cursor_unary(
            backend_url,
            access_token,
            machine_id,
            DASHBOARD_GET_TEAM_RULES_PATH,
            &get_team_rules_request(team_id),
            TEAM_RULES_REQUEST_TIMEOUT_MS,
            ghost_mode,
        )?;
        groups.push(decode_team_rules(&response)?);
    }
    Ok(merge_team_rules(groups)
        .into_iter()
        .map(|rule| rule.to_json())
        .collect())
}

pub struct ProductionTeamRulesResolver {
    backend_url: String,
    auth: Arc<HostAuthExtension>,
    snapshot: Mutex<Option<Vec<Value>>>,
    refresh_lock: Mutex<()>,
}

impl ProductionTeamRulesResolver {
    pub fn new(backend_url: String, auth: Arc<HostAuthExtension>) -> Self {
        Self {
            backend_url,
            auth,
            snapshot: Mutex::new(None),
            refresh_lock: Mutex::new(()),
        }
    }

    pub fn preload(self: &Arc<Self>) {
        let resolver = Arc::clone(self);
        let _ = thread::Builder::new()
            .name("host-managed-team-rules-initial".into())
            .spawn(move || {
                let _ = resolver.refresh();
            });
    }

    pub fn refresh(&self) -> Option<Vec<Value>> {
        let _refresh = self
            .refresh_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let access_token = match self.auth.get_access_token() {
            Ok(value) => value,
            Err(error) => {
                self.auth
                    .service()
                    .log(&format!("managed team rules await Auth: {error}"));
                return self.snapshot();
            }
        };
        let machine_id = match self.auth.get_machine_id() {
            Ok(value) => value,
            Err(error) => {
                self.auth
                    .service()
                    .log(&format!("managed team rules machine id unavailable: {error}"));
                return self.snapshot();
            }
        };
        match fetch_sand_team_rules(&self.backend_url, &access_token, &machine_id) {
            Ok(rules) => {
                *self
                    .snapshot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(rules.clone());
                Some(rules)
            }
            Err(error) => {
                self.auth.service().log(&format!(
                    "managed team rules refresh failed; preserving previous snapshot: {error}"
                ));
                self.snapshot()
            }
        }
    }

    pub fn resolve_rules(&self) -> Option<Vec<Value>> {
        if let Some(snapshot) = self.snapshot() {
            return Some(snapshot);
        }

        // Keep Cursor/Sand managed-rule discovery off the first-output critical
        // path when this desktop has no Cursor renewal credential. Fabushi
        // account credentials belong exclusively to first-party Fabushi
        // account/Responses transport, so a missing Cursor token is an expected
        // state for the signed Fabushi product rather than a reason to trigger
        // synchronous renewal work on every ordinary turn. Once HostAuth has a
        // cached Cursor token, refresh remains available and preserves the
        // production managed-rules behavior for Cursor-authenticated installs.
        if self.auth.peek_access_token().is_none() {
            return None;
        }

        self.refresh()
    }

    pub fn snapshot(&self) -> Option<Vec<Value>> {
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}
