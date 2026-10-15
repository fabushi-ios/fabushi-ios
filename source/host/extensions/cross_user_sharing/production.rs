use super::xuser_relay::{SandXuserRelayRuntime, XuserRelayHost, XUSER_RELAY_POLL_INTERVAL_MS};
use super::xuser_state_reconcile::{XuserRoom, reconcile_rooms};
use super::xuser_turn_dedupe_store::{SandXuserTurnDedupeStore, XUSER_TURN_DEDUPE_TTL_MS};
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
    fn run_remote_turn(&self, agent_id: &str, system_prompt: &str, prompt: &str) -> Result<Vec<String>, String>;
}

struct RelayHost(Arc<dyn CrossUserHostBridge>);
impl XuserRelayHost for RelayHost {
    fn poll(&self, ack_ids: &[String]) -> Result<Value, String> { self.0.poll(ack_ids) }
}

#[derive(Default)]
struct SharingSnapshot {
    generation: u64,
    self_auth_id: Option<String>,
    rooms: Vec<XuserRoom>,
}

pub struct MobileCrossUserSharingOwner {
    relay: Arc<SandXuserRelayRuntime>,
    stopped: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl MobileCrossUserSharingOwner {
    pub fn start(root: &Path, host: Arc<dyn CrossUserHostBridge>) -> Self {
        let snapshot = Arc::new(Mutex::new(SharingSnapshot::default()));
        let dedupe = Arc::new(SandXuserTurnDedupeStore::new(
            root,
            XUSER_TURN_DEDUPE_TTL_MS,
            Box::new(system_now_ms),
        ));
        let event_host = Arc::clone(&host);
        let event_snapshot = Arc::clone(&snapshot);
        let event_dedupe = Arc::clone(&dedupe);
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
                if room.host_auth_id != host_auth_id || !is_member || !owns_agent || !event_dedupe.mark_seen_if_new(&nonce) {
                    return Ok(true);
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
                    .run_remote_turn(&agent_id, &system_prompt, &prompt)
                    .unwrap_or_default()
                    .into_iter()
                    .take(2)
                    .collect::<Vec<_>>();
                {
                    let current = event_snapshot.lock().unwrap_or_else(|p| p.into_inner());
                    if current.generation != generation
                        || current.self_auth_id.as_deref() != Some(self_auth_id.as_str())
                    {
                        return Ok(true);
                    }
                }
                let _ = event_host.send(&json!({
                    "kind": "turn-result",
                    "roomId": room_id,
                    "turnNonce": nonce,
                    "agentId": agent_id,
                    "messages": messages,
                }));
                Ok(true)
            }),
            Arc::new(system_now_ms),
        ));
        relay.start();
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_relay = Arc::clone(&relay);
        let worker_host = Arc::clone(&host);
        let worker_snapshot = Arc::clone(&snapshot);
        let worker_stopped = Arc::clone(&stopped);
        let worker = thread::Builder::new().name("fabushi-xuser-relay".into()).spawn(move || {
            while !worker_stopped.load(Ordering::Acquire) {
                if let Ok(value) = worker_host.state() {
                    reconcile_snapshot(&worker_snapshot, &value);
                }
                let _ = worker_relay.tick();
                thread::sleep(Duration::from_millis(XUSER_RELAY_POLL_INTERVAL_MS));
            }
        }).ok();
        Self { relay, stopped, worker: Mutex::new(worker) }
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

fn reconcile_snapshot(snapshot: &Mutex<SharingSnapshot>, value: &Value) {
    let self_auth_id = value.get("selfAuthId").and_then(Value::as_str).map(str::to_string);
    let remote = value.get("rooms").and_then(Value::as_array).into_iter().flatten().filter_map(|room| {
        serde_json::from_value::<XuserRoom>(room.clone()).ok()
    }).collect::<Vec<_>>();
    let mut state = snapshot.lock().unwrap_or_else(|p| p.into_inner());
    if state.self_auth_id != self_auth_id {
        state.generation = state.generation.wrapping_add(1).max(1);
        state.rooms.clear();
    }
    let result = reconcile_rooms(&state.rooms, remote, self_auth_id.as_deref());
    state.self_auth_id = self_auth_id;
    state.rooms = result.rooms;
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
        fn run_remote_turn(&self, _: &str, _: &str, _: &str) -> Result<Vec<String>, String> { self.turns.fetch_add(1, Ordering::SeqCst); Ok(vec!["answer".into()]) }
    }

    #[test]
    fn account_switch_reconcile_drops_stale_room_ownership() {
        let snapshot = Mutex::new(SharingSnapshot::default());
        reconcile_snapshot(&snapshot, &json!({
            "selfAuthId":"account-a",
            "rooms":[{"roomId":"room","name":"Room","hostAuthId":"account-a","members":[]}]
        }));
        assert_eq!(snapshot.lock().unwrap().rooms.len(), 1);
        reconcile_snapshot(&snapshot, &json!({"selfAuthId":"account-b","rooms":[]}));
        let state = snapshot.lock().unwrap();
        assert_eq!(state.self_auth_id.as_deref(), Some("account-b"));
        assert_eq!(state.generation, 2);
        assert!(state.rooms.is_empty());
    }
}
