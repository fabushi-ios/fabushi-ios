use super::xuser_departure_obligations::{
    SandXuserDepartureObligations, XuserDepartureRelay,
};
use super::xuser_entry_publisher::SandXuserEntryPublisher;
use super::xuser_pending_departure_store::{
    SandXuserPendingDepartureStore, XuserDeparture,
};
use super::xuser_relay::{SandXuserRelayRuntime, XuserRelayHost, XUSER_RELAY_POLL_INTERVAL_MS};
use super::xuser_remote_turns::{
    OutboundResultDisposition, RemoteTurnBudget, SandXuserOutboundTurns,
};
use super::xuser_room_tombstone_store::SandXuserRoomTombstoneStore;
use super::xuser_state_reconcile::{XuserRoom, reconcile_rooms};
use super::xuser_turn_dedupe_store::{SandXuserTurnDedupeStore, XUSER_TURN_DEDUPE_TTL_MS};
use super::xuser_turn_result_store::{
    SandXuserTurnResultStore, TurnAdmission, XUSER_TURN_RESULT_TTL_MS,
};
use super::xuser_wire_normalization::{build_shared_room_guardrail_prompt, normalize_turn_messages};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub trait CrossUserHostBridge: Send + Sync {
    fn state(&self) -> Result<Value, String>;
    fn poll(&self, ack_ids: &[String]) -> Result<Value, String>;
    fn send(&self, payload: &Value) -> Result<Value, String>;
    fn leave_room(&self, room_id: &str) -> Result<Value, String>;
    fn remove_deleted_agent(&self, agent_id: &str) -> Result<Value, String>;
    fn run_remote_turn(
        &self,
        agent_id: &str,
        turn_nonce: &str,
        system_prompt: &str,
        prompt: &str,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Vec<String>, String>;
}

struct RelayHost(Arc<dyn CrossUserHostBridge>);
impl XuserRelayHost for RelayHost {
    fn poll(&self, ack_ids: &[String]) -> Result<Value, String> { self.0.poll(ack_ids) }
}

#[derive(Default, Clone)]
struct SharingSnapshot {
    generation: u64,
    is_enabled: bool,
    self_auth_id: Option<String>,
    rooms: Vec<XuserRoom>,
}

#[derive(Clone)]
struct LocalSharedOperation {
    generation: u64,
    room_id: String,
    self_auth_id: String,
    agent_id: String,
    agent_name: String,
}

struct DepartureRelay {
    host: Arc<dyn CrossUserHostBridge>,
}

impl XuserDepartureRelay for DepartureRelay {
    fn leave_room(&self, room_id: &str) -> Result<(), String> {
        self.host.leave_room(room_id).map(|_| ())
    }

    fn remove_deleted_agent(&self, agent_id: &str) -> Result<(), String> {
        self.host.remove_deleted_agent(agent_id).map(|_| ())
    }
}

pub struct MobileCrossUserSharingOwner {
    relay: Arc<SandXuserRelayRuntime>,
    stopped: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
    snapshot: Arc<Mutex<SharingSnapshot>>,
    departures: SandXuserDepartureObligations,
    outbound_turns: Arc<SandXuserOutboundTurns>,
    entry_publisher: Arc<SandXuserEntryPublisher>,
    local_operations: Arc<Mutex<std::collections::BTreeMap<String, LocalSharedOperation>>>,
}

impl MobileCrossUserSharingOwner {
    pub fn start(root: &Path, host: Arc<dyn CrossUserHostBridge>) -> Self {
        Self::start_with_notify(
            root,
            host,
            Arc::new(|| false),
            Arc::new(|| false),
        )
    }

    pub fn start_with_notify(
        root: &Path,
        host: Arc<dyn CrossUserHostBridge>,
        is_notify_connected: Arc<dyn Fn() -> bool + Send + Sync>,
        is_notify_safety_poll_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        let snapshot = Arc::new(Mutex::new(SharingSnapshot::default()));
        let departures = SandXuserDepartureObligations::new(
            Arc::new(SandXuserPendingDepartureStore::new(root)),
            Arc::new(SandXuserRoomTombstoneStore::new(root)),
        );
        let dedupe = Arc::new(SandXuserTurnDedupeStore::new(
            root,
            XUSER_TURN_DEDUPE_TTL_MS,
            Box::new(system_now_ms),
        ));
        let turn_results = Arc::new(SandXuserTurnResultStore::new(
            root,
            XUSER_TURN_RESULT_TTL_MS,
            Box::new(system_now_ms),
        ));
        let remote_turn_budget = Arc::new(RemoteTurnBudget::new());
        let outbound_host = Arc::clone(&host);
        let outbound_turns = Arc::new(SandXuserOutboundTurns::new(
            root,
            Arc::new(move |payload| outbound_host.send(payload)),
            Arc::new(system_now_ms),
        ));
        let publisher_host = Arc::clone(&host);
        let entry_publisher = Arc::new(SandXuserEntryPublisher::new(Arc::new(move |payload| {
            publisher_host.send(payload)
        })));
        let local_operations = Arc::new(Mutex::new(std::collections::BTreeMap::<String, LocalSharedOperation>::new()));
        let event_host = Arc::clone(&host);
        let event_snapshot = Arc::clone(&snapshot);
        let event_dedupe = Arc::clone(&dedupe);
        let event_turn_results = Arc::clone(&turn_results);
        let event_budget = Arc::clone(&remote_turn_budget);
        let event_outbound_turns = Arc::clone(&outbound_turns);
        let event_publisher = Arc::clone(&entry_publisher);
        let stopped = Arc::new(AtomicBool::new(false));
        let event_stopped = Arc::clone(&stopped);
        let relay = Arc::new(SandXuserRelayRuntime::new(
            Arc::new(RelayHost(Arc::clone(&host))),
            Arc::new(move |event| {
                let kind = event.get("kind").and_then(Value::as_str).unwrap_or("");
                if kind == "turn-result" {
                    let generation = event_snapshot
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .generation;
                    match event_outbound_turns.handle_turn_result(generation, event)? {
                        OutboundResultDisposition::Settled(settlement) => {
                            for (index, message) in settlement.messages.iter().enumerate() {
                                event_publisher.publish_agent_message(
                                    &settlement.room_id,
                                    &format!("remote-turn:{}:{index}", settlement.nonce),
                                    &settlement.owner_auth_id,
                                    &settlement.agent_id,
                                    &settlement.member_name,
                                    message,
                                )?;
                            }
                            return Ok(true);
                        }
                        OutboundResultDisposition::Duplicate | OutboundResultDisposition::Late => {
                            return Ok(true);
                        }
                    }
                }
                if kind != "turn-request" {
                    return Ok(true);
                }
                let room_id = required(event, "roomId")?;
                let nonce = required(event, "turnNonce")?;
                let agent_id = required(event, "agentId")?;
                let host_auth_id = required(event, "hostAuthId")?;
                let (generation, self_auth_id, room) = {
                    let state = event_snapshot.lock().unwrap_or_else(|p| p.into_inner());
                    let Some(self_auth_id) = state.self_auth_id.clone() else { return Ok(true) };
                    let room = state.rooms.iter().find(|room| room.room_id == room_id).cloned();
                    (state.generation, self_auth_id, room)
                };
                let Some(room) = room else { return Ok(true) };
                let owns_agent = room.members.iter().any(|member| {
                    member.auth_id == self_auth_id && member.agent_id.as_deref() == Some(agent_id.as_str())
                });
                let is_member = room.host_auth_id == self_auth_id || room.members.iter().any(|member| member.auth_id == self_auth_id);
                if room.host_auth_id != host_auth_id || !is_member || !owns_agent {
                    return Ok(true);
                }
                let budget_key = format!("{room_id}\0{host_auth_id}\0{agent_id}");
                if !event_budget.accept(&budget_key, system_now_ms()) {
                    return Ok(true);
                }
                let durable_nonce = format!("{self_auth_id}\0{nonce}");
                match event_turn_results.admit(&durable_nonce)? {
                    TurnAdmission::Completed(messages) => {
                        event_host.send(&json!({
                            "kind": "turn-result",
                            "roomId": room_id,
                            "turnNonce": nonce,
                            "agentId": agent_id,
                            "messages": messages,
                        }))?;
                        return Ok(true);
                    }
                    TurnAdmission::Reserved => {
                        return Ok(false);
                    }
                    TurnAdmission::New => {}
                }
                if !event_dedupe.mark_seen_if_new(&durable_nonce)? {
                    return Ok(false);
                }
                let messages = normalize_turn_messages(event.get("newMessages").unwrap_or(&Value::Null));
                let context = messages.iter().map(|message| {
                    format!("{} {}: {}", if message.is_self {"self"} else {&message.speaker_kind}, message.speaker_name, message.text)
                }).collect::<Vec<_>>().join("\n");
                let host_name = event
                    .get("hostName")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("the room host");
                let system_prompt = format!(
                    "You are responding as a member of the shared room '{}'. Reply only with the member response.{}",
                    room.name,
                    build_shared_room_guardrail_prompt(host_name, true)
                );
                let prompt = if context.is_empty() {
                    "Respond to the shared room.".to_string()
                } else {
                    format!("Shared-room context:\n{context}\n\nRespond now.")
                };
                let messages = event_host
                    .run_remote_turn(
                        &agent_id,
                        &nonce,
                        &system_prompt,
                        &prompt,
                        Arc::clone(&event_stopped),
                    )
                    .unwrap_or_default()
                    .into_iter()
                    .take(2)
                    .collect::<Vec<_>>();
                {
                    let current = event_snapshot.lock().unwrap_or_else(|p| p.into_inner());
                    if current.generation != generation
                        || current.self_auth_id.as_deref() != Some(self_auth_id.as_str())
                    {
                        return Ok(false);
                    }
                }
                event_turn_results.complete(&durable_nonce, messages.clone())?;
                event_host.send(&json!({
                    "kind": "turn-result",
                    "roomId": room_id,
                    "turnNonce": nonce,
                    "agentId": agent_id,
                    "messages": messages,
                }))?;
                Ok(true)
            }),
            Arc::new(system_now_ms),
            is_notify_connected,
            is_notify_safety_poll_enabled,
        ));
        relay.start();
        let worker_relay = Arc::clone(&relay);
        let worker_host = Arc::clone(&host);
        let worker_snapshot = Arc::clone(&snapshot);
        let worker_departures = departures.clone();
        let worker_departure_relay = DepartureRelay { host: Arc::clone(&host) };
        let worker_outbound_turns = Arc::clone(&outbound_turns);
        let worker_local_operations = Arc::clone(&local_operations);
        let worker_stopped = Arc::clone(&stopped);
        let worker = thread::Builder::new().name("fabushi-xuser-relay".into()).spawn(move || {
            while !worker_stopped.load(Ordering::Acquire) {
                if let Ok(value) = worker_host.state() {
                    if let Ok(account_changed) =
                        reconcile_snapshot(&worker_snapshot, &worker_departures, &value)
                    {
                        if account_changed {
                            worker_relay.reset_account();
                            remote_turn_budget.clear();
                            let generation = worker_snapshot
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .generation;
                            let _ = worker_outbound_turns.reset_account(generation);
                            worker_local_operations
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .retain(|_, operation| operation.generation == generation);
                        }
                        let state = worker_snapshot.lock().unwrap_or_else(|p| p.into_inner());
                        if state.is_enabled {
                            let generation = state.generation;
                            let self_auth_id = state.self_auth_id.clone();
                            let registry = state.rooms.iter().map(|room| room.room_id.clone())
                                .collect::<std::collections::BTreeSet<_>>();
                            drop(state);
                            let _ = worker_outbound_turns.tick(generation);
                            let _ = worker_departures.drain(&worker_departure_relay);
                            let _ = worker_departures.prune_tombstones_for_registry(
                                system_now_ms(),
                                &registry,
                                self_auth_id.as_deref(),
                            );
                            let _ = worker_relay.tick();
                        }
                    }
                }
                thread::sleep(Duration::from_millis(XUSER_RELAY_POLL_INTERVAL_MS));
            }
        }).ok();
        Self {
            relay,
            stopped,
            worker: Mutex::new(worker),
            snapshot,
            departures,
            outbound_turns,
            entry_publisher,
            local_operations,
        }
    }

    pub fn note_leave_room(
        &self,
        room_id: &str,
        target_auth_id: Option<&str>,
    ) -> Result<Option<XuserDeparture>, String> {
        let room_id = room_id.trim();
        if room_id.is_empty() {
            return Ok(None);
        }
        let self_auth_id = self.snapshot.lock().unwrap_or_else(|p| p.into_inner()).self_auth_id.clone();
        if target_auth_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some_and(|target| Some(target) != self_auth_id.as_deref())
        {
            return Ok(None);
        }
        self.departures
            .record_leave(self_auth_id, room_id.to_string(), system_now_ms())
            .map(Some)
    }

    pub fn settle_departure(&self, departure: &XuserDeparture) -> Result<(), String> {
        self.departures.pending.clear(departure)
    }

    pub fn note_agent_deleted(&self, agent_id: &str) -> Result<(), String> {
        let agent_id = agent_id.trim();
        if agent_id.is_empty() {
            return Ok(());
        }
        let self_auth_id = self.snapshot.lock().unwrap_or_else(|p| p.into_inner()).self_auth_id.clone();
        self.departures
            .record_agent_removal(self_auth_id, agent_id.to_string())
            .map(|_| ())
    }

    pub fn observe_host_dispatch(&self, input: &str, output: &str) -> Result<(), String> {
        let request: Value = match serde_json::from_str(input) {
            Ok(value) => value,
            Err(_) => return Ok(()),
        };
        let response: Value = match serde_json::from_str(output) {
            Ok(value) => value,
            Err(_) => return Ok(()),
        };
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(());
        }
        match request.get("method").and_then(Value::as_str) {
            Some("feature.execute") => self.observe_feature_execute(&request, &response),
            Some("feature.receive") => self.observe_feature_receive(&response),
            _ => Ok(()),
        }
    }

    fn observe_feature_execute(&self, request: &Value, response: &Value) -> Result<(), String> {
        let Some(command) = request
            .get("params")
            .and_then(|value| value.get("command"))
        else {
            return Ok(());
        };
        if command.get("type").and_then(Value::as_str) != Some("chat.send") {
            return Ok(());
        }
        let Some(agent_id) = command
            .get("agentId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(());
        };
        let text = command.get("text").and_then(Value::as_str).unwrap_or("").trim();
        if text.is_empty() {
            return Ok(());
        }
        let request_id = command
            .get("requestId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("shared-room-message");
        let operation_id = response
            .get("result")
            .and_then(|value| value.get("operationId"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let (generation, self_auth_id, room) = {
            let snapshot = self.snapshot.lock().unwrap_or_else(|p| p.into_inner());
            if !snapshot.is_enabled {
                return Ok(());
            }
            let Some(self_auth_id) = snapshot.self_auth_id.clone() else {
                return Ok(());
            };
            let room = snapshot.rooms.iter().find(|room| {
                room.members.iter().any(|member| {
                    member.auth_id == self_auth_id
                        && member.agent_id.as_deref() == Some(agent_id)
                })
            }).cloned();
            (snapshot.generation, self_auth_id, room)
        };
        let Some(room) = room else {
            return Ok(());
        };
        let human_name = room
            .members
            .iter()
            .find(|member| member.auth_id == self_auth_id && member.agent_id.is_none())
            .and_then(|member| member.name.clone())
            .unwrap_or_else(|| "Host".to_string());
        let local_agent_name = room
            .members
            .iter()
            .find(|member| {
                member.auth_id == self_auth_id
                    && member.agent_id.as_deref() == Some(agent_id)
            })
            .and_then(|member| member.name.clone())
            .unwrap_or_else(|| agent_id.to_string());

        self.entry_publisher.publish_human_message(
            &room.room_id,
            request_id,
            &self_auth_id,
            &human_name,
            text,
        )?;

        if let Some(operation_id) = operation_id.as_deref() {
            self.local_operations
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(
                    operation_id.to_string(),
                    LocalSharedOperation {
                        generation,
                        room_id: room.room_id.clone(),
                        self_auth_id: self_auth_id.clone(),
                        agent_id: agent_id.to_string(),
                        agent_name: local_agent_name,
                    },
                );
        }

        let peers = room
            .members
            .iter()
            .map(|member| {
                json!({
                    "id": member.agent_id.clone().unwrap_or_else(|| member.auth_id.clone()),
                    "name": member.name.clone().unwrap_or_else(|| {
                        member.agent_id.clone().unwrap_or_else(|| member.auth_id.clone())
                    }),
                    "description": "",
                })
            })
            .collect::<Vec<_>>();
        let new_messages = json!([{
            "speakerKind": "human",
            "speakerName": human_name,
            "text": text,
            "isSelf": true,
        }]);
        for member in room.members.iter().filter(|member| {
            member.auth_id != self_auth_id && member.agent_id.is_some()
        }) {
            let Some(remote_agent_id) = member.agent_id.as_deref() else {
                continue;
            };
            let member_name = member.name.as_deref().unwrap_or(remote_agent_id);
            let _ = self.outbound_turns.request_remote_member_turn(
                generation,
                &room.room_id,
                &room.host_auth_id,
                &member.auth_id,
                remote_agent_id,
                member_name,
                &room.name,
                "",
                Value::Array(peers.clone()),
                new_messages.clone(),
                operation_id.as_deref(),
            )?;
        }
        Ok(())
    }

    fn observe_feature_receive(&self, response: &Value) -> Result<(), String> {
        let Some(event) = response.get("result") else {
            return Ok(());
        };
        let Some(operation_id) = event
            .get("operationId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(());
        };
        let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
        if event_type == "chat.message" && event.get("role").and_then(Value::as_str) == Some("assistant") {
            let operation = self
                .local_operations
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(operation_id)
                .cloned();
            if let Some(operation) = operation {
                let generation_matches = {
                    let snapshot = self.snapshot.lock().unwrap_or_else(|p| p.into_inner());
                    snapshot.generation == operation.generation
                        && snapshot.self_auth_id.as_deref() == Some(operation.self_auth_id.as_str())
                        && snapshot.rooms.iter().any(|room| room.room_id == operation.room_id)
                };
                if generation_matches {
                    if let Some(text) = event
                        .get("text")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        let entry_id = event
                            .get("messageId")
                            .and_then(Value::as_str)
                            .unwrap_or(operation_id);
                        self.entry_publisher.publish_agent_message(
                            &operation.room_id,
                            entry_id,
                            &operation.self_auth_id,
                            &operation.agent_id,
                            &operation.agent_name,
                            text,
                        )?;
                    }
                }
            }
        }
        if matches!(
            event_type,
            "operation.completed" | "operation.failed" | "operation.interrupted"
        ) {
            self.local_operations
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(operation_id);
        }
        Ok(())
    }

    pub fn relay_notify_handler(&self) -> Arc<dyn Fn() + Send + Sync> {
        let relay = Arc::clone(&self.relay);
        Arc::new(move || {
            let _ = relay.request_drain();
        })
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.outbound_turns.stop();
        self.local_operations
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.relay.stop();
        if let Some(worker) = self.worker.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = worker.join();
        }
    }
}

impl Drop for MobileCrossUserSharingOwner {
    fn drop(&mut self) { self.stop(); }
}

fn reconcile_snapshot(
    snapshot: &Mutex<SharingSnapshot>,
    departures: &SandXuserDepartureObligations,
    value: &Value,
) -> Result<bool, String> {
    let self_auth_id = value.get("selfAuthId").and_then(Value::as_str).map(str::to_string);
    let is_enabled = value.get("isEnabled").and_then(Value::as_bool).unwrap_or(false);
    let remote = value.get("rooms").and_then(Value::as_array).into_iter().flatten().filter_map(|room| {
        serde_json::from_value::<XuserRoom>(room.clone()).ok()
    }).collect::<Vec<_>>();
    let mut state = snapshot.lock().unwrap_or_else(|p| p.into_inner());
    let account_changed = state.self_auth_id != self_auth_id;
    if account_changed {
        state.generation = state.generation.wrapping_add(1).max(1);
        state.rooms.clear();
    }
    let result = reconcile_rooms(&state.rooms, remote, self_auth_id.as_deref());
    let mut rooms = Vec::new();
    for room in result.rooms {
        if !departures.is_room_abandoned(&room.room_id, self_auth_id.as_deref())? {
            rooms.push(room);
        }
    }
    state.is_enabled = is_enabled;
    state.self_auth_id = self_auth_id;
    state.rooms = rooms;
    Ok(account_changed)
}

fn required(value: &Value, key: &str) -> Result<String, String> {
    value.get(key).and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty())
        .map(str::to_string).ok_or_else(|| format!("{key} is required"))
}

fn system_now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Host {
        state: Mutex<Value>,
        polls: AtomicUsize,
        sends: Mutex<Vec<Value>>,
        turns: AtomicUsize,
    }
    impl CrossUserHostBridge for Host {
        fn state(&self) -> Result<Value, String> { Ok(self.state.lock().unwrap().clone()) }
        fn poll(&self, _: &[String]) -> Result<Value, String> { self.polls.fetch_add(1, Ordering::SeqCst); Ok(json!({"events": []})) }
        fn send(&self, value: &Value) -> Result<Value, String> { self.sends.lock().unwrap().push(value.clone()); Ok(json!({})) }
        fn leave_room(&self, _: &str) -> Result<Value, String> { Ok(json!({"ok":true})) }
        fn remove_deleted_agent(&self, _: &str) -> Result<Value, String> { Ok(json!({"ok":true})) }
        fn run_remote_turn(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
            cancelled: Arc<AtomicBool>,
        ) -> Result<Vec<String>, String> {
            if cancelled.load(Ordering::Acquire) {
                return Err("cancelled".into());
            }
            self.turns.fetch_add(1, Ordering::SeqCst);
            Ok(vec!["answer".into()])
        }
    }

    #[test]
    fn account_switch_reconcile_drops_stale_room_ownership() {
        let snapshot = Mutex::new(SharingSnapshot::default());
        let root = std::env::temp_dir().join(format!(
            "fabushi-xuser-snapshot-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let departures = SandXuserDepartureObligations::new(
            Arc::new(SandXuserPendingDepartureStore::new(&root)),
            Arc::new(SandXuserRoomTombstoneStore::new(&root)),
        );
        assert!(reconcile_snapshot(&snapshot, &departures, &json!({
            "isEnabled":true,
            "selfAuthId":"account-a",
            "rooms":[{"roomId":"room","name":"Room","hostAuthId":"account-a","members":[]}]
        })).unwrap());
        assert_eq!(snapshot.lock().unwrap().rooms.len(), 1);
        assert!(reconcile_snapshot(
            &snapshot,
            &departures,
            &json!({"isEnabled":true,"selfAuthId":"account-b","rooms":[]}),
        ).unwrap());
        let state = snapshot.lock().unwrap();
        assert_eq!(state.self_auth_id.as_deref(), Some("account-b"));
        assert_eq!(state.generation, 2);
        assert!(state.rooms.is_empty());
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }    #[test]
    fn shipping_dispatch_publishes_room_entry_and_requests_remote_member_turn() {
        let root = std::env::temp_dir().join(format!(
            "fabushi-xuser-shipping-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let host = Arc::new(Host {
            state: Mutex::new(json!({
                "isEnabled":true,
                "selfAuthId":"local-auth",
                "rooms":[{
                    "roomId":"room-1",
                    "name":"Shared room",
                    "hostAuthId":"local-auth",
                    "members":[
                        {"authId":"local-auth","agentId":"local-agent","name":"Local agent"},
                        {"authId":"local-auth","name":"Local human"},
                        {"authId":"remote-auth","agentId":"remote-agent","name":"Remote agent"}
                    ]
                }]
            })),
            polls: AtomicUsize::new(0),
            sends: Mutex::new(Vec::new()),
            turns: AtomicUsize::new(0),
        });
        let owner = MobileCrossUserSharingOwner::start(&root, host.clone());
        for _ in 0..100 {
            let ready = {
                let snapshot = owner.snapshot.lock().unwrap();
                snapshot.is_enabled
                    && snapshot.self_auth_id.as_deref() == Some("local-auth")
                    && snapshot.rooms.iter().any(|room| room.room_id == "room-1")
            };
            if ready {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        owner.observe_host_dispatch(
            &json!({
                "method":"feature.execute",
                "params":{"command":{
                    "type":"chat.send",
                    "requestId":"message-1",
                    "agentId":"local-agent",
                    "text":"hello room"
                }}
            }).to_string(),
            &json!({"ok":true,"result":{"operationId":"operation-1"}}).to_string(),
        ).unwrap();
        let sends = host.sends.lock().unwrap().clone();
        assert!(sends.iter().any(|value| {
            value.get("kind").and_then(Value::as_str) == Some("room-entry")
                && value.get("entry").and_then(|entry| entry.get("kind")).and_then(Value::as_str)
                    == Some("human-message")
        }));
        assert!(sends.iter().any(|value| {
            value.get("kind").and_then(Value::as_str) == Some("turn-request")
                && value.get("ownerAuthId").and_then(Value::as_str) == Some("remote-auth")
                && value.get("agentId").and_then(Value::as_str) == Some("remote-agent")
        }));
        drop(sends);

        owner.observe_host_dispatch(
            &json!({"method":"feature.receive","params":{}}).to_string(),
            &json!({"ok":true,"result":{
                "type":"chat.message",
                "operationId":"operation-1",
                "role":"assistant",
                "messageId":"assistant-1",
                "text":"local reply"
            }}).to_string(),
        ).unwrap();
        assert!(host.sends.lock().unwrap().iter().any(|value| {
            value.get("kind").and_then(Value::as_str) == Some("room-entry")
                && value.get("entry").and_then(|entry| entry.get("kind")).and_then(Value::as_str)
                    == Some("agent-message")
                && value.get("entry").and_then(|entry| entry.get("text")).and_then(Value::as_str)
                    == Some("local reply")
        }));
        owner.stop();
        let _ = std::fs::remove_dir_all(root);
    }


}
