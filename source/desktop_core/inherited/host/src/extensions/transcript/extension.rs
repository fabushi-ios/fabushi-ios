use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::profile_watch::{
    ProductionProfileWatch, ProfileWatchEvent, ProfileWatchEventSink,
};
use super::roster_emit::{ProductionRosterEmit, RosterEventSink};
use super::roster_projection::OUTLINE_STREAM_COALESCE_MS;
use super::transcript_manager::{
    TranscriptManager, TranscriptManagerServices, TranscriptTurnExecutionPort,
};
use crate::extensions::attachments::attachments_service::AttachmentsService;
use crate::extensions::content_search::extension::ProductionContentSearchExtension;
use crate::extensions::memory::extension::HostMemoryExtension;
use crate::extensions::session::production::ProductionSessionWorkers;
use crate::extensions::telemetry::extension::HostTelemetryExtension;
use crate::extensions::trays::extension::HostTraysExtension;
use crate::extensions::turn_execution::turn_execution_service::TurnExecutionRegistry;
use crate::host_event_bus::{
    HostEventFailureMode, HostEventSubscription, SandHostEventBus,
};
use crate::transcript_mutation_events::{
    TRANSCRIPT_MUTATION_ORIGIN_KEY, TranscriptMutationSubscription, subscribe_transcript_mutations,
};

pub const TRANSCRIPT_EXTENSION_ID: &str = "transcript";
pub const TRANSCRIPT_EXTENSION_DEPENDENCIES: &[&str] = &[
    "attachments",
    "content-search",
    "memory",
    "session",
    "telemetry",
    "trays",
    "turn-execution",
];

pub const TRANSCRIPT_PROFILE_CHANGED_TOPIC: &str = "transcript.profile-changed";
pub const TRANSCRIPT_TIMELINE_EVENT_TOPIC: &str = "transcript.timeline-event";

#[derive(Clone)]
pub struct TranscriptExtensionDeps {
    pub attachments: Arc<AttachmentsService>,
    pub content_search: Arc<ProductionContentSearchExtension>,
    pub memory: HostMemoryExtension,
    pub telemetry: HostTelemetryExtension,
    pub trays: Arc<HostTraysExtension>,
    pub turn_execution: Arc<Mutex<TurnExecutionRegistry>>,
    pub events: SandHostEventBus,
}

#[derive(Clone)]
pub struct TranscriptExtensionEventBridge {
    events: SandHostEventBus,
}

impl TranscriptExtensionEventBridge {
    pub fn new(events: SandHostEventBus) -> Self {
        Self { events }
    }

    fn emit(&self, topic: &str, payload: serde_json::Value) {
        let _ = self
            .events
            .emit_topic(topic, &payload, HostEventFailureMode::Continue);
    }

    pub fn profile_changed(&self, agent_id: &str) {
        self.emit(
            TRANSCRIPT_PROFILE_CHANGED_TOPIC,
            json!({"agentId": agent_id}),
        );
    }

    pub fn timeline_event(&self, agent_id: &str, event: Value) {
        self.emit(
            TRANSCRIPT_TIMELINE_EVENT_TOPIC,
            json!({"agentId": agent_id, "event": event}),
        );
    }

    pub fn profile_watch_event_sink(&self) -> ProfileWatchEventSink {
        let bridge = self.clone();
        Arc::new(move |event| match event {
            ProfileWatchEvent::Timeline { agent_id, event } => {
                bridge.timeline_event(&agent_id, event);
            }
            ProfileWatchEvent::ProfileChanged { agent_id } => {
                bridge.profile_changed(&agent_id);
            }
        })
    }

    pub fn subscribe_profile_changed<F>(&self, listener: F) -> HostEventSubscription
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        self.events.on(TRANSCRIPT_PROFILE_CHANGED_TOPIC, move |payload| {
            if let Some(agent_id) = payload.get("agentId").and_then(Value::as_str) {
                listener(agent_id);
            }
            Ok(())
        })
    }

    pub fn bind_profile_watch_projection(
        &self,
        roster: Arc<ProductionRosterEmit>,
    ) -> Vec<HostEventSubscription> {
        let timeline_roster = Arc::clone(&roster);
        let timeline = self
            .events
            .on(TRANSCRIPT_TIMELINE_EVENT_TOPIC, move |payload| {
                let Some(agent_id) = payload.get("agentId").and_then(Value::as_str) else {
                    return Ok(());
                };
                let Some(event) = payload.get("event") else {
                    return Ok(());
                };
                timeline_roster.publish_timeline_event(agent_id, event.clone());
                Ok(())
            });
        let profile = self.subscribe_profile_changed(move |agent_id| {
            roster.publish_profile_changed(agent_id);
        });
        vec![timeline, profile]
    }

    pub fn automation_config_changed(&self) {
        self.emit("transcript.automation-config-changed", json!({}));
    }

    pub fn channel_config_changed(&self) {
        self.emit("transcript.channel-config-changed", json!({}));
    }

    pub fn listener_connect_card(&self, agent_id: &str, platform: &str) {
        self.emit(
            "transcript.listener-connect-card",
            json!({"agentId": agent_id, "platform": platform}),
        );
    }

    pub fn run_started(&self, request_id: &str) {
        self.emit("transcript.run-started", json!({"requestId": request_id}));
    }

    pub fn run_ended(&self, request_id: &str) {
        self.emit("transcript.run-ended", json!({"requestId": request_id}));
    }
}

/// Shipping Grok-shaped Transcript extension owner.
///
/// This is the production composition boundary corresponding to frozen
/// `source/host/extensions/transcript/extension.ts`: it retains the exact Host
/// dependency owners and installs the Host-event observers at extension start,
/// while delegated transcript domains remain independently testable modules.
pub struct TranscriptExtension {
    manager: Arc<TranscriptManager>,
    roster_emit: Arc<ProductionRosterEmit>,
    profile_watch: Option<ProductionProfileWatch>,
    profile_watch_error: Option<String>,
    deps: Option<TranscriptExtensionDeps>,
    events: Option<TranscriptExtensionEventBridge>,
    _profile_watch_subscriptions: Vec<HostEventSubscription>,
    _outline_stream_subscription: Option<HostEventSubscription>,
    _human_transcript_subscription: Option<TranscriptMutationSubscription>,
    human_transcript_alive: Arc<Mutex<bool>>,
}

impl TranscriptExtension {
    pub fn manager(&self) -> Arc<TranscriptManager> {
        Arc::clone(&self.manager)
    }

    pub fn roster_emit(&self) -> Arc<ProductionRosterEmit> {
        Arc::clone(&self.roster_emit)
    }

    pub fn event_bridge(&self) -> Option<TranscriptExtensionEventBridge> {
        self.events.clone()
    }

    pub fn profile_watch_active(&self) -> bool {
        self.profile_watch.is_some()
    }

    pub fn profile_watch_error(&self) -> Option<&str> {
        self.profile_watch_error.as_deref()
    }
}

impl Drop for TranscriptExtension {
    fn drop(&mut self) {
        // Wait for an admitted read/publication, then reject even callbacks
        // already copied by the global registry before unsubscribe.
        if let Ok(mut alive) = self.human_transcript_alive.lock() {
            *alive = false;
        }
        if let Some(subscription) = self._human_transcript_subscription.take() {
            subscription.unsubscribe();
        }
        self._outline_stream_subscription.take();
        self._profile_watch_subscriptions.clear();
        self.profile_watch.take();
        self.manager.dispose();
        self.events.take();
        self.deps.take();
    }
}

pub fn start_transcript_extension(
    root_dir: &Path,
    sessions: Arc<ProductionSessionWorkers>,
    event_sink: RosterEventSink,
) -> TranscriptExtension {
    start_transcript_extension_with_event_bridge(root_dir, sessions, event_sink, None)
}

fn start_transcript_extension_with_event_bridge(
    root_dir: &Path,
    sessions: Arc<ProductionSessionWorkers>,
    event_sink: RosterEventSink,
    events: Option<TranscriptExtensionEventBridge>,
) -> TranscriptExtension {
    let manager = Arc::new(TranscriptManager::new(root_dir, Arc::clone(&sessions)));
    let roster_emit = Arc::new(ProductionRosterEmit::new(
        Arc::clone(&sessions),
        manager.transcript_runtime(),
        event_sink,
    ));
    roster_emit.set_outline_stream_coalescing_ms(OUTLINE_STREAM_COALESCE_MS);
    manager
        .bind_roster_emit(Arc::clone(&roster_emit))
        .expect("Transcript roster must be configured exactly once through TranscriptManager");

    let (profile_events, profile_watch_subscriptions) = if let Some(events) = events.as_ref() {
        (
            events.profile_watch_event_sink(),
            events.bind_profile_watch_projection(Arc::clone(&roster_emit)),
        )
    } else {
        let profile_roster = Arc::clone(&roster_emit);
        let sink: ProfileWatchEventSink = Arc::new(move |event| match event {
            ProfileWatchEvent::Timeline { agent_id, event } => {
                profile_roster.publish_timeline_event(&agent_id, event);
            }
            ProfileWatchEvent::ProfileChanged { agent_id } => {
                profile_roster.publish_profile_changed(&agent_id);
            }
        });
        (sink, Vec::new())
    };

    let human_transcript_alive = Arc::new(Mutex::new(true));
    let human_transcript_subscription = bind_human_transcript_updates(
        Arc::clone(&sessions),
        Arc::clone(&roster_emit),
        Arc::clone(&human_transcript_alive),
    );
    let (profile_watch, profile_watch_error) = match ProductionProfileWatch::start(
        sessions,
        Arc::clone(&roster_emit),
        profile_events,
    ) {
        Ok(watch) => (Some(watch), None),
        Err(error) => (None, Some(error.to_string())),
    };
    TranscriptExtension {
        manager,
        roster_emit,
        profile_watch,
        profile_watch_error,
        deps: None,
        events,
        _profile_watch_subscriptions: profile_watch_subscriptions,
        _outline_stream_subscription: None,
        _human_transcript_subscription: Some(human_transcript_subscription),
        human_transcript_alive,
    }
}

fn bind_human_transcript_updates(
    sessions: Arc<ProductionSessionWorkers>,
    roster: Arc<ProductionRosterEmit>,
    alive: Arc<Mutex<bool>>,
) -> TranscriptMutationSubscription {
    subscribe_transcript_mutations(move |mutation| {
        if mutation.get("kind").and_then(Value::as_str) != Some("entries-upserted") {
            return;
        }
        let Ok(admitted) = alive.lock() else { return; };
        if !*admitted {
            return;
        }
        let Some(agent_id) = mutation.get("agentId").and_then(Value::as_str) else { return; };
        let Some(origin) = mutation.get(TRANSCRIPT_MUTATION_ORIGIN_KEY).and_then(Value::as_str) else { return; };
        let Some(entries) = mutation.get("entries").and_then(Value::as_array) else { return; };
        let Some(entry_ids) = entries.iter()
            .map(|entry| entry.get("id").and_then(Value::as_str).map(str::to_owned))
            .collect::<Option<Vec<_>>>()
        else { return; };
        // The registry is process-wide. Only this Session's authorized Human
        // owner can supply content; notification bodies are never forwarded.
        let Ok(current) = sessions.read_human_transcript_mutation_entries(agent_id, origin, &entry_ids) else {
            return;
        };
        for entry in current {
            roster.publish_transcript_updated(agent_id, &entry);
        }
        drop(admitted);
    })
}

pub fn bind_runner_outline_stream_events(
    events: &SandHostEventBus,
    roster: Arc<ProductionRosterEmit>,
) -> HostEventSubscription {
    events.subscribe_listener(move |event| {
        if event.get("channel").and_then(serde_json::Value::as_str) != Some("runner-inference") {
            return;
        }
        let Some(payload) = event.get("payload") else {
            return;
        };
        let Some(stream_id) = payload.get("streamId").and_then(serde_json::Value::as_str) else {
            return;
        };
        match payload.get("type").and_then(serde_json::Value::as_str) {
            Some("delta") => {
                let Some(agent_id) = payload.get("agentId").and_then(serde_json::Value::as_str) else {
                    return;
                };
                let Some(content) = payload.get("content").and_then(serde_json::Value::as_str) else {
                    return;
                };
                roster.apply_runner_text_delta(agent_id, stream_id, content);
            }
            Some("completed" | "failed" | "cancelled") => {
                roster.finish_runner_outline_stream(stream_id);
            }
            _ => {}
        }
    })
}

pub fn start_production_transcript_extension(
    root_dir: &Path,
    sessions: Arc<ProductionSessionWorkers>,
    deps: TranscriptExtensionDeps,
) -> TranscriptExtension {
    let events = TranscriptExtensionEventBridge::new(deps.events.clone());
    let roster_events = deps.events.clone();
    let mut extension = start_transcript_extension_with_event_bridge(
        root_dir,
        sessions,
        Arc::new(move |event| roster_events.publish(event)),
        Some(events.clone()),
    );
    extension
        .manager
        .set_production_services(TranscriptManagerServices {
            telemetry: Arc::clone(&deps.telemetry.brain),
            product_analytics: deps.telemetry.analytics.clone(),
            trace_flusher: deps.telemetry.api(),
            memory: deps.memory.clone(),
            content_search: Arc::clone(&deps.content_search),
            attachments: Arc::clone(&deps.attachments),
            trays: Arc::clone(&deps.trays),
        })
        .expect("Transcript production services must be configured exactly once");
    extension
        .manager
        .set_turn_execution(TranscriptTurnExecutionPort::new(Arc::clone(
            &deps.turn_execution,
        )));
    let automation_events = events.clone();
    extension
        .manager
        .set_automation_config_changed_observer(Some(Arc::new(move || {
            automation_events.automation_config_changed();
        })));
    let channel_config_events = events.clone();
    extension
        .manager
        .set_channel_config_changed_observer(Some(Arc::new(move || {
            channel_config_events.channel_config_changed();
        })));
    let listener_events = events.clone();
    extension
        .manager
        .set_listener_connect_observer(Some(Arc::new(move |agent_id, platform| {
            listener_events.listener_connect_card(agent_id, platform);
        })));
    let outline_stream_subscription =
        bind_runner_outline_stream_events(&deps.events, Arc::clone(&extension.roster_emit));
    let lifecycle_events = events.clone();
    extension
        .manager
        .set_agent_run_lifecycle_observer(Some(Arc::new(move |event_type, request_id| {
            if event_type == "started" {
                lifecycle_events.run_started(request_id);
            } else if event_type == "ended" {
                lifecycle_events.run_ended(request_id);
            }
        })));
    extension.deps = Some(deps);
    extension._outline_stream_subscription = Some(outline_stream_subscription);
    extension
}
