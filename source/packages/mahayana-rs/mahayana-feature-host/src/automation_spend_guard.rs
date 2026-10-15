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
}
