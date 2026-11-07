use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::extensions::auth::extension::HostAuthExtension;
use crate::extensions::extension_ids_generated::HostExtensionId;
use crate::host_event_bus::{HostEventSubscription, SandHostEventBus};

use super::mobile_push_notifier::{
    CursorMobilePushOptions, MobilePushInput, MobilePushNotify, NotificationAgent,
    SandMobilePushNotifier, create_cursor_mobile_push_sender,
};

pub const NOTIFICATIONS_EXTENSION_ID: HostExtensionId = HostExtensionId::Notifications;
pub const NOTIFICATIONS_DEPENDENCIES: &[HostExtensionId] = &[HostExtensionId::Auth];

pub type WindowFocusedAtSource = Arc<dyn Fn() -> Option<u64> + Send + Sync>;
pub type NotificationClock = Arc<dyn Fn() -> u64 + Send + Sync>;

struct NotificationExtensionState {
    notifier: SandMobilePushNotifier,
    known_agent_ids: BTreeSet<String>,
    seeded: bool,
}

pub struct HostNotificationsExtension {
    _subscription: HostEventSubscription,
    _state: Arc<Mutex<NotificationExtensionState>>,
}

pub fn notification_agent_from_value(value: &Value) -> Option<NotificationAgent> {
    let id = value.get("id").and_then(Value::as_str)?.trim().to_string();
    if id.is_empty() {
        return None;
    }
    let awaiting_reason = value
        .get("awaitingUserResponse")
        .and_then(Value::as_object)
        .and_then(|object| object.get("reason"))
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(NotificationAgent {
        id,
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        is_running: value
            .get("isRunning")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        awaiting_reason,
        notify_enabled: value
            .get("notifyOnUpdatesEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        is_hidden_from_sidebar: value
            .get("isHiddenFromSidebar")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        last_message_id: value
            .get("lastMessageId")
            .and_then(Value::as_str)
            .map(str::to_string),
        last_message_preview: value
            .get("lastMessagePreview")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn agents_from_value(value: &Value) -> Vec<NotificationAgent> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(notification_agent_from_value)
        .collect()
}

fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn process_full_roster(
    state: &mut NotificationExtensionState,
    agents: Vec<NotificationAgent>,
    focused_at_ms: Option<u64>,
    now_ms: u64,
) {
    let current_ids = agents
        .iter()
        .map(|agent| agent.id.clone())
        .collect::<BTreeSet<_>>();

    if state.seeded {
        state
            .notifier
            .handle_agents_event(&agents, focused_at_ms, now_ms);
        let removed = state
            .known_agent_ids
            .difference(&current_ids)
            .cloned()
            .collect::<Vec<_>>();
        for agent_id in removed {
            state.notifier.forget(&agent_id);
        }
        state.known_agent_ids = current_ids;
    } else {
        state.notifier.seed_baseline(&agents, now_ms);
        state.seeded = true;
        state.known_agent_ids.extend(current_ids);
    }
}

fn process_upsert(
    state: &mut NotificationExtensionState,
    agent: NotificationAgent,
    focused_at_ms: Option<u64>,
    now_ms: u64,
) {
    state.known_agent_ids.insert(agent.id.clone());
    state
        .notifier
        .handle_agent_upserted(agent, focused_at_ms, now_ms);
}

fn process_forget(state: &mut NotificationExtensionState, agent_id: &str) {
    state.known_agent_ids.remove(agent_id);
    state.notifier.forget(agent_id);
}

pub fn start_notifications_extension_with_sender(
    events: SandHostEventBus,
    initial_baseline: Option<Vec<NotificationAgent>>,
    window_focused_at: WindowFocusedAtSource,
    now: NotificationClock,
    notify: MobilePushNotify,
) -> HostNotificationsExtension {
    let baseline_seeded = initial_baseline.is_some();
    let baseline = initial_baseline.unwrap_or_default();
    let mut notifier = SandMobilePushNotifier::new(notify);
    let baseline_now = now();
    if baseline_seeded {
        notifier.seed_baseline(&baseline, baseline_now);
    }
    let state = Arc::new(Mutex::new(NotificationExtensionState {
        notifier,
        known_agent_ids: baseline
            .iter()
            .map(|agent| agent.id.clone())
            .collect(),
        seeded: baseline_seeded,
    }));

    let listener_state = Arc::clone(&state);
    let listener_focus = Arc::clone(&window_focused_at);
    let listener_now = Arc::clone(&now);
    let subscription = events.subscribe_listener(move |event| {
        let focused_at_ms = listener_focus();
        let now_ms = listener_now();
        let Ok(mut state) = listener_state.lock() else {
            return;
        };

        if let Some(kind) = event.get("kind").and_then(Value::as_str) {
            match kind {
                "notification-baseline" => {
                    let agents = agents_from_value(
                        event.get("agents").unwrap_or(&Value::Null),
                    );
                    process_full_roster(&mut state, agents, focused_at_ms, now_ms);
                    return;
                }
                "notification-agents" => {
                    let agents = agents_from_value(
                        event
                            .get("event")
                            .and_then(|value| value.get("agents"))
                            .unwrap_or(&Value::Null),
                    );
                    process_full_roster(&mut state, agents, focused_at_ms, now_ms);
                    return;
                }
                "notification-agent-upserted" => {
                    if let Some(agent) = event
                        .get("event")
                        .and_then(|value| value.get("agent"))
                        .and_then(notification_agent_from_value)
                    {
                        process_upsert(&mut state, agent, focused_at_ms, now_ms);
                    }
                    return;
                }
                "notification-agent-forgotten" => {
                    if let Some(agent_id) = event.get("agentId").and_then(Value::as_str) {
                        process_forget(&mut state, agent_id);
                    }
                    return;
                }
                _ => {}
            }
        }

        match event.get("channel").and_then(Value::as_str) {
            Some("agents") => {
                let agents = agents_from_value(
                    event
                        .get("payload")
                        .and_then(|payload| payload.get("agents"))
                        .unwrap_or(&Value::Null),
                );
                process_full_roster(&mut state, agents, focused_at_ms, now_ms);
            }
            Some("agent-upserted") => {
                if let Some(agent) = event
                    .get("payload")
                    .and_then(|payload| payload.get("agent"))
                    .and_then(notification_agent_from_value)
                {
                    process_upsert(&mut state, agent, focused_at_ms, now_ms);
                }
            }
            Some("agent-forgotten") => {
                if let Some(agent_id) = event
                    .get("payload")
                    .and_then(|payload| payload.get("agentId"))
                    .and_then(Value::as_str)
                {
                    process_forget(&mut state, agent_id);
                }
            }
            _ => {}
        }
    });

    HostNotificationsExtension {
        _subscription: subscription,
        _state: state,
    }
}

pub fn start_notifications_extension(
    auth: Arc<HostAuthExtension>,
    events: SandHostEventBus,
    initial_baseline: Option<Vec<NotificationAgent>>,
    window_focused_at: WindowFocusedAtSource,
) -> Result<HostNotificationsExtension, String> {
    let options = CursorMobilePushOptions::production(auth)?;
    let blocking_sender = create_cursor_mobile_push_sender(options);
    let notify: MobilePushNotify = Arc::new(move |input: MobilePushInput| {
        let sender = Arc::clone(&blocking_sender);
        thread::Builder::new()
            .name("sand-mobile-push".into())
            .spawn(move || {
                let _ = sender(input);
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    });

    Ok(start_notifications_extension_with_sender(
        events,
        initial_baseline,
        window_focused_at,
        Arc::new(system_now_ms),
        notify,
    ))
}
