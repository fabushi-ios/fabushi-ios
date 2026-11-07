use std::collections::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

pub const LEGACY_IMPLICIT_AGENT_ID: &str = "main";
pub const SYSTEM_AGENT_ROSTER_IDS: &[&str] = &["system", "gateway"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum AgentRosterProperty {
    #[serde(rename = "entries")]
    Entries(Map<String, Value>),
    #[serde(rename = "list")]
    List(Vec<Value>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedAgentEntry {
    pub id: String,
    pub entry: Value,
    pub source_kind: String, // "entries" or "list"
    pub source_ref: String,  // key or index string
}

pub fn normalize_agent_id(id: &str) -> String {
    let trimmed = id.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        LEGACY_IMPLICIT_AGENT_ID.to_string()
    } else {
        trimmed
    }
}

pub fn read_agent_roster_property(raw: &Value) -> Option<AgentRosterProperty> {
    let obj = raw.as_object()?;
    let agents = obj.get("agents")?.as_object()?;

    if let Some(entries) = agents.get("entries") {
        if let Some(map) = entries.as_object() {
            return Some(AgentRosterProperty::Entries(map.clone()));
        }
    }
    if let Some(list) = agents.get("list") {
        if let Some(arr) = list.as_array() {
            return Some(AgentRosterProperty::List(arr.clone()));
        }
    }
    None
}

pub fn has_agent_roster_property(raw: &Value) -> bool {
    read_agent_roster_property(raw).is_some()
}

pub fn collect_agent_entries(
    cfg: &Value,
    limit: Option<usize>,
) -> Vec<ListedAgentEntry> {
    let roster = match read_agent_roster_property(cfg) {
        Some(r) => r,
        None => return Vec::new(),
    };

    let mut result = Vec::new();
    match roster {
        AgentRosterProperty::Entries(map) => {
            for (id, val) in map {
                if let Some(mut entry_obj) = val.as_object().cloned() {
                    entry_obj.insert("id".to_string(), json!(id));
                    result.push(ListedAgentEntry {
                        id: normalize_agent_id(&id),
                        entry: Value::Object(entry_obj),
                        source_kind: "entries".to_string(),
                        source_ref: id,
                    });
                    if let Some(l) = limit {
                        if result.len() >= l {
                            break;
                        }
                    }
                }
            }
        }
        AgentRosterProperty::List(list) => {
            for (idx, item) in list.into_iter().enumerate() {
                if let Some(obj) = item.as_object() {
                    let id = obj
                        .get("id")
                        .and_then(Value::as_str)
                        .map(normalize_agent_id)
                        .unwrap_or_else(|| LEGACY_IMPLICIT_AGENT_ID.to_string());
                    result.push(ListedAgentEntry {
                        id,
                        entry: Value::Object(obj.clone()),
                        source_kind: "list".to_string(),
                        source_ref: idx.to_string(),
                    });
                    if let Some(l) = limit {
                        if result.len() >= l {
                            break;
                        }
                    }
                }
            }
        }
    }
    result
}

pub fn list_agent_entries(cfg: &Value) -> Vec<Value> {
    collect_agent_entries(cfg, None)
        .into_iter()
        .map(|item| item.entry)
        .collect()
}

pub fn list_agent_ids(cfg: &Value) -> Vec<String> {
    let entries = collect_agent_entries(cfg, None);
    if entries.is_empty() && !has_agent_roster_property(cfg) {
        return vec![LEGACY_IMPLICIT_AGENT_ID.to_string()];
    }
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for entry in entries {
        if seen.insert(entry.id.clone()) {
            ids.push(entry.id);
        }
    }
    ids
}

pub fn list_gateway_owned_agent_ids(cfg: &Value) -> Vec<String> {
    let mut ids = list_agent_ids(cfg);
    for sys_id in SYSTEM_AGENT_ROSTER_IDS {
        let norm = sys_id.to_string();
        if !ids.contains(&norm) {
            ids.push(norm);
        }
    }
    ids
}

pub fn try_resolve_sole_agent_id(cfg: &Value) -> Option<String> {
    let entries = collect_agent_entries(cfg, Some(2));
    if entries.is_empty() {
        if !has_agent_roster_property(cfg) {
            return Some(LEGACY_IMPLICIT_AGENT_ID.to_string());
        }
        return None;
    }
    if entries.len() == 1 {
        Some(entries[0].id.clone())
    } else {
        None
    }
}

pub fn try_resolve_raw_legacy_default_agent_id(cfg: &Value) -> Option<String> {
    let is_explicit = cfg
        .get("agents")
        .and_then(|a| a.get("ownership"))
        .and_then(Value::as_str)
        == Some("explicit");
    if is_explicit {
        return None;
    }
    let entries = collect_agent_entries(cfg, None);
    let marked: Vec<_> = entries
        .into_iter()
        .filter(|e| {
            e.entry
                .get("default")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .collect();
    if marked.len() == 1 {
        Some(marked[0].id.clone())
    } else {
        None
    }
}

pub fn try_resolve_default_agent_id(cfg: &Value) -> Option<String> {
    try_resolve_raw_legacy_default_agent_id(cfg).or_else(|| try_resolve_sole_agent_id(cfg))
}

pub fn resolve_configured_acp_subagent_target_ids(cfg: &Value) -> HashSet<String> {
    let mut ids: HashSet<String> = list_agent_ids(cfg).into_iter().collect();

    for entry in list_agent_entries(cfg) {
        if let Some(runtime) = entry.get("runtime").and_then(Value::as_object) {
            if runtime.get("type").and_then(Value::as_str) == Some("acp") {
                if let Some(target) = runtime
                    .get("acp")
                    .and_then(Value::as_object)
                    .and_then(|a| a.get("agent"))
                    .and_then(Value::as_str)
                {
                    ids.insert(normalize_agent_id(target));
                }
            }
        }
    }

    if let Some(acp) = cfg.get("acp").and_then(Value::as_object) {
        if let Some(def_agent) = acp.get("defaultAgent").and_then(Value::as_str) {
            ids.insert(normalize_agent_id(def_agent));
        }
        if let Some(allowed) = acp.get("allowedAgents").and_then(Value::as_array) {
            for item in allowed {
                if let Some(s) = item.as_str() {
                    let trimmed = s.trim();
                    if trimmed != "*" && !trimmed.is_empty() {
                        ids.insert(normalize_agent_id(trimmed));
                    }
                }
            }
        }
    }
    ids
}

// ---------------------------------------------------------------------------
// Roster Include Provenance (src/config/agent-roster-provenance.ts)
// ---------------------------------------------------------------------------

pub const INCLUDE_KEY: &str = "$include";

pub fn include_contribution_owns_agent_roster(path: &[&str], value: &Value) -> bool {
    if path.is_empty() {
        return has_agent_roster_property(value);
    }
    if path.len() == 1 && path[0] == "agents" {
        return value.as_object().map_or(false, |obj| {
            obj.contains_key("entries") || obj.contains_key("list")
        });
    }
    if path[0] != "agents" {
        return false;
    }
    if path.get(1) == Some(&"entries") {
        return path.len() <= 3 || path.get(3) == Some(&"default");
    }
    if path.get(1) == Some(&"list") {
        return path.len() <= 3 || path.get(3) == Some(&"id") || path.get(3) == Some(&"default");
    }
    false
}

pub fn include_contribution_owns_bindings(path: &[&str], value: &Value) -> bool {
    if path.is_empty() {
        return value
            .as_object()
            .map_or(false, |obj| obj.contains_key("bindings"));
    }
    path[0] == "bindings"
}

fn roster_entry_boundary_contains_include(val: &Value) -> bool {
    let obj = match val.as_object() {
        Some(o) => o,
        None => return false,
    };
    if obj.contains_key(INCLUDE_KEY) {
        return true;
    }
    for field in ["id", "default"] {
        if let Some(f_val) = obj.get(field) {
            if let Some(f_obj) = f_val.as_object() {
                if f_obj.contains_key(INCLUDE_KEY) {
                    return true;
                }
            }
        }
    }
    false
}

fn authored_roster_boundary_contains_include(val: &Value) -> bool {
    if let Some(obj) = val.as_object() {
        if obj.contains_key(INCLUDE_KEY) {
            return true;
        }
        return obj.values().any(roster_entry_boundary_contains_include);
    }
    if let Some(arr) = val.as_array() {
        return arr.iter().any(roster_entry_boundary_contains_include);
    }
    false
}

fn read_roster_value(raw: &Value) -> Option<&Value> {
    let obj = raw.as_object()?;
    let agents = obj.get("agents")?.as_object()?;
    if let Some(entries) = agents.get("entries") {
        return Some(entries);
    }
    agents.get("list")
}

pub fn config_include_owns_agent_roster_values(
    parsed: &Value,
    resolved: &Value,
    include_contributes_roster: bool,
) -> bool {
    if !has_agent_roster_property(resolved) {
        return false;
    }
    if let Some(authored_roster) = read_roster_value(parsed) {
        if authored_roster_boundary_contains_include(authored_roster) {
            return true;
        }
    }
    include_contributes_roster
}

// ---------------------------------------------------------------------------
// Implicit Roster Defaults (src/config/implicit-agent-roster.ts)
// ---------------------------------------------------------------------------

pub fn apply_implicit_agent_roster_defaults(raw: &Value) -> Value {
    let obj = match raw.as_object() {
        Some(o) => o,
        None => return raw.clone(),
    };
    if let Some(agents_val) = obj.get("agents") {
        if !agents_val.is_object() {
            return raw.clone();
        }
    }
    let agents_obj = obj
        .get("agents")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    if agents_obj.get("ownership").and_then(Value::as_str) == Some("explicit") {
        return raw.clone();
    }
    if let Some(list_val) = agents_obj.get("list") {
        if !list_val.as_array().map_or(false, |a| a.is_empty()) {
            return raw.clone();
        }
    }
    if let Some(entries_val) = agents_obj.get("entries") {
        if !entries_val.as_object().map_or(false, |o| o.is_empty()) {
            return raw.clone();
        }
    }

    let mut defaults = agents_obj;
    defaults.remove("list");
    let mut new_entries = Map::new();
    new_entries.insert("main".to_string(), Value::Object(Map::new()));
    defaults.insert("entries".to_string(), Value::Object(new_entries));

    let mut new_raw = obj.clone();
    new_raw.insert("agents".to_string(), Value::Object(defaults));
    Value::Object(new_raw)
}

// ---------------------------------------------------------------------------
// Security Audit (src/security/audit-agent-roster.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityAuditFinding {
    pub check_id: String,
    pub severity: String,
    pub title: String,
    pub detail: String,
    pub remediation: String,
}

pub fn collect_agent_roster_findings(cfg: &Value) -> Vec<SecurityAuditFinding> {
    let agents = list_agent_entries(cfg);
    if agents.is_empty() && !has_agent_roster_property(cfg) {
        return Vec::new();
    }
    let default_count = agents
        .iter()
        .filter(|e| e.get("default").and_then(Value::as_bool) == Some(true))
        .count();

    let explicit_ownership = cfg
        .get("agents")
        .and_then(|a| a.get("ownership"))
        .and_then(Value::as_str)
        == Some("explicit");

    let resolvable = if explicit_ownership {
        default_count == 0
    } else {
        try_resolve_default_agent_id(cfg).is_some()
    };

    if resolvable {
        return Vec::new();
    }

    vec![SecurityAuditFinding {
        check_id: "config.agent_roster.invalid_default_count".to_string(),
        severity: "warn".to_string(),
        title: "Agent roster has an invalid default selection".to_string(),
        detail: if explicit_ownership {
            format!("Expected no agents.entries default=true entries with agents.ownership=explicit, found {default_count}.")
        } else {
            format!("Expected a resolvable default agent (sole entry, one default=true marker, or agents.ownership=explicit); found {default_count} default markers across {} configured agents.", agents.len())
        },
        remediation: "Run `openclaw doctor --fix` to repair the authored agent roster.".to_string(),
    }]
}

// ---------------------------------------------------------------------------
// Workspace Transition & Survivor Pinning (src/config/agent-workspace-roster-transition.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Agent \"{agent_id}\" deletion cleanup is still pending; finish or retry its deletion before changing its workspace, agent directory, or session store.")]
pub struct AgentDeletionTargetsPendingError {
    pub agent_id: String,
}

pub fn pin_survivor_workspace_for_roster_collapse(
    source_config: &Value,
    target_config: &Value,
    source_workspace_dir: Option<&str>,
) -> (Value, Vec<Vec<String>>) {
    let source_entries = collect_agent_entries(source_config, None);
    let target_entries = collect_agent_entries(target_config, None);

    if source_entries.len() <= 1 || target_entries.len() != 1 {
        return (target_config.clone(), Vec::new());
    }

    let survivor_id = target_entries[0].id.clone();
    if !source_entries.iter().any(|e| e.id == survivor_id) {
        return (target_config.clone(), Vec::new());
    }

    let mut new_target = target_config.clone();
    let mut target_agents = new_target
        .get("agents")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut entries_map = match target_agents.get("entries").and_then(Value::as_object) {
        Some(m) => m.clone(),
        None => {
            let mut m = Map::new();
            for item in &target_entries {
                m.insert(item.id.clone(), item.entry.clone());
            }
            m
        }
    };

    let entry_key = entries_map
        .keys()
        .find(|k| normalize_agent_id(k) == survivor_id)
        .cloned();

    let entry_key = match entry_key {
        Some(k) => k,
        None => return (target_config.clone(), Vec::new()),
    };

    let mut entry_obj = entries_map
        .get(&entry_key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let workspace_val = entry_obj.get("workspace").and_then(Value::as_str);
    let workspace_needs_pin = workspace_val.map_or(true, |w| w.trim().is_empty());

    if !workspace_needs_pin {
        return (target_config.clone(), Vec::new());
    }

    let pinned_dir = source_workspace_dir.unwrap_or("");
    entry_obj.insert("workspace".to_string(), json!(pinned_dir));
    entries_map.insert(entry_key.clone(), Value::Object(entry_obj));
    target_agents.insert("entries".to_string(), Value::Object(entries_map));
    new_target
        .as_object_mut()
        .map(|obj| obj.insert("agents".to_string(), Value::Object(target_agents)));

    (
        new_target,
        vec![vec![
            "agents".to_string(),
            "entries".to_string(),
            entry_key,
            "workspace".to_string(),
        ]],
    )
}

// ---------------------------------------------------------------------------
// Doctor Config Write Staging (src/commands/doctor/shared/roster-include-write.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorStageResult {
    pub config: Value,
    pub remaining_paths: Vec<Vec<String>>,
    pub persist_canonical_agent_roster: bool,
}

pub fn can_write_doctor_include(
    parsed: &Value,
    resolved: &Value,
    include_owns_roster: bool,
) -> bool {
    !config_include_owns_agent_roster_values(parsed, resolved, include_owns_roster)
}
