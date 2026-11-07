use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicU64, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

pub const MAX_TRAYS: usize = 20;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorTray {
    pub kind: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub title: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actions: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dedupe_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PushErrorOptions {
    pub agent_id: Option<String>,
    pub title: String,
    pub detail: String,
    pub request_id: Option<String>,
    pub error_kind: Option<String>,
    pub raw_detail: Option<String>,
    pub actions: Vec<Value>,
    pub dedupe_key: Option<String>,
    pub count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TrayEvent {
    Pushed(ErrorTray),
    Cleared,
    Dismissed(String),
}

type TrayListener = Arc<dyn Fn(TrayEvent) + Send + Sync>;
type CreateId = Arc<dyn Fn() -> String + Send + Sync>;
type Now = Arc<dyn Fn() -> u64 + Send + Sync>;

#[derive(Default)]
struct TrayState {
    trays: Vec<ErrorTray>,
    listeners: BTreeMap<u64, TrayListener>,
}

struct TrayInner {
    state: Mutex<TrayState>,
    create_id: CreateId,
    now: Now,
    next_listener_id: AtomicU64,
}

#[derive(Clone)]
pub struct TrayManager {
    inner: Arc<TrayInner>,
}

pub struct TraySubscription {
    id: u64,
    inner: Weak<TrayInner>,
}

impl Drop for TraySubscription {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .listeners
                .remove(&self.id);
        }
    }
}

impl Default for TrayManager {
    fn default() -> Self {
        Self::new(
            || Uuid::new_v4().to_string(),
            || {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
                    .unwrap_or_default()
            },
        )
    }
}

impl TrayManager {
    pub fn new<Create, Clock>(create_id: Create, now: Clock) -> Self
    where
        Create: Fn() -> String + Send + Sync + 'static,
        Clock: Fn() -> u64 + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(TrayInner {
                state: Mutex::new(TrayState::default()),
                create_id: Arc::new(create_id),
                now: Arc::new(now),
                next_listener_id: AtomicU64::new(1),
            }),
        }
    }

    pub fn get_trays(&self) -> Vec<ErrorTray> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .trays
            .clone()
    }

    pub fn push_error(&self, options: PushErrorOptions) -> ErrorTray {
        let now = (self.inner.now)();
        let mut events = Vec::new();
        let tray = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());

            if let Some(dedupe_key) = options.dedupe_key.as_deref() {
                if let Some(index) = state
                    .trays
                    .iter()
                    .position(|tray| tray.dedupe_key.as_deref() == Some(dedupe_key))
                {
                    let existing = state.trays[index].clone();
                    let updated = ErrorTray {
                        title: options.title,
                        detail: options.detail,
                        request_id: options.request_id,
                        count: Some(options.count.unwrap_or(existing.count.unwrap_or(1).saturating_add(1))),
                        created_at: now,
                        error_kind: options.error_kind,
                        raw_detail: options.raw_detail,
                        actions: (!options.actions.is_empty()).then_some(options.actions),
                        ..existing
                    };
                    state.trays[index] = updated.clone();
                    events.push(TrayEvent::Pushed(updated.clone()));
                    updated
                } else {
                    Self::insert_new(&self.inner, &mut state, options, now, &mut events)
                }
            } else {
                Self::insert_new(&self.inner, &mut state, options, now, &mut events)
            }
        };
        self.emit_all(events);
        tray
    }

    fn insert_new(
        inner: &TrayInner,
        state: &mut TrayState,
        options: PushErrorOptions,
        now: u64,
        events: &mut Vec<TrayEvent>,
    ) -> ErrorTray {
        let count = options
            .dedupe_key
            .as_ref()
            .map(|_| options.count.unwrap_or(1));
        let tray = ErrorTray {
            kind: "error".into(),
            id: (inner.create_id)(),
            agent_id: options.agent_id,
            title: options.title,
            detail: options.detail,
            request_id: options.request_id,
            created_at: now,
            error_kind: options.error_kind,
            raw_detail: options.raw_detail,
            actions: (!options.actions.is_empty()).then_some(options.actions),
            dedupe_key: options.dedupe_key,
            count,
        };
        state.trays.push(tray.clone());
        events.push(TrayEvent::Pushed(tray.clone()));
        if state.trays.len() > MAX_TRAYS {
            let excess = state.trays.len() - MAX_TRAYS;
            let dropped = state.trays.drain(0..excess).collect::<Vec<_>>();
            events.extend(
                dropped
                    .into_iter()
                    .map(|tray| TrayEvent::Dismissed(tray.id)),
            );
        }
        tray
    }

    pub fn clear_all(&self) {
        let should_emit = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.trays.is_empty() {
                false
            } else {
                state.trays.clear();
                true
            }
        };
        if should_emit {
            self.emit_all(vec![TrayEvent::Cleared]);
        }
    }

    pub fn dismiss(&self, id: &str) -> bool {
        let removed = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let before = state.trays.len();
            state.trays.retain(|tray| tray.id != id);
            state.trays.len() != before
        };
        if removed {
            self.emit_all(vec![TrayEvent::Dismissed(id.to_string())]);
        }
        removed
    }

    pub fn clear_for_agent(&self, agent_id: &str) {
        let removed = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut removed = Vec::new();
            state.trays.retain(|tray| {
                if tray.agent_id.as_deref() == Some(agent_id) {
                    removed.push(tray.id.clone());
                    false
                } else {
                    true
                }
            });
            removed
        };
        self.emit_all(
            removed
                .into_iter()
                .map(TrayEvent::Dismissed)
                .collect(),
        );
    }

    pub fn subscribe(&self, listener: TrayListener) -> TraySubscription {
        let id = self.inner.next_listener_id.fetch_add(1, Ordering::Relaxed);
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .listeners
            .insert(id, listener);
        TraySubscription {
            id,
            inner: Arc::downgrade(&self.inner),
        }
    }

    fn emit_all(&self, events: Vec<TrayEvent>) {
        if events.is_empty() {
            return;
        }
        let listeners = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .listeners
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for event in events {
            for listener in &listeners {
                listener(event.clone());
            }
        }
    }
}
