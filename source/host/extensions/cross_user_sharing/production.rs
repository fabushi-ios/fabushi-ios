use super::xuser_departure_obligations::{
    SandXuserDepartureObligations, XuserDepartureRelay,
};
use super::xuser_pending_departure_store::{
    SandXuserPendingDepartureStore, XuserDeparture,
};
use super::xuser_relay::{SandXuserRelayRuntime, XuserRelayHost, XUSER_RELAY_POLL_INTERVAL_MS};
use super::xuser_room_tombstone_store::SandXuserRoomTombstoneStore;
use super::xuser_state_reconcile::{XuserRoom, reconcile_rooms};
use super::xuser_turn_dedupe_store::{SandXuserTurnDedupeStore, XUSER_TURN_DEDUPE_TTL_MS};
use super::xuser_turn_result_store::{
    SandXuserTurnResultStore, TurnAdmission, XUSER_TURN_RESULT_TTL_MS,
};
use super::xuser_wire_normalization::normalize_turn_messages;
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

#[derive(Default)]
struct SharingSnapshot {
    generation: u64,
    is_enabled: bool,
    self_auth_id: Option<String>,
    rooms: Vec<XuserRoom>,
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
}

impl MobileCrossUserSharingOwner {
    pub fn start(root: &Path, host: Arc<dyn CrossUserHostBridge>) -> Self {
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
        let event_host = Arc::clone(&host);
        let event_snapshot = Arc::clone(&snapshot);
        let event_dedupe = Arc::clone(&dedupe);
        let event_turn_results = Arc::clone(&turn_results);
        let stopped = Arc::new(AtomicBool::new(false));
        let event_stopped = Arc::clone(&stopped);
        let relay = Arc::new(SandXuserRelayRuntime::new(
            Arc::new(RelayHost(Arc::clone(&host))),
            Arc::new(move |event| {
                let kind = event.get("kind").and_then(Value::as_str).unwrap_or("");
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
                let system_prompt = format!(
                    "You are responding as a member of the shared room '{}'. Reply only with the member response.",
                    room.name
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
        ));
        relay.start();
        let worker_relay = Arc::clone(&relay);
        let worker_host = Arc::clone(&host);
        let worker_snapshot = Arc::clone(&snapshot);
        let worker_departures = departures.clone();
        let worker_departure_relay = DepartureRelay { host: Arc::clone(&host) };
        let worker_stopped = Arc::clone(&stopped);
        let worker = thread::Builder::new().name("fabushi-xuser-relay".into()).spawn(move || {
            while !worker_stopped.load(Ordering::Acquire) {
                if let Ok(value) = worker_host.state() {
                    if let Ok(account_changed) =
                        reconcile_snapshot(&worker_snapshot, &worker_departures, &value)
                    {
                        if account_changed {
                            worker_relay.reset_account();
                        }
                        let state = worker_snapshot.lock().unwrap_or_else(|p| p.into_inner());
                        if state.is_enabled {
                            let self_auth_id = state.self_auth_id.clone();
                            let registry = state.rooms.iter().map(|room| room.room_id.clone())
                                .collect::<std::collections::BTreeSet<_>>();
                            drop(state);
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

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
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
    }
}
