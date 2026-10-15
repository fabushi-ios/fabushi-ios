const AUTOMATION_SPEND_GUARD_IDLE_TTL_MS: i64 = 3 * 24 * 60 * 60 * 1_000;
const AUTOMATION_SPEND_GUARD_MIN_UNREAD_COUNT: usize = 15;
const AUTOMATION_SPEND_GUARD_MIN_FIRES_SINCE_VIEWED: usize = 20;
const AUTOMATION_SPEND_GUARD_PAUSE_DELAY_MS: i64 = 3 * 24 * 60 * 60 * 1_000;
const AUTOMATION_SPEND_GUARD_SNOOZE_MS: i64 = 30 * 24 * 60 * 60 * 1_000;
const AUTOMATION_SPEND_GUARD_STORE_VERSION: u32 = 1;
const AUTOMATION_SPEND_GUARD_STORE_MAX_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct AutomationSpendGuardAgentState {
    last_activity_at_ms: i64,
    last_viewed_at_ms: i64,
    unread_count: usize,
    nudged_at_ms: Option<i64>,
    snoozed_until_ms: Option<i64>,
    opted_out: bool,
    card_entry_ids: Vec<String>,
    paused_automation_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct AutomationSpendGuardJournal {
    version: u32,
    agents: BTreeMap<String, AutomationSpendGuardAgentState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutomationSpendGuardDecision {
    OptedOut,
    UserActive,
    Snoozed,
    Pause,
    AwaitingAck,
    Nudge,
    BelowThresholds,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AutomationSpendGuardAdmission {
    Continue(Option<String>),
    Suppress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutomationSpendGuardAnswer {
    Keep,
    Pause,
    OptOut,
    Resume,
    StayPaused,
}

impl AutomationSpendGuardAnswer {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "spend-guard:keep" => Some(Self::Keep),
            "spend-guard:pause" => Some(Self::Pause),
            "spend-guard:never-ask" => Some(Self::OptOut),
            "spend-guard:resume" => Some(Self::Resume),
            "spend-guard:stay-paused" => Some(Self::StayPaused),
            _ => None,
        }
    }

    fn acknowledgement(self) -> &'static str {
        match self {
            Self::Keep => "keep your routines running",
            Self::Pause => "pause your routines",
            Self::OptOut => "keep your routines running and stop asking",
            Self::Resume => "resume the routines that the app paused",
            Self::StayPaused => "keep your routines paused",
        }
    }
}

fn automation_spend_guard_decision(
    guard: &AutomationSpendGuardAgentState,
    fires_since_viewed: usize,
    now_ms: i64,
) -> AutomationSpendGuardDecision {
    if guard.opted_out {
        return AutomationSpendGuardDecision::OptedOut;
    }
    if guard.last_viewed_at_ms > 0
        && now_ms.saturating_sub(guard.last_viewed_at_ms) < AUTOMATION_SPEND_GUARD_IDLE_TTL_MS
    {
        return AutomationSpendGuardDecision::UserActive;
    }
    if guard
        .snoozed_until_ms
        .is_some_and(|snoozed_until_ms| snoozed_until_ms > now_ms)
    {
        return AutomationSpendGuardDecision::Snoozed;
    }
    if let Some(nudged_at_ms) = guard
        .nudged_at_ms
        .filter(|nudged_at_ms| *nudged_at_ms > guard.last_viewed_at_ms)
    {
        if now_ms.saturating_sub(nudged_at_ms) >= AUTOMATION_SPEND_GUARD_PAUSE_DELAY_MS {
            return AutomationSpendGuardDecision::Pause;
        }
        return AutomationSpendGuardDecision::AwaitingAck;
    }
    if guard.unread_count >= AUTOMATION_SPEND_GUARD_MIN_UNREAD_COUNT
        || fires_since_viewed >= AUTOMATION_SPEND_GUARD_MIN_FIRES_SINCE_VIEWED
    {
        return AutomationSpendGuardDecision::Nudge;
    }
    AutomationSpendGuardDecision::BelowThresholds
}

fn automation_spend_guard_path(automation_path: &Path) -> PathBuf {
    automation_path.with_extension("spend-guard.json")
}

fn load_automation_spend_guard(
    path: &Path,
) -> BTreeMap<String, AutomationSpendGuardAgentState> {
    let Ok(bytes) = std::fs::read(path) else {
        return BTreeMap::new();
    };
    if bytes.len() > AUTOMATION_SPEND_GUARD_STORE_MAX_BYTES {
        return BTreeMap::new();
    }
    let Ok(mut journal) = serde_json::from_slice::<AutomationSpendGuardJournal>(&bytes) else {
        return BTreeMap::new();
    };
    if journal.version != AUTOMATION_SPEND_GUARD_STORE_VERSION {
        return BTreeMap::new();
    }
    journal.agents.retain(|agent_id, state| {
        let valid = is_safe_memory_agent_id(agent_id);
        if valid {
            state.card_entry_ids.truncate(32);
            state.paused_automation_ids.truncate(256);
            state.unread_count = state.unread_count.min(100_000);
        }
        valid
    });
    journal.agents
}

fn persist_automation_spend_guard(
    path: Option<&Path>,
    agents: &BTreeMap<String, AutomationSpendGuardAgentState>,
) -> Result<(), FeatureHostError> {
    let Some(path) = path else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            FeatureHostError::Contract(format!("create spend guard directory: {error}"))
        })?;
    }
    let bytes = serde_json::to_vec(&AutomationSpendGuardJournal {
        version: AUTOMATION_SPEND_GUARD_STORE_VERSION,
        agents: agents.clone(),
    })
    .map_err(|error| FeatureHostError::Contract(format!("encode spend guard state: {error}")))?;
    if bytes.len() > AUTOMATION_SPEND_GUARD_STORE_MAX_BYTES {
        return Err(FeatureHostError::Contract(
            "spend guard state exceeds durable store bound".into(),
        ));
    }
    let temporary = path.with_extension(format!(
        "spend-guard.json.{}.{}.tmp",
        std::process::id(),
        Uuid::new_v4()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|error| {
        FeatureHostError::Contract(format!("open spend guard temporary store: {error}"))
    })?;
    std::io::Write::write_all(&mut file, &bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| FeatureHostError::Contract(format!("write spend guard state: {error}")))?;
    match std::fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(_error) if path.exists() => {
            std::fs::remove_file(path).map_err(|error| {
                FeatureHostError::Contract(format!("replace spend guard state: {error}"))
            })?;
            std::fs::rename(&temporary, path).map_err(|error| {
                FeatureHostError::Contract(format!("commit spend guard state: {error}"))
            })
        }
        Err(error) => Err(FeatureHostError::Contract(format!(
            "commit spend guard state: {error}"
        ))),
    }
}

fn spend_guard_agent_for_conversation(
    state: &FeatureState,
    conversation_id: &str,
) -> Option<String> {
    if conversation_id == MAHAYANA_AI_CONVERSATION_ID {
        return Some("mahayana-assistant".into());
    }
    state
        .bots
        .iter()
        .find(|(_, bot)| bot.conversation_id.as_deref() == Some(conversation_id))
        .map(|(agent_id, _)| agent_id.clone())
}

fn spend_guard_fires_since_viewed(
    state: &FeatureState,
    agent_id: &str,
    last_viewed_at_ms: i64,
) -> usize {
    state
        .automations
        .values()
        .filter(|automation| {
            automation.agent_id.as_deref().unwrap_or("mahayana-assistant") == agent_id
        })
        .flat_map(|automation| automation.runs.iter())
        .filter(|run| run.started_at > last_viewed_at_ms)
        .count()
}

fn spend_guard_widget(paused: bool) -> TranscriptCard {
    let (prompt, options) = if paused {
        (
            "I paused all your routines while you were away to avoid wasted spend. Want me to start them back up?".to_string(),
            vec![
                mahayana_host_protocol::WidgetChoiceOption {
                    label: "Resume routines".into(),
                    value: Some("spend-guard:resume".into()),
                    description: None,
                    style: Some("primary".into()),
                },
                mahayana_host_protocol::WidgetChoiceOption {
                    label: "Keep them paused".into(),
                    value: Some("spend-guard:stay-paused".into()),
                    description: None,
                    style: None,
                },
            ],
        )
    } else {
        (
            "You've been away for a bit — keep my routines running?".to_string(),
            vec![
                mahayana_host_protocol::WidgetChoiceOption {
                    label: "Keep them running".into(),
                    value: Some("spend-guard:keep".into()),
                    description: None,
                    style: Some("primary".into()),
                },
                mahayana_host_protocol::WidgetChoiceOption {
                    label: "Pause them all".into(),
                    value: Some("spend-guard:pause".into()),
                    description: None,
                    style: None,
                },
                mahayana_host_protocol::WidgetChoiceOption {
                    label: "Keep running, don't ask again".into(),
                    value: Some("spend-guard:never-ask".into()),
                    description: None,
                    style: None,
                },
            ],
        )
    };
    TranscriptCard::Widget {
        widget: mahayana_host_protocol::WidgetQuestion {
            prompt,
            help_text: None,
            options,
            allow_custom: false,
            dismiss_on_move_on: false,
        },
        responded_value: None,
        widget_dismissed: false,
        widget_skipped: false,
    }
}

fn spend_guard_card_accepts(card: &TranscriptCard, value: &str) -> bool {
    let TranscriptCard::Widget { widget, .. } = card else {
        return false;
    };
    widget
        .options
        .iter()
        .any(|option| option.value.as_deref() == Some(value))
}

fn spend_guard_preserved_unread(
    current: &AutomationSpendGuardAgentState,
) -> AutomationSpendGuardAgentState {
    AutomationSpendGuardAgentState {
        last_activity_at_ms: current.last_activity_at_ms,
        last_viewed_at_ms: current.last_viewed_at_ms,
        unread_count: current.unread_count,
        ..AutomationSpendGuardAgentState::default()
    }
}

fn spend_guard_disable_agent_automations(
    state: &mut FeatureState,
    agent_id: &str,
) -> Vec<String> {
    let mut paused = Vec::new();
    for automation in state.automations.values_mut() {
        if automation.agent_id.as_deref().unwrap_or("mahayana-assistant") != agent_id
            || !automation.enabled
        {
            continue;
        }
        automation.enabled = false;
        automation.next_run_at_ms = None;
        paused.push(automation.id.clone());
    }
    paused
}

fn spend_guard_resume_agent_automations(
    state: &mut FeatureState,
    agent_id: &str,
    ids: &[String],
    now_ms: i64,
    user_time_zone: Option<&str>,
) -> Vec<String> {
    let mut resumed = Vec::new();
    for id in ids {
        let Some(automation) = state.automations.get_mut(id) else {
            continue;
        };
        if automation.agent_id.as_deref().unwrap_or("mahayana-assistant") != agent_id
            || automation.enabled
        {
            continue;
        }
        automation.enabled = true;
        let trigger = automation
            .trigger
            .clone()
            .unwrap_or_else(|| AutomationTrigger::Schedule {
                schedule: automation.schedule.clone(),
            });
        automation.next_run_at_ms = automation_next_run(
            &trigger,
            &automation.schedule,
            true,
            now_ms,
            user_time_zone,
        );
        resumed.push(id.clone());
    }
    resumed
}

impl FeatureHostController {
    fn automation_spend_guard_path(&self) -> Option<PathBuf> {
        self.active_account_root(self.automation_path.as_deref())
            .map(|path| automation_spend_guard_path(&path))
    }

    fn persist_automation_spend_guard_state(
        &self,
        agents: &BTreeMap<String, AutomationSpendGuardAgentState>,
    ) -> Result<(), FeatureHostError> {
        let path = self.automation_spend_guard_path();
        persist_automation_spend_guard(path.as_deref(), agents)
    }

    fn mark_automation_spend_guard_viewed(
        &self,
        agent_id: &str,
        at_ms: i64,
    ) -> Result<(), FeatureHostError> {
        let snapshot = {
            let mut state = self.state()?;
            let guard = state
                .automation_spend_guard
                .entry(agent_id.to_string())
                .or_default();
            guard.last_viewed_at_ms = guard.last_viewed_at_ms.max(at_ms);
            guard.unread_count = 0;
            state.automation_spend_guard.clone()
        };
        self.persist_automation_spend_guard_state(&snapshot)
    }

    fn mark_automation_spend_guard_viewed_for_conversation(
        &self,
        conversation_id: &str,
        at_ms: i64,
    ) -> Result<(), FeatureHostError> {
        let agent_id = {
            let state = self.state()?;
            spend_guard_agent_for_conversation(&state, conversation_id)
        };
        if let Some(agent_id) = agent_id {
            self.mark_automation_spend_guard_viewed(&agent_id, at_ms)?;
        }
        Ok(())
    }

    fn mark_automation_spend_guard_activity(
        &self,
        agent_id: &str,
        at_ms: i64,
    ) -> Result<(), FeatureHostError> {
        let snapshot = {
            let mut state = self.state()?;
            let guard = state
                .automation_spend_guard
                .entry(agent_id.to_string())
                .or_default();
            if guard.last_activity_at_ms >= at_ms {
                return Ok(());
            }
            guard.last_activity_at_ms = at_ms;
            guard.unread_count = guard.unread_count.saturating_add(1).min(100_000);
            state.automation_spend_guard.clone()
        };
        self.persist_automation_spend_guard_state(&snapshot)
    }

    fn spend_guard_conversation_id(
        state: &FeatureState,
        agent_id: &str,
    ) -> Result<String, FeatureHostError> {
        if agent_id == "mahayana-assistant" {
            return Ok(MAHAYANA_AI_CONVERSATION_ID.to_string());
        }
        state
            .bots
            .get(agent_id)
            .and_then(|bot| bot.conversation_id.clone())
            .ok_or_else(|| {
                FeatureHostError::Contract(format!(
                    "spend guard owner has no canonical conversation: {agent_id}"
                ))
            })
    }

    fn issue_automation_spend_guard_card(
        &self,
        state: &mut FeatureState,
        agent_id: &str,
        paused: bool,
        now_ms: i64,
    ) -> Result<String, FeatureHostError> {
        let card = spend_guard_widget(paused);
        let conversation_id = Self::spend_guard_conversation_id(state, agent_id)?;
        let message_id = mahayana_core::MessageId::generated("spend-guard");
        let entry_id = format!("{}-card-0", message_id.as_str());

        #[cfg(feature = "production")]
        if self.config.mode == HostMode::Production {
            let encoded = serde_json::to_value(&card).map_err(|error| {
                FeatureHostError::Contract(format!("encode spend guard card: {error}"))
            })?;
            let inserted = self.runtime()?.append_conversation_message(
                ConversationId(conversation_id.clone()),
                mahayana_core::Message {
                    id: message_id,
                    conversation_id: ConversationId(conversation_id),
                    role: RuntimeMessageRole::Assistant,
                    text: String::new(),
                    created_at_ms: now_ms,
                    metadata: json!({
                        "runtime": "fabushi-feature-host",
                        "transcriptCard": encoded,
                    }),
                },
            )?;
            if !inserted {
                return Err(FeatureHostError::Contract(
                    "spend guard canonical transcript append was rejected".into(),
                ));
            }
        }

        state.events.push_back(HostEvent::TranscriptCard {
            timestamp: timestamp(),
            entry_id: entry_id.clone(),
            operation_id: None,
            card,
        });
        let guard = state
            .automation_spend_guard
            .entry(agent_id.to_string())
            .or_default();
        guard.last_activity_at_ms = guard.last_activity_at_ms.max(now_ms);
        guard.unread_count = guard.unread_count.saturating_add(1).min(100_000);
        Ok(entry_id)
    }

    fn emit_spend_guard_automation_changes(
        state: &mut FeatureState,
        ids: &[String],
        action: &str,
    ) {
        for id in ids {
            if let Some(automation) = state.automations.get(id).cloned() {
                state.events.push_back(HostEvent::AutomationChanged {
                    timestamp: timestamp(),
                    action: action.to_string(),
                    automation,
                });
            }
        }
    }

    fn apply_automation_spend_guard_before_admission(
        &self,
        automation_id: &str,
        agent_id: Option<&str>,
        trigger: &RoutineTrigger,
        primary_run_id: Option<&str>,
        coalesced_run_ids: &[String],
    ) -> Result<AutomationSpendGuardAdmission, FeatureHostError> {
        if matches!(trigger, RoutineTrigger::Manual) {
            return Ok(AutomationSpendGuardAdmission::Continue(None));
        }
        let user_time_zone = self.host_settings_user_timezone_direct()?;
        let now_ms = now_millis();
        let mut state = self.state()?;
        let automation = state
            .automations
            .get(automation_id)
            .cloned()
            .ok_or_else(|| FeatureHostError::Contract(format!(
                "unknown automation: {automation_id}"
            )))?;
        ensure_automation_agent_scope(&automation, agent_id)?;
        let owner = automation
            .agent_id
            .clone()
            .unwrap_or_else(|| "mahayana-assistant".into());
        let current = state
            .automation_spend_guard
            .get(&owner)
            .cloned()
            .unwrap_or_default();
        let fires_since_viewed =
            spend_guard_fires_since_viewed(&state, &owner, current.last_viewed_at_ms);
        let decision = automation_spend_guard_decision(&current, fires_since_viewed, now_ms);

        match decision {
            AutomationSpendGuardDecision::Nudge => {
                let entry_id =
                    self.issue_automation_spend_guard_card(&mut state, &owner, false, now_ms)?;
                let after_card = state
                    .automation_spend_guard
                    .get(&owner)
                    .cloned()
                    .unwrap_or(current);
                let mut next = spend_guard_preserved_unread(&after_card);
                next.nudged_at_ms = Some(now_ms);
                next.card_entry_ids = vec![entry_id];
                state.automation_spend_guard.insert(owner, next);
                let snapshot = state.automation_spend_guard.clone();
                drop(state);
                self.persist_automation_spend_guard_state(&snapshot)?;
                Ok(AutomationSpendGuardAdmission::Continue(Some(format!(
                    "<system_reminder>\nThe user has been away while your routines kept firing; there are {fires_since_viewed} routine runs since their last view. They may be spending money on work nobody is reading.\nThe app has already asked them directly whether to keep your routines running, and applies their answer itself. Do NOT ask again yourself and do NOT edit any automation.json; just acknowledge their choice if it comes back as their reply.\nIf they neither answer nor return within three days, the app will pause ALL of this agent's routines and tell them so.\n</system_reminder>"
                ))))
            }
            AutomationSpendGuardDecision::AwaitingAck => {
                if current.card_entry_ids.is_empty() {
                    let entry_id =
                        self.issue_automation_spend_guard_card(&mut state, &owner, false, now_ms)?;
                    state
                        .automation_spend_guard
                        .entry(owner)
                        .or_default()
                        .card_entry_ids = vec![entry_id];
                    let snapshot = state.automation_spend_guard.clone();
                    drop(state);
                    self.persist_automation_spend_guard_state(&snapshot)?;
                }
                Ok(AutomationSpendGuardAdmission::Continue(None))
            }
            AutomationSpendGuardDecision::Pause => {
                let before_automations = state.automations.clone();
                let paused = spend_guard_disable_agent_automations(&mut state, &owner);
                if paused.is_empty() {
                    return Ok(AutomationSpendGuardAdmission::Suppress);
                }
                self.persist_automations(&state.automations)?;
                prune_pending_listener_resumes(&mut state);
                prune_pending_automation_event_batches(&mut state);
                let after_automations = state.automations.clone();
                push_automation_lifecycle_diff(
                    &mut state,
                    &before_automations,
                    &after_automations,
                    AutomationLifecycleSource::SpendGuard,
                    user_time_zone.as_deref(),
                    now_ms,
                );
                Self::emit_spend_guard_automation_changes(&mut state, &paused, "disabled");
                let already_open = !current.paused_automation_ids.is_empty();
                let issued = if already_open {
                    None
                } else {
                    Some(self.issue_automation_spend_guard_card(
                        &mut state,
                        &owner,
                        true,
                        now_ms,
                    )?)
                };
                let after_card = state
                    .automation_spend_guard
                    .get(&owner)
                    .cloned()
                    .unwrap_or(current.clone());
                let mut next = spend_guard_preserved_unread(&after_card);
                next.nudged_at_ms = current.nudged_at_ms;
                next.card_entry_ids = current.card_entry_ids.clone();
                if let Some(entry_id) = issued {
                    next.card_entry_ids.push(entry_id);
                }
                let mut paused_ids = current
                    .paused_automation_ids
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>();
                paused_ids.extend(paused);
                next.paused_automation_ids = paused_ids.into_iter().collect();
                state.automation_spend_guard.insert(owner, next);
                if let Some(run_id) = primary_run_id {
                    Self::record_dropped_event_fire(
                        &mut state,
                        run_id.to_string(),
                        automation_id,
                        "user_away_paused",
                    );
                }
                for run_id in coalesced_run_ids {
                    Self::record_dropped_event_fire(
                        &mut state,
                        run_id.clone(),
                        automation_id,
                        "user_away_paused",
                    );
                }
                let snapshot = state.automation_spend_guard.clone();
                drop(state);
                self.persist_automation_spend_guard_state(&snapshot)?;
                Ok(AutomationSpendGuardAdmission::Suppress)
            }
            AutomationSpendGuardDecision::OptedOut
            | AutomationSpendGuardDecision::UserActive
            | AutomationSpendGuardDecision::Snoozed
            | AutomationSpendGuardDecision::BelowThresholds => {
                Ok(AutomationSpendGuardAdmission::Continue(None))
            }
        }
    }

    fn apply_automation_spend_guard_widget_answer(
        &self,
        agent_id: &str,
        entry_id: &str,
        card: &TranscriptCard,
        value: &str,
    ) -> Result<Option<String>, FeatureHostError> {
        let Some(answer) = AutomationSpendGuardAnswer::parse(value) else {
            return Ok(None);
        };
        if !spend_guard_card_accepts(card, value) {
            return Ok(None);
        }
        let user_time_zone = self.host_settings_user_timezone_direct()?;
        let now_ms = now_millis();
        let mut state = self.state()?;
        let current = state
            .automation_spend_guard
            .get(agent_id)
            .cloned()
            .unwrap_or_default();
        if !current.card_entry_ids.iter().any(|id| id == entry_id) {
            return Ok(None);
        }

        let mut next = AutomationSpendGuardAgentState {
            last_activity_at_ms: current.last_activity_at_ms,
            last_viewed_at_ms: current.last_viewed_at_ms.max(now_ms),
            unread_count: 0,
            ..AutomationSpendGuardAgentState::default()
        };

        match answer {
            AutomationSpendGuardAnswer::Keep | AutomationSpendGuardAnswer::Resume => {
                let before_automations = state.automations.clone();
                let resumed = spend_guard_resume_agent_automations(
                    &mut state,
                    agent_id,
                    &current.paused_automation_ids,
                    now_ms,
                    user_time_zone.as_deref(),
                );
                if !resumed.is_empty() {
                    self.persist_automations(&state.automations)?;
                    let after_automations = state.automations.clone();
                    push_automation_lifecycle_diff(
                        &mut state,
                        &before_automations,
                        &after_automations,
                        AutomationLifecycleSource::SpendGuard,
                        user_time_zone.as_deref(),
                        now_ms,
                    );
                    Self::emit_spend_guard_automation_changes(&mut state, &resumed, "enabled");
                }
                next.snoozed_until_ms =
                    Some(now_ms.saturating_add(AUTOMATION_SPEND_GUARD_SNOOZE_MS));
            }
            AutomationSpendGuardAnswer::OptOut => {
                let before_automations = state.automations.clone();
                let resumed = spend_guard_resume_agent_automations(
                    &mut state,
                    agent_id,
                    &current.paused_automation_ids,
                    now_ms,
                    user_time_zone.as_deref(),
                );
                if !resumed.is_empty() {
                    self.persist_automations(&state.automations)?;
                    let after_automations = state.automations.clone();
                    push_automation_lifecycle_diff(
                        &mut state,
                        &before_automations,
                        &after_automations,
                        AutomationLifecycleSource::SpendGuard,
                        user_time_zone.as_deref(),
                        now_ms,
                    );
                    Self::emit_spend_guard_automation_changes(&mut state, &resumed, "enabled");
                }
                next.opted_out = true;
            }
            AutomationSpendGuardAnswer::Pause => {
                let mut paused_ids = current
                    .paused_automation_ids
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>();
                let before_automations = state.automations.clone();
                let newly_paused = spend_guard_disable_agent_automations(&mut state, agent_id);
                if !newly_paused.is_empty() {
                    self.persist_automations(&state.automations)?;
                    prune_pending_listener_resumes(&mut state);
                    prune_pending_automation_event_batches(&mut state);
                    let after_automations = state.automations.clone();
                    push_automation_lifecycle_diff(
                        &mut state,
                        &before_automations,
                        &after_automations,
                        AutomationLifecycleSource::SpendGuard,
                        user_time_zone.as_deref(),
                        now_ms,
                    );
                    Self::emit_spend_guard_automation_changes(
                        &mut state,
                        &newly_paused,
                        "disabled",
                    );
                }
                paused_ids.extend(newly_paused);
                next.paused_automation_ids = paused_ids.into_iter().collect();
                if !next.paused_automation_ids.is_empty() {
                    next.card_entry_ids = current.card_entry_ids.clone();
                }
            }
            AutomationSpendGuardAnswer::StayPaused => {}
        }

        state
            .automation_spend_guard
            .insert(agent_id.to_string(), next);
        let snapshot = state.automation_spend_guard.clone();
        drop(state);
        self.persist_automation_spend_guard_state(&snapshot)?;
        Ok(Some(format!(
            "<system_reminder>\nThe app asked the user about the money your routines spend while they are away. They chose to {}, and the app has ALREADY applied that itself.\nAcknowledge their choice in one short line. Do NOT edit any automation.json and do NOT ask again.\n</system_reminder>",
            answer.acknowledgement()
        )))
    }
}

#[cfg(test)]
mod automation_spend_guard_tests {
    use super::*;

    #[test]
    fn spend_guard_decision_matches_desktop_threshold_order() {
        let now = 10 * AUTOMATION_SPEND_GUARD_IDLE_TTL_MS;
        let mut guard = AutomationSpendGuardAgentState {
            last_viewed_at_ms: now - AUTOMATION_SPEND_GUARD_IDLE_TTL_MS - 1,
            unread_count: AUTOMATION_SPEND_GUARD_MIN_UNREAD_COUNT,
            ..AutomationSpendGuardAgentState::default()
        };
        assert_eq!(
            automation_spend_guard_decision(&guard, 0, now),
            AutomationSpendGuardDecision::Nudge
        );
        guard.nudged_at_ms = Some(now - AUTOMATION_SPEND_GUARD_PAUSE_DELAY_MS);
        assert_eq!(
            automation_spend_guard_decision(&guard, 0, now),
            AutomationSpendGuardDecision::Pause
        );
        guard.opted_out = true;
        assert_eq!(
            automation_spend_guard_decision(&guard, usize::MAX, now),
            AutomationSpendGuardDecision::OptedOut
        );
    }

    #[test]
    fn spend_guard_nudge_reminder_keeps_desktop_do_not_reask_contract() {
        let reminder = format!(
            "<system_reminder>\nThe user has been away while your routines kept firing; there are {} routine runs since their last view. They may be spending money on work nobody is reading.\nThe app has already asked them directly whether to keep your routines running, and applies their answer itself. Do NOT ask again yourself and do NOT edit any automation.json; just acknowledge their choice if it comes back as their reply.\nIf they neither answer nor return within three days, the app will pause ALL of this agent's routines and tell them so.\n</system_reminder>",
            20
        );
        assert!(reminder.contains("The app has already asked them directly"));
        assert!(reminder.contains("Do NOT ask again yourself"));
        assert!(reminder.contains("pause ALL of this agent's routines"));
    }

    #[test]
    fn spend_guard_store_is_bounded_and_round_trips_account_state() {
        let root = std::env::temp_dir().join(format!("spend-guard-{}", Uuid::new_v4()));
        let path = root.join("automations.spend-guard.json");
        let mut agents = BTreeMap::new();
        agents.insert(
            "mahayana-assistant".into(),
            AutomationSpendGuardAgentState {
                last_activity_at_ms: 11,
                last_viewed_at_ms: 7,
                unread_count: 18,
                nudged_at_ms: Some(9),
                snoozed_until_ms: None,
                opted_out: false,
                card_entry_ids: vec!["card-1".into()],
                paused_automation_ids: vec!["routine-1".into()],
            },
        );
        persist_automation_spend_guard(Some(&path), &agents).expect("persist guard");
        assert_eq!(load_automation_spend_guard(&path), agents);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn spend_guard_fires_since_viewed_counts_only_owned_recent_runs() {
        let mut state = FeatureState::default();
        state.automations.insert(
            "owned".into(),
            AutomationSummary {
                id: "owned".into(),
                agent_id: Some("mahayana-assistant".into()),
                name: "owned".into(),
                prompt: "review".into(),
                schedule: "@daily".into(),
                trigger: Some(AutomationTrigger::Schedule { schedule: "@daily".into() }),
                enabled: true,
                created_at_ms: 1,
                raised_notices: Vec::new(),
                runs: vec![
                    AutomationRunSummary {
                        id: "old".into(),
                        started_at: 5,
                        status: AutomationRunStatus::Ok,
                        detail: None,
                        event: None,
                        coalesced_run_ids: Vec::new(),
                    },
                    AutomationRunSummary {
                        id: "new".into(),
                        started_at: 12,
                        status: AutomationRunStatus::Ok,
                        detail: None,
                        event: None,
                        coalesced_run_ids: Vec::new(),
                    },
                ],
                last_run_at_ms: Some(12),
                next_run_at_ms: None,
            },
        );
        assert_eq!(
            spend_guard_fires_since_viewed(&state, "mahayana-assistant", 10),
            1
        );
    }

    #[test]
    fn spend_guard_pause_and_resume_only_mutate_guard_owned_routines() {
        let mut state = FeatureState::default();
        let make = |id: &str, enabled: bool| AutomationSummary {
            id: id.into(),
            agent_id: Some("mahayana-assistant".into()),
            name: id.into(),
            prompt: "review".into(),
            schedule: "@daily".into(),
            trigger: Some(AutomationTrigger::Schedule { schedule: "@daily".into() }),
            enabled,
            created_at_ms: 1,
            raised_notices: Vec::new(),
            runs: Vec::new(),
            last_run_at_ms: None,
            next_run_at_ms: None,
        };
        state.automations.insert("guard".into(), make("guard", true));
        state.automations.insert("already-off".into(), make("already-off", false));
        let paused =
            spend_guard_disable_agent_automations(&mut state, "mahayana-assistant");
        assert_eq!(paused, vec!["guard".to_string()]);
        assert!(!state.automations["guard"].enabled);
        assert!(!state.automations["already-off"].enabled);
        let resumed = spend_guard_resume_agent_automations(
            &mut state,
            "mahayana-assistant",
            &paused,
            3,
            Some("UTC"),
        );
        assert_eq!(resumed, vec!["guard".to_string()]);
        assert!(state.automations["guard"].enabled);
        assert!(!state.automations["already-off"].enabled);
        assert!(state.events.is_empty(), "mutation helpers must not emit before durable commit");
    }
}
