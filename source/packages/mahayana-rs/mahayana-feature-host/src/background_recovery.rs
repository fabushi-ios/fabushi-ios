const BACKGROUND_RECOVERY_JOURNAL_MAX_BYTES: u64 = 1024 * 1024;
const BACKGROUND_RECOVERY_MAX_ACTIVE: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BackgroundRecoveryPhase {
    Dispatching,
    Running,
    Suspended,
    RecoveryRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackgroundRecoveryExecution {
    operation_id: String,
    conversation_id: String,
    account_key: Option<String>,
    epoch: u64,
    agent_id: String,
    agent_name: String,
    source: String,
    teach_artifact: Option<String>,
    phase: BackgroundRecoveryPhase,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackgroundRecoveryJournal {
    version: u32,
    account_key: Option<String>,
    executions: Vec<BackgroundRecoveryExecution>,
}

fn background_recovery_owner_matches(
    state: &FeatureState,
    execution: &BackgroundRecoveryExecution,
) -> bool {
    execution.epoch == state.routine_epoch
        && state.bots.get(&execution.agent_id).is_some_and(|bot| {
            bot.conversation_id.as_deref() == Some(execution.conversation_id.as_str())
        })
}

impl FeatureHostController {
    fn background_recovery_journal_path(&self) -> Option<PathBuf> {
        self.active_account_root(self.automation_path.as_deref())
            .map(|path| path.with_extension("background-executions.json"))
    }

    fn persist_background_recoveries(&self, state: &FeatureState) -> Result<(), FeatureHostError> {
        let Some(path) = self.background_recovery_journal_path() else { return Ok(()); };
        let journal = BackgroundRecoveryJournal {
            version: 1,
            account_key: self.routine_account_key()?,
            executions: state.background_recoveries.values().cloned().collect(),
        };
        let bytes = serde_json::to_vec(&journal)
            .map_err(|error| FeatureHostError::Contract(format!("encode background recovery journal: {error}")))?;
        if bytes.len() as u64 > BACKGROUND_RECOVERY_JOURNAL_MAX_BYTES {
            return Err(FeatureHostError::Contract("background recovery journal exceeds safety bound".into()));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| FeatureHostError::Contract(format!("create background recovery directory: {error}")))?;
        }
        let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4().simple()));
        let write = (|| -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, &path)?;
            #[cfg(unix)]
            if let Some(parent) = path.parent() {
                std::fs::File::open(parent)?.sync_all()?;
            }
            Ok(())
        })();
        if write.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        write.map_err(|error| FeatureHostError::Contract(format!("commit background recovery journal: {error}")))
    }

    fn restore_background_recoveries(&self) -> Result<(), FeatureHostError> {
        let Some(path) = self.background_recovery_journal_path() else { return Ok(()); };
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(FeatureHostError::Contract(format!("read background recovery journal: {error}"))),
        };
        let mut bytes = Vec::new();
        file.take(BACKGROUND_RECOVERY_JOURNAL_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| FeatureHostError::Contract(format!("read bounded background recovery journal: {error}")))?;
        if bytes.len() as u64 > BACKGROUND_RECOVERY_JOURNAL_MAX_BYTES {
            return Err(FeatureHostError::Contract("background recovery journal exceeds safety bound".into()));
        }
        let journal: BackgroundRecoveryJournal = serde_json::from_slice(&bytes)
            .map_err(|error| FeatureHostError::Contract(format!("decode background recovery journal: {error}")))?;
        let account_key = self.routine_account_key()?;
        if journal.version != 1 || journal.account_key != account_key {
            return Err(FeatureHostError::Contract("background recovery journal version/account mismatch".into()));
        }
        if journal.executions.len() > BACKGROUND_RECOVERY_MAX_ACTIVE {
            return Err(FeatureHostError::Contract("too many saved background recoveries".into()));
        }
        let mut state = self.state()?;
        let mut restored = BTreeMap::new();
        for mut execution in journal.executions {
            if execution.operation_id.trim().is_empty()
                || execution.account_key != account_key
                || restored.contains_key(&execution.operation_id)
            {
                return Err(FeatureHostError::Contract("invalid or duplicate background recovery identity".into()));
            }
            execution.epoch = state.routine_epoch;
            if !background_recovery_owner_matches(&state, &execution) {
                continue;
            }
            if matches!(execution.phase, BackgroundRecoveryPhase::Dispatching | BackgroundRecoveryPhase::Running) {
                execution.phase = if self.config.mode == HostMode::Production {
                    BackgroundRecoveryPhase::Suspended
                } else {
                    BackgroundRecoveryPhase::RecoveryRequired
                };
            }
            if execution.phase == BackgroundRecoveryPhase::Suspended {
                state.background_operations.insert(
                    execution.operation_id.clone(),
                    BackgroundOperationContext {
                        agent_id: execution.agent_id.clone(),
                        agent_name: execution.agent_name.clone(),
                        source: execution.source.clone(),
                        teach_artifact: execution.teach_artifact.clone(),
                    },
                );
                state.operations.insert(execution.operation_id.clone());
                state.operation_agents.insert(execution.operation_id.clone(), execution.agent_id.clone());
            }
            restored.insert(execution.operation_id.clone(), execution);
        }
        state.background_recoveries = restored;
        self.persist_background_recoveries(&state)
    }

    fn resume_suspended_background_operations(&self) -> Result<(), FeatureHostError> {
        let pending = self.state()?.background_recoveries.values()
            .filter(|execution| execution.phase == BackgroundRecoveryPhase::Suspended)
            .cloned()
            .collect::<Vec<_>>();
        for execution in pending {
            if !background_recovery_owner_matches(&self.state()?, &execution) {
                continue;
            }
            if self.config.mode == HostMode::Production {
                #[cfg(feature = "production")]
                {
                    let session = self.runtime()?.product_execute("mahayana.auth.session.restore", &json!({}))?;
                    let auth = auth_payload(&session);
                    let actual = auth_account_id(auth).as_deref().map(account_fingerprint);
                    if auth.get("loggedIn").and_then(Value::as_bool) != Some(true)
                        || actual.is_none()
                        || actual != execution.account_key
                    {
                        return Err(FeatureHostError::Contract("background recovery session was revoked or replaced".into()));
                    }
                    self.runtime()?.resume_operation(mahayana_conversation::ResumeConversationOperationRequest {
                        conversation_id: ConversationId(execution.conversation_id.clone()),
                        operation_id: OperationId(execution.operation_id.clone()),
                        hidden: true,
                        show_assistant_output: false,
                        reply_to_message_id: None,
                        is_fork: false,
                        attachment_batch_id: None,
                    })?;
                }
                #[cfg(not(feature = "production"))]
                return Err(FeatureHostError::ProductionUnavailable);
            } else {
                continue;
            }
            let mut state = self.state()?;
            let current = state.background_recoveries.get_mut(&execution.operation_id)
                .ok_or_else(|| FeatureHostError::Contract("background recovery disappeared before resume".into()))?;
            if current.phase == BackgroundRecoveryPhase::Suspended {
                current.phase = BackgroundRecoveryPhase::Running;
                state.background_operations.insert(
                    execution.operation_id.clone(),
                    BackgroundOperationContext {
                        agent_id: execution.agent_id.clone(),
                        agent_name: execution.agent_name.clone(),
                        source: execution.source.clone(),
                        teach_artifact: execution.teach_artifact.clone(),
                    },
                );
                state.operations.insert(execution.operation_id.clone());
                state.operation_agents.insert(execution.operation_id.clone(), execution.agent_id.clone());
                self.persist_background_recoveries(&state)?;
            }
        }
        Ok(())
    }

    fn settle_background_recovery(&self, operation_id: &str) -> Result<(), FeatureHostError> {
        let mut state = self.state()?;
        if state.background_recoveries.remove(operation_id).is_some() {
            self.persist_background_recoveries(&state)?;
        }
        Ok(())
    }
}
