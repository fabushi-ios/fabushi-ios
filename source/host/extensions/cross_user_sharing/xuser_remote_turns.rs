use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const REMOTE_TURN_BUDGET_MAX: usize = 30;
pub const REMOTE_TURN_BUDGET_WINDOW_MS: u64 = 10 * 60_000;
pub const REMOTE_MEMBER_UNREACHABLE_MS: u64 = 1_060_000;
pub const REMOTE_MEMBER_RETRY_BACKOFF_MS: u64 = 10_000;
pub const REMOTE_AGENT_ID_PREFIX: &str = "sand-remote:";
pub const SAND_XUSER_OUTBOUND_TURNS_FILE_NAME: &str = "host-xuser-outbound-turns.json";
const OUTBOUND_TERMINAL_TTL_MS: u64 = 3 * 24 * 60 * 60 * 1_000;

#[derive(Default)]
pub struct RemoteTurnBudget {
    entries: Mutex<BTreeMap<String, VecDeque<u64>>>,
}

impl RemoteTurnBudget {
    pub fn new() -> Self { Self::default() }

    pub fn accept(&self, key: &str, now_ms: u64) -> bool {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        let queue = entries.entry(key.to_string()).or_default();
        while queue.front().is_some_and(|seen| now_ms.saturating_sub(*seen) >= REMOTE_TURN_BUDGET_WINDOW_MS) {
            queue.pop_front();
        }
        if queue.len() >= REMOTE_TURN_BUDGET_MAX {
            return false;
        }
        queue.push_back(now_ms);
        true
    }

    pub fn clear(&self) {
        self.entries.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
}

pub fn parse_remote_agent_id(value: &str) -> Option<(String, String)> {
    let encoded = value.strip_prefix(REMOTE_AGENT_ID_PREFIX)?;
    let (owner, agent) = encoded.split_once('/')?;
    let owner = percent_decode(owner)?;
    let agent = percent_decode(agent)?;
    if owner.is_empty() || agent.is_empty() { None } else { Some((owner, agent)) }
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() { return None; }
            let high = hex(bytes[index + 1])?;
            let low = hex(bytes[index + 2])?;
            output.push((high << 4) | low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}

fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundTurnSettlement {
    pub nonce: String,
    pub room_id: String,
    pub owner_auth_id: String,
    pub agent_id: String,
    pub member_name: String,
    pub origin_operation_id: Option<String>,
    pub messages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundResultDisposition {
    Settled(OutboundTurnSettlement),
    Duplicate,
    Late,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredOutboundTurn {
    nonce: String,
    generation: u64,
    member_key: String,
    room_id: String,
    host_auth_id: String,
    owner_auth_id: String,
    agent_id: String,
    member_name: String,
    #[serde(default)]
    origin_operation_id: Option<String>,
    payload: Value,
    created_at_ms: u64,
    next_attempt_at_ms: u64,
    attempt_count: u32,
    last_send_succeeded: bool,
    #[serde(default)]
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredTerminalTurn {
    generation: u64,
    member_key: String,
    room_id: String,
    owner_auth_id: String,
    agent_id: String,
    member_name: String,
    #[serde(default)]
    origin_operation_id: Option<String>,
    terminal_at_ms: u64,
    expires_at_ms: u64,
    reason: String,
    #[serde(default)]
    messages: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PersistedOutboundState {
    version: u32,
    pending: BTreeMap<String, StoredOutboundTurn>,
    terminal: BTreeMap<String, StoredTerminalTurn>,
    unreachable_since_ms: BTreeMap<String, u64>,
    unreachable_until_ms: BTreeMap<String, u64>,
}

#[derive(Default)]
struct RuntimeState {
    loaded: bool,
    stopped: bool,
    data: PersistedOutboundState,
}

type RelaySend = Arc<dyn Fn(&Value) -> Result<Value, String> + Send + Sync>;
type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub struct SandXuserOutboundTurns {
    file_path: PathBuf,
    send: RelaySend,
    now_ms: Clock,
    state: Mutex<RuntimeState>,
    budget: RemoteTurnBudget,
}

impl SandXuserOutboundTurns {
    pub fn new(root: &Path, send: RelaySend, now_ms: Clock) -> Self {
        Self {
            file_path: root.join(SAND_XUSER_OUTBOUND_TURNS_FILE_NAME),
            send,
            now_ms,
            state: Mutex::new(RuntimeState::default()),
            budget: RemoteTurnBudget::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn request_remote_member_turn(
        &self,
        generation: u64,
        room_id: &str,
        host_auth_id: &str,
        owner_auth_id: &str,
        agent_id: &str,
        member_name: &str,
        group_name: &str,
        group_description: &str,
        peers: Value,
        new_messages: Value,
        origin_operation_id: Option<&str>,
    ) -> Result<Option<String>, String> {
        let room_id = required(room_id, "roomId")?;
        let host_auth_id = required(host_auth_id, "hostAuthId")?;
        let owner_auth_id = required(owner_auth_id, "ownerAuthId")?;
        let agent_id = required(agent_id, "agentId")?;
        let member_name = required(member_name, "memberName")?;
        let member_key = format!("{room_id}\0{owner_auth_id}\0{agent_id}");
        let now = (self.now_ms)();

        {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            self.ensure_loaded(&mut state)?;
            prune(&mut state.data, now);
            if state.stopped {
                return Ok(None);
            }
            if state
                .data
                .unreachable_until_ms
                .get(&member_key)
                .is_some_and(|until| *until > now)
            {
                return Ok(None);
            }
        }
        if !self.budget.accept(&member_key, now) {
            return Ok(None);
        }

        let nonce = uuid::Uuid::new_v4().to_string();
        let payload = json!({
            "kind": "turn-request",
            "roomId": room_id,
            "hostAuthId": host_auth_id,
            "ownerAuthId": owner_auth_id,
            "agentId": agent_id,
            "turnNonce": nonce,
            "groupName": group_name,
            "groupDescription": group_description,
            "peers": peers,
            "newMessages": new_messages,
        });
        let turn = StoredOutboundTurn {
            nonce: nonce.clone(),
            generation,
            member_key,
            room_id,
            host_auth_id,
            owner_auth_id,
            agent_id,
            member_name,
            origin_operation_id: origin_operation_id.map(str::to_string),
            payload,
            created_at_ms: now,
            next_attempt_at_ms: now,
            attempt_count: 0,
            last_send_succeeded: false,
            last_error: None,
        };
        self.mutate(|data| {
            data.pending.insert(nonce.clone(), turn);
            Ok(())
        })?;
        let _ = self.attempt_send(&nonce, generation);
        Ok(Some(nonce))
    }

    pub fn tick(&self, generation: u64) -> Result<(), String> {
        let now = (self.now_ms)();
        let pending = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            self.ensure_loaded(&mut state)?;
            if state.stopped {
                return Ok(());
            }
            prune(&mut state.data, now);
            state.data.pending.values().cloned().collect::<Vec<_>>()
        };

        for turn in pending {
            if turn.generation != generation {
                self.terminalize(&turn.nonce, "stale-account", now, false)?;
                continue;
            }
            let unreachable_since = {
                let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
                self.ensure_loaded(&mut state)?;
                state.data.unreachable_since_ms.get(&turn.member_key).copied()
            };
            if unreachable_since.is_some_and(|since| now.saturating_sub(since) >= REMOTE_MEMBER_UNREACHABLE_MS) {
                self.terminalize(&turn.nonce, "unreachable", now, true)?;
                continue;
            }
            if now.saturating_sub(turn.created_at_ms) >= REMOTE_MEMBER_UNREACHABLE_MS {
                self.terminalize(&turn.nonce, "timeout", now, true)?;
                continue;
            }
            if !turn.last_send_succeeded && now >= turn.next_attempt_at_ms {
                let _ = self.attempt_send(&turn.nonce, generation);
            }
        }
        Ok(())
    }

    pub fn handle_turn_result(
        &self,
        generation: u64,
        event: &Value,
    ) -> Result<OutboundResultDisposition, String> {
        let nonce = event
            .get("turnNonce")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "turnNonce is required".to_string())?
            .to_string();
        let messages = event
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .take(2)
            .collect::<Vec<_>>();
        let now = (self.now_ms)();

        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        self.ensure_loaded(&mut state)?;
        prune(&mut state.data, now);
        if let Some(terminal) = state.data.terminal.get(&nonce) {
            return Ok(if terminal.reason == "completed" {
                OutboundResultDisposition::Duplicate
            } else {
                OutboundResultDisposition::Late
            });
        }
        let Some(turn) = state.data.pending.get(&nonce).cloned() else {
            return Ok(OutboundResultDisposition::Late);
        };
        if turn.generation != generation {
            drop(state);
            self.terminalize(&nonce, "stale-account", now, false)?;
            return Ok(OutboundResultDisposition::Late);
        }
        let before = state.data.clone();
        state.data.pending.remove(&nonce);
        state.data.unreachable_since_ms.remove(&turn.member_key);
        state.data.unreachable_until_ms.remove(&turn.member_key);
        state.data.terminal.insert(
            nonce.clone(),
            StoredTerminalTurn {
                generation,
                member_key: turn.member_key.clone(),
                room_id: turn.room_id.clone(),
                owner_auth_id: turn.owner_auth_id.clone(),
                agent_id: turn.agent_id.clone(),
                member_name: turn.member_name.clone(),
                origin_operation_id: turn.origin_operation_id.clone(),
                terminal_at_ms: now,
                expires_at_ms: now.saturating_add(OUTBOUND_TERMINAL_TTL_MS),
                reason: "completed".into(),
                messages: messages.clone(),
            },
        );
        if let Err(error) = persist(&self.file_path, &state.data) {
            state.data = before;
            return Err(error);
        }
        Ok(OutboundResultDisposition::Settled(OutboundTurnSettlement {
            nonce,
            room_id: turn.room_id,
            owner_auth_id: turn.owner_auth_id,
            agent_id: turn.agent_id,
            member_name: turn.member_name,
            origin_operation_id: turn.origin_operation_id,
            messages,
        }))
    }

    pub fn cancel(&self, generation: u64, nonce: &str) -> Result<bool, String> {
        let nonce = nonce.trim();
        if nonce.is_empty() {
            return Ok(false);
        }
        let now = (self.now_ms)();
        let matches = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            self.ensure_loaded(&mut state)?;
            state
                .data
                .pending
                .get(nonce)
                .is_some_and(|turn| turn.generation == generation)
        };
        if !matches {
            return Ok(false);
        }
        self.terminalize(nonce, "cancelled", now, false)?;
        Ok(true)
    }

    pub fn reset_account(&self, generation: u64) -> Result<(), String> {
        let now = (self.now_ms)();
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        self.ensure_loaded(&mut state)?;
        let before = state.data.clone();
        let turns = state.data.pending.values().cloned().collect::<Vec<_>>();
        state.data.pending.clear();
        for turn in turns {
            state.data.terminal.insert(
                turn.nonce.clone(),
                terminal_from(&turn, generation, now, "stale-account", Vec::new()),
            );
        }
        state.data.unreachable_since_ms.clear();
        state.data.unreachable_until_ms.clear();
        if let Err(error) = persist(&self.file_path, &state.data) {
            state.data = before;
            return Err(error);
        }
        self.budget.clear();
        Ok(())
    }

    pub fn stop(&self) {
        self.budget.clear();
        if let Ok(mut state) = self.state.lock() {
            state.stopped = true;
        }
    }

    #[cfg(test)]
    fn pending_nonces(&self) -> Result<Vec<String>, String> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        self.ensure_loaded(&mut state)?;
        Ok(state.data.pending.keys().cloned().collect())
    }

    #[cfg(test)]
    fn terminal_reason(&self, nonce: &str) -> Result<Option<String>, String> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        self.ensure_loaded(&mut state)?;
        Ok(state.data.terminal.get(nonce).map(|turn| turn.reason.clone()))
    }

    fn attempt_send(&self, nonce: &str, generation: u64) -> Result<bool, String> {
        let now = (self.now_ms)();
        let turn = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            self.ensure_loaded(&mut state)?;
            if state.stopped {
                return Ok(false);
            }
            let Some(turn) = state.data.pending.get(nonce) else {
                return Ok(false);
            };
            if turn.generation != generation || turn.last_send_succeeded || now < turn.next_attempt_at_ms {
                return Ok(false);
            }
            turn.clone()
        };

        let result = (self.send)(&turn.payload);
        let mut should_terminalize = false;
        {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            self.ensure_loaded(&mut state)?;
            let before = state.data.clone();
            {
                let Some(current) = state.data.pending.get_mut(nonce) else {
                    return Ok(false);
                };
                if current.generation != generation {
                    return Ok(false);
                }
                current.attempt_count = current.attempt_count.saturating_add(1);
                match &result {
                    Ok(_) => {
                        current.last_send_succeeded = true;
                        current.last_error = None;
                    }
                    Err(error) => {
                        current.last_send_succeeded = false;
                        current.last_error = Some(error.clone());
                        current.next_attempt_at_ms =
                            now.saturating_add(REMOTE_MEMBER_RETRY_BACKOFF_MS);
                    }
                }
            }
            match &result {
                Ok(_) => {
                    state.data.unreachable_since_ms.remove(&turn.member_key);
                    state.data.unreachable_until_ms.remove(&turn.member_key);
                }
                Err(_) => {
                    let since = *state
                        .data
                        .unreachable_since_ms
                        .entry(turn.member_key.clone())
                        .or_insert(now);
                    should_terminalize =
                        now.saturating_sub(since) >= REMOTE_MEMBER_UNREACHABLE_MS;
                }
            }
            if let Err(error) = persist(&self.file_path, &state.data) {
                state.data = before;
                return Err(error);
            }
        }
        if should_terminalize {
            self.terminalize(nonce, "unreachable", now, true)?;
        }
        Ok(result.is_ok())
    }

    fn terminalize(
        &self,
        nonce: &str,
        reason: &str,
        now: u64,
        backoff_member: bool,
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        self.ensure_loaded(&mut state)?;
        let Some(turn) = state.data.pending.get(nonce).cloned() else {
            return Ok(());
        };
        let before = state.data.clone();
        state.data.pending.remove(nonce);
        state.data.unreachable_since_ms.remove(&turn.member_key);
        if backoff_member {
            state.data.unreachable_until_ms.insert(
                turn.member_key.clone(),
                now.saturating_add(REMOTE_MEMBER_UNREACHABLE_MS),
            );
        }
        state.data.terminal.insert(
            nonce.to_string(),
            terminal_from(&turn, turn.generation, now, reason, Vec::new()),
        );
        if let Err(error) = persist(&self.file_path, &state.data) {
            state.data = before;
            return Err(error);
        }
        Ok(())
    }

    fn mutate(&self, operation: impl FnOnce(&mut PersistedOutboundState) -> Result<(), String>) -> Result<(), String> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        self.ensure_loaded(&mut state)?;
        let before = state.data.clone();
        operation(&mut state.data)?;
        if let Err(error) = persist(&self.file_path, &state.data) {
            state.data = before;
            return Err(error);
        }
        Ok(())
    }

    fn ensure_loaded(&self, state: &mut RuntimeState) -> Result<(), String> {
        if state.loaded {
            return Ok(());
        }
        let now = (self.now_ms)();
        let mut data = load(&self.file_path)?;
        prune(&mut data, now);
        // A restart makes the previous transport outcome uncertain. Resend the
        // exact same durable nonce; remote-side dedupe keeps execution exactly once.
        for turn in data.pending.values_mut() {
            turn.last_send_succeeded = false;
            turn.next_attempt_at_ms = now;
        }
        state.data = data;
        state.loaded = true;
        Ok(())
    }
}

fn terminal_from(
    turn: &StoredOutboundTurn,
    generation: u64,
    now: u64,
    reason: &str,
    messages: Vec<String>,
) -> StoredTerminalTurn {
    StoredTerminalTurn {
        generation,
        member_key: turn.member_key.clone(),
        room_id: turn.room_id.clone(),
        owner_auth_id: turn.owner_auth_id.clone(),
        agent_id: turn.agent_id.clone(),
        member_name: turn.member_name.clone(),
        origin_operation_id: turn.origin_operation_id.clone(),
        terminal_at_ms: now,
        expires_at_ms: now.saturating_add(OUTBOUND_TERMINAL_TTL_MS),
        reason: reason.to_string(),
        messages,
    }
}

fn required(value: &str, field: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        Err(format!("{field} is required"))
    } else {
        Ok(value.to_string())
    }
}

fn prune(data: &mut PersistedOutboundState, now: u64) {
    data.version = 1;
    data.terminal.retain(|_, turn| turn.expires_at_ms > now);
    data.unreachable_until_ms.retain(|_, until| *until > now);
    let pending_keys = data
        .pending
        .values()
        .map(|turn| turn.member_key.clone())
        .collect::<std::collections::BTreeSet<_>>();
    data.unreachable_since_ms.retain(|key, _| pending_keys.contains(key));
}

fn load(path: &Path) -> Result<PersistedOutboundState, String> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(PersistedOutboundState { version: 1, ..Default::default() });
        }
        Err(error) => {
            return Err(format!("read xuser outbound turn store {}: {error}", path.display()));
        }
    };
    let mut state: PersistedOutboundState = serde_json::from_str(&raw)
        .map_err(|error| format!("decode xuser outbound turn store: {error}"))?;
    if state.version != 1 {
        return Err(format!("unsupported xuser outbound turn store version {}", state.version));
    }
    state.version = 1;
    Ok(state)
}

fn persist(path: &Path, state: &PersistedOutboundState) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "xuser outbound turn store has no parent".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("create xuser outbound turn directory {}: {error}", parent.display()))?;
    let part = PathBuf::from(format!("{}.part", path.display()));
    let bytes = serde_json::to_vec(state)
        .map_err(|error| format!("encode xuser outbound turn store: {error}"))?;
    let mut file = File::create(&part)
        .map_err(|error| format!("create xuser outbound turn temp {}: {error}", part.display()))?;
    file.write_all(&bytes)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("write xuser outbound turn temp {}: {error}", part.display()))?;
    fs::rename(&part, path).map_err(|error| {
        let _ = fs::remove_file(&part);
        format!("replace xuser outbound turn store {}: {error}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    fn root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fabushi-xuser-outbound-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ))
    }

    struct Sender {
        fail: AtomicBool,
        sent: Mutex<Vec<Value>>,
    }

    impl Sender {
        fn callback(self: &Arc<Self>) -> RelaySend {
            let sender = Arc::clone(self);
            Arc::new(move |payload| {
                sender.sent.lock().unwrap().push(payload.clone());
                if sender.fail.load(Ordering::SeqCst) {
                    Err("offline".into())
                } else {
                    Ok(json!({"ok": true}))
                }
            })
        }
    }

    fn clock(now: &Arc<AtomicU64>) -> Clock {
        let now = Arc::clone(now);
        Arc::new(move || now.load(Ordering::SeqCst))
    }

    fn request(manager: &SandXuserOutboundTurns, generation: u64) -> Option<String> {
        manager
            .request_remote_member_turn(
                generation,
                "room",
                "host",
                "owner",
                "agent",
                "Remote",
                "Room",
                "",
                json!([{"id":"peer","name":"Peer"}]),
                json!([{"speakerKind":"user","speakerName":"You","text":"hello","isSelf":true}]),
                Some("op"),
            )
            .unwrap()
    }

    #[test]
    fn outbound_success_duplicate_and_exactly_once_settlement() {
        let root = root("success");
        let now = Arc::new(AtomicU64::new(100));
        let sender = Arc::new(Sender { fail: AtomicBool::new(false), sent: Mutex::new(Vec::new()) });
        let manager = SandXuserOutboundTurns::new(&root, sender.callback(), clock(&now));
        let nonce = request(&manager, 1).expect("request");
        let event = json!({"turnNonce": nonce, "messages":["one","two","three"]});
        let settled = manager.handle_turn_result(1, &event).unwrap();
        match settled {
            OutboundResultDisposition::Settled(value) => {
                assert_eq!(value.messages, vec!["one", "two"]);
                assert_eq!(value.origin_operation_id.as_deref(), Some("op"));
            }
            other => panic!("unexpected settlement: {other:?}"),
        }
        assert_eq!(manager.handle_turn_result(1, &event).unwrap(), OutboundResultDisposition::Duplicate);
        assert!(manager.pending_nonces().unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn offline_retries_with_backoff_then_terminal_unreachable() {
        let root = root("offline");
        let now = Arc::new(AtomicU64::new(1_000));
        let sender = Arc::new(Sender { fail: AtomicBool::new(true), sent: Mutex::new(Vec::new()) });
        let manager = SandXuserOutboundTurns::new(&root, sender.callback(), clock(&now));
        let nonce = request(&manager, 1).expect("request");
        assert_eq!(sender.sent.lock().unwrap().len(), 1);
        now.store(1_000 + REMOTE_MEMBER_RETRY_BACKOFF_MS - 1, Ordering::SeqCst);
        manager.tick(1).unwrap();
        assert_eq!(sender.sent.lock().unwrap().len(), 1);
        now.store(1_000 + REMOTE_MEMBER_RETRY_BACKOFF_MS, Ordering::SeqCst);
        manager.tick(1).unwrap();
        assert_eq!(sender.sent.lock().unwrap().len(), 2);
        now.store(1_000 + REMOTE_MEMBER_UNREACHABLE_MS, Ordering::SeqCst);
        manager.tick(1).unwrap();
        assert!(matches!(manager.terminal_reason(&nonce).unwrap().as_deref(), Some("unreachable") | Some("timeout")));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn timeout_cancel_and_late_results_are_terminal() {
        let root = root("terminal");
        let now = Arc::new(AtomicU64::new(10));
        let sender = Arc::new(Sender { fail: AtomicBool::new(false), sent: Mutex::new(Vec::new()) });
        let manager = SandXuserOutboundTurns::new(&root, sender.callback(), clock(&now));

        let cancelled = request(&manager, 3).unwrap();
        assert!(manager.cancel(3, &cancelled).unwrap());
        assert_eq!(
            manager.handle_turn_result(3, &json!({"turnNonce":cancelled,"messages":["late"]})).unwrap(),
            OutboundResultDisposition::Late
        );

        let timed_out = request(&manager, 3).unwrap();
        now.store(10 + REMOTE_MEMBER_UNREACHABLE_MS + 1, Ordering::SeqCst);
        manager.tick(3).unwrap();
        assert_eq!(manager.terminal_reason(&timed_out).unwrap().as_deref(), Some("timeout"));
        assert_eq!(
            manager.handle_turn_result(3, &json!({"turnNonce":timed_out,"messages":["late"]})).unwrap(),
            OutboundResultDisposition::Late
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restart_recovers_same_nonce_and_account_switch_fences_old_result() {
        let root = root("restart");
        let now = Arc::new(AtomicU64::new(100));
        let sender = Arc::new(Sender { fail: AtomicBool::new(false), sent: Mutex::new(Vec::new()) });
        let nonce = {
            let manager = SandXuserOutboundTurns::new(&root, sender.callback(), clock(&now));
            request(&manager, 7).unwrap()
        };
        let before = sender.sent.lock().unwrap().len();
        {
            let manager = SandXuserOutboundTurns::new(&root, sender.callback(), clock(&now));
            manager.tick(7).unwrap();
            assert_eq!(sender.sent.lock().unwrap().len(), before + 1);
            manager.reset_account(8).unwrap();
            assert_eq!(
                manager.handle_turn_result(8, &json!({"turnNonce":nonce,"messages":["stale"]})).unwrap(),
                OutboundResultDisposition::Late
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stop_releases_runtime_but_preserves_durable_recovery() {
        let root = root("stop");
        let now = Arc::new(AtomicU64::new(100));
        let sender = Arc::new(Sender { fail: AtomicBool::new(true), sent: Mutex::new(Vec::new()) });
        let nonce = {
            let manager = SandXuserOutboundTurns::new(&root, sender.callback(), clock(&now));
            let nonce = request(&manager, 1).unwrap();
            manager.stop();
            manager.tick(1).unwrap();
            nonce
        };
        sender.fail.store(false, Ordering::SeqCst);
        let manager = SandXuserOutboundTurns::new(&root, sender.callback(), clock(&now));
        manager.tick(1).unwrap();
        assert!(manager.pending_nonces().unwrap().contains(&nonce));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn budget_exhaustion_blocks_thirty_first_turn() {
        let root = root("budget");
        let now = Arc::new(AtomicU64::new(1));
        let sender = Arc::new(Sender { fail: AtomicBool::new(false), sent: Mutex::new(Vec::new()) });
        let manager = SandXuserOutboundTurns::new(&root, sender.callback(), clock(&now));
        for _ in 0..REMOTE_TURN_BUDGET_MAX {
            assert!(request(&manager, 1).is_some());
        }
        assert!(request(&manager, 1).is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remote_turn_budget_is_windowed_and_parser_is_canonical() {
        let budget = RemoteTurnBudget::new();
        for index in 0..REMOTE_TURN_BUDGET_MAX {
            assert!(budget.accept("room\0host\0agent", index as u64));
        }
        assert!(!budget.accept("room\0host\0agent", 20));
        assert!(budget.accept("room\0host\0agent", REMOTE_TURN_BUDGET_WINDOW_MS + 1));
        assert_eq!(
            parse_remote_agent_id("sand-remote:user%2Fone/agent%20two"),
            Some(("user/one".into(), "agent two".into()))
        );
        assert!(parse_remote_agent_id("local-agent").is_none());
    }
}
