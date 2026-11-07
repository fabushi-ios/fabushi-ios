use std::collections::BTreeMap;
use std::sync::Arc;

use prost::Message;
use uuid::Uuid;

use crate::cursor_backend::{
    resolve_sand_ghost_mode_header, send_cursor_unary_with_request_id,
};
use crate::extensions::auth::credential_renewer::get_configured_backend_url;
use crate::extensions::auth::extension::HostAuthExtension;

pub const SAND_MOBILE_PUSH_FOCUS_FRESHNESS_MS: u64 = 5 * 60_000;
pub const SAND_OS_NOTIFICATION_THROTTLE_MS: u64 = 5_000;
pub const NOTIFY_SAND_AGENT_TURN_FINISHED_PATH: &str =
    "/aiserver.v1.GrokBotService/NotifySandAgentTurnFinished";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationAgent {
    pub id: String,
    pub name: String,
    pub is_running: bool,
    pub awaiting_reason: Option<String>,
    pub notify_enabled: bool,
    pub is_hidden_from_sidebar: bool,
    pub last_message_id: Option<String>,
    pub last_message_preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobilePushInput {
    pub agent_id: String,
    pub agent_name: String,
    pub message_preview: String,
    pub last_message_id: String,
    pub awaiting_user_response: bool,
}

pub trait MobilePushAuth: Send + Sync {
    fn get_access_token(&self) -> Result<String, String>;
    fn get_machine_id(&self) -> Result<String, String>;
}

impl MobilePushAuth for HostAuthExtension {
    fn get_access_token(&self) -> Result<String, String> {
        HostAuthExtension::get_access_token(self).map_err(|error| error.to_string())
    }

    fn get_machine_id(&self) -> Result<String, String> {
        HostAuthExtension::get_machine_id(self).map_err(|error| error.to_string())
    }
}

pub type MobilePushNotify =
    Arc<dyn Fn(MobilePushInput) -> Result<(), String> + Send + Sync>;

#[derive(Clone)]
pub struct CursorMobilePushOptions {
    pub auth: Arc<dyn MobilePushAuth>,
    pub backend_url: String,
}

impl CursorMobilePushOptions {
    pub fn production(auth: Arc<dyn MobilePushAuth>) -> Result<Self, String> {
        Ok(Self {
            auth,
            backend_url: get_configured_backend_url().map_err(|error| error.to_string())?,
        })
    }
}

#[derive(Clone, PartialEq, Message)]
struct NotifySandAgentTurnFinishedRequestWire {
    #[prost(string, tag = "1")]
    agent_id: String,
    #[prost(string, tag = "2")]
    agent_name: String,
    #[prost(string, tag = "3")]
    message_preview: String,
    #[prost(string, tag = "4")]
    last_message_id: String,
    #[prost(bool, tag = "5")]
    awaiting_user_response: bool,
}

pub fn create_cursor_mobile_push_sender(
    options: CursorMobilePushOptions,
) -> MobilePushNotify {
    Arc::new(move |input| {
        let access_token = options.auth.get_access_token()?;
        let machine_id = options.auth.get_machine_id()?;
        let ghost_mode = resolve_sand_ghost_mode_header(
            &options.backend_url,
            &access_token,
            &machine_id,
        );
        let request = NotifySandAgentTurnFinishedRequestWire {
            agent_id: input.agent_id,
            agent_name: input.agent_name,
            message_preview: input.message_preview,
            last_message_id: input.last_message_id,
            awaiting_user_response: input.awaiting_user_response,
        };
        let request_id = Uuid::new_v4().to_string();
        let _ = send_cursor_unary_with_request_id(
            &options.backend_url,
            &access_token,
            &machine_id,
            NOTIFY_SAND_AGENT_TURN_FINISHED_PATH,
            &request.encode_to_vec(),
            None,
            ghost_mode,
            &request_id,
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    })
}

#[derive(Debug, Clone)]
struct Snapshot {
    agent: NotificationAgent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TransitionKind {
    Done,
    NeedsInput,
}

#[derive(Debug, Clone)]
struct Transition {
    agent_id: String,
    agent_name: String,
    kind: TransitionKind,
    last_message_id: Option<String>,
    last_message_preview: Option<String>,
}

#[derive(Default)]
struct NotificationDecider {
    previous: BTreeMap<String, Snapshot>,
    last_notified_at_ms: BTreeMap<(String, TransitionKind), u64>,
    accounted_message_id: BTreeMap<String, Option<String>>,
}

impl NotificationDecider {
    fn seed_baseline(&mut self, agents: &[NotificationAgent]) {
        for agent in agents {
            self.previous
                .entry(agent.id.clone())
                .or_insert_with(|| Snapshot {
                    agent: agent.clone(),
                });
            self.accounted_message_id
                .entry(agent.id.clone())
                .or_insert_with(|| agent.last_message_id.clone());
        }
    }

    fn decide_agent(
        &mut self,
        agent: &NotificationAgent,
        now_ms: u64,
        is_window_focused: bool,
    ) -> Vec<Transition> {
        let mut out = Vec::new();
        if let Some(before) = self
            .previous
            .get(&agent.id)
            .map(|snapshot| snapshot.agent.clone())
        {
            let became_awaiting =
                agent.awaiting_reason.is_some() && before.awaiting_reason.is_none();
            let finished_turn =
                before.is_running && !agent.is_running && agent.awaiting_reason.is_none();
            let kind = if became_awaiting {
                Some(TransitionKind::NeedsInput)
            } else if finished_turn {
                Some(TransitionKind::Done)
            } else {
                None
            };
            if let Some(kind) = kind {
                let accounted = self
                    .accounted_message_id
                    .get(&agent.id)
                    .cloned()
                    .flatten();
                let duplicate_done = kind == TransitionKind::Done
                    && (agent.last_message_id.is_none()
                        || agent.last_message_id == accounted);
                self.accounted_message_id
                    .insert(agent.id.clone(), agent.last_message_id.clone());
                let key = (agent.id.clone(), kind);
                let throttled = self.last_notified_at_ms.get(&key).is_some_and(|last| {
                    now_ms.saturating_sub(*last) < SAND_OS_NOTIFICATION_THROTTLE_MS
                });
                if !duplicate_done
                    && !agent.is_hidden_from_sidebar
                    && agent.notify_enabled
                    && !is_window_focused
                    && !throttled
                {
                    self.last_notified_at_ms.insert(key, now_ms);
                    out.push(Transition {
                        agent_id: agent.id.clone(),
                        agent_name: agent.name.clone(),
                        kind,
                        last_message_id: agent.last_message_id.clone(),
                        last_message_preview: agent.last_message_preview.clone(),
                    });
                }
            }
        } else {
            self.accounted_message_id
                .insert(agent.id.clone(), agent.last_message_id.clone());
        }
        self.previous.insert(
            agent.id.clone(),
            Snapshot {
                agent: agent.clone(),
            },
        );
        out
    }

    fn retain_agents(&mut self, agent_ids: &[String]) {
        self.previous
            .retain(|id, _| agent_ids.iter().any(|candidate| candidate == id));
    }

    fn forget(&mut self, agent_id: &str) {
        self.previous.remove(agent_id);
        self.accounted_message_id.remove(agent_id);
        self.last_notified_at_ms
            .retain(|(id, _), _| id != agent_id);
    }
}

pub struct SandMobilePushNotifier {
    decider: NotificationDecider,
    notify: MobilePushNotify,
    has_seeded_baseline: bool,
    pre_seed_deltas: Vec<(NotificationAgent, Option<u64>)>,
}

impl SandMobilePushNotifier {
    pub fn new(notify: MobilePushNotify) -> Self {
        Self {
            decider: NotificationDecider::default(),
            notify,
            has_seeded_baseline: false,
            pre_seed_deltas: Vec::new(),
        }
    }

    pub fn seed_baseline(&mut self, agents: &[NotificationAgent], now_ms: u64) {
        self.decider.seed_baseline(agents);
        self.flush_pre_seed_deltas(now_ms);
    }

    pub fn handle_agents_event(
        &mut self,
        agents: &[NotificationAgent],
        window_focused_at_ms: Option<u64>,
        now_ms: u64,
    ) {
        let focused = is_focused(window_focused_at_ms, now_ms);
        for agent in agents {
            let transitions = self.decider.decide_agent(agent, now_ms, focused);
            self.fire_transitions(transitions);
        }
        let ids = agents
            .iter()
            .map(|agent| agent.id.clone())
            .collect::<Vec<_>>();
        self.decider.retain_agents(&ids);
        self.flush_pre_seed_deltas(now_ms);
    }

    pub fn handle_agent_upserted(
        &mut self,
        agent: NotificationAgent,
        window_focused_at_ms: Option<u64>,
        now_ms: u64,
    ) {
        if !self.has_seeded_baseline {
            self.pre_seed_deltas
                .push((agent, window_focused_at_ms));
            return;
        }
        let focused = is_focused(window_focused_at_ms, now_ms);
        let transitions = self.decider.decide_agent(&agent, now_ms, focused);
        self.fire_transitions(transitions);
    }

    pub fn forget(&mut self, agent_id: &str) {
        self.decider.forget(agent_id);
    }

    fn flush_pre_seed_deltas(&mut self, now_ms: u64) {
        if self.has_seeded_baseline {
            return;
        }
        self.has_seeded_baseline = true;
        let buffered = std::mem::take(&mut self.pre_seed_deltas);
        for (agent, focused_at) in buffered {
            self.handle_agent_upserted(agent, focused_at, now_ms);
        }
    }

    fn fire_transitions(&self, transitions: Vec<Transition>) {
        for transition in transitions {
            let _ = (self.notify)(MobilePushInput {
                agent_id: transition.agent_id,
                agent_name: transition.agent_name,
                message_preview: transition.last_message_preview.unwrap_or_default(),
                last_message_id: transition.last_message_id.unwrap_or_default(),
                awaiting_user_response: transition.kind == TransitionKind::NeedsInput,
            });
        }
    }
}

fn is_focused(window_focused_at_ms: Option<u64>, now_ms: u64) -> bool {
    window_focused_at_ms.is_some_and(|focused_at| {
        now_ms.saturating_sub(focused_at) <= SAND_MOBILE_PUSH_FOCUS_FRESHNESS_MS
    })
}
