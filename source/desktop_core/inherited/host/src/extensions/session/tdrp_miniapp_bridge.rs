//! Telegram / TDRP MiniApp Bridge, Package Verification, Launch Sessions, Grants, and Protocol Handling.

use super::*;

pub type MiniAppPackageVerifier = Arc<
    dyn Fn(&str, Option<&str>, Option<&str>) -> Result<(), MiniAppPostError> + Send + Sync,
>;

#[derive(Debug, Clone)]
pub struct HostMiniAppLaunchSession {
    pub session: fabushi_messaging_core::MiniAppSession,
    pub grant: fabushi_messaging_core::MiniAppGrant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MiniAppPostError {
    SessionNotFound(String),
    SessionExpired { expired_at_ms: i64, now_ms: i64 },
    GrantMismatch,
    GrantExpired { expired_at_ms: i64, now_ms: i64 },
    PermissionDenied(fabushi_messaging_core::MiniAppPermission),
    MissingConversationBinding,
    ConversationMismatch { expected: String, actual: String },
    UserConfirmationRequired,
    NoBotRecipient,
    InvalidRequestId,
    NonceTooLong,
    NotParticipant,
    ConversationNotFound(String),
    AppNotInstalled(String),
    PackageDigestMismatch(String),
    StaleScope(String),
    Conflict(String),
    PersistenceFailed(String),
}

impl std::fmt::Display for MiniAppPostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionNotFound(id) => write!(f, "MiniApp launch session not found: {id}"),
            Self::SessionExpired { expired_at_ms, now_ms } => {
                write!(f, "MiniApp launch session expired at {expired_at_ms} (now {now_ms})")
            }
            Self::GrantMismatch => write!(f, "MiniApp grant mismatch"),
            Self::GrantExpired { expired_at_ms, now_ms } => {
                write!(f, "MiniApp grant expired at {expired_at_ms} (now {now_ms})")
            }
            Self::PermissionDenied(perm) => write!(f, "MiniApp permission denied: {perm:?}"),
            Self::MissingConversationBinding => write!(f, "MiniApp session is not bound to a conversation"),
            Self::ConversationMismatch { expected, actual } => {
                write!(f, "MiniApp conversation mismatch: session bound to {expected}, requested {actual}")
            }
            Self::UserConfirmationRequired => write!(f, "MiniApp SendMessage requires explicit user confirmation"),
            Self::NoBotRecipient => write!(f, "MiniApp SendData requires a bot recipient participant in the conversation"),
            Self::InvalidRequestId => write!(f, "MiniApp request requires a non-empty requestId"),
            Self::NonceTooLong => write!(f, "MiniApp clientNonce exceeds 200 UTF-8 bytes"),
            Self::NotParticipant => write!(f, "Authenticated local human is not a participant in the conversation"),
            Self::ConversationNotFound(id) => write!(f, "Conversation not found: {id}"),
            Self::AppNotInstalled(id) => write!(f, "MiniApp package is not installed or verified: {id}"),
            Self::PackageDigestMismatch(msg) => write!(f, "MiniApp package digest or version mismatch: {msg}"),
            Self::StaleScope(msg) => write!(f, "MiniApp account scope or generation is stale or invalid: {msg}"),
            Self::Conflict(msg) => write!(f, "MiniApp message conflict: {msg}"),
            Self::PersistenceFailed(err) => write!(f, "MiniApp message persistence failed: {err}"),
        }
    }
}

impl std::error::Error for MiniAppPostError {}

impl ProductionSessionWorkers {
    pub fn set_mini_app_package_verifier(&self, verifier: MiniAppPackageVerifier) {
        if let Ok(mut guard) = self.mini_app_package_verifier.lock() {
            *guard = Some(verifier);
        }
    }

    pub fn create_ledger_package_verifier(
        db: Arc<Mutex<rusqlite::Connection>>,
        scope_provider: Arc<dyn Fn() -> Result<crate::extensions::mcp::installable_package::ledger::HostInstallScope, String> + Send + Sync>,
    ) -> MiniAppPackageVerifier {
        Arc::new(move |app_id: &str, version: Option<&str>, digest: Option<&str>| {
            let conn = db
                .lock()
                .map_err(|_| MiniAppPostError::PersistenceFailed("db lock poisoned".into()))?;
            let scope = scope_provider().map_err(MiniAppPostError::StaleScope)?;
            if scope.account_scope.is_empty() || scope.generation == 0 {
                return Err(MiniAppPostError::StaleScope("invalid account scope or zero generation".into()));
            }
            use rusqlite::OptionalExtension;
            let current_account: Option<(String, i64)> = conn
                .query_row(
                    "SELECT account_scope_hash, generation FROM host_miniapp_native_account WHERE singleton=1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(|e| MiniAppPostError::PersistenceFailed(format!("account query error: {e:?}")))?;
            if let Some((active_scope, active_gen)) = current_account {
                if active_scope != scope.account_scope || active_gen as u64 != scope.generation {
                    return Err(MiniAppPostError::StaleScope(format!(
                        "account generation mismatch: scope generation {}, active account generation {}",
                        scope.generation, active_gen
                    )));
                }
            }
            let installed = crate::extensions::mcp::installable_package::ledger::installed(&conn, &scope, app_id)
                .map_err(|e| MiniAppPostError::PersistenceFailed(format!("ledger query error: {e:?}")))?;
            let Some(record) = installed else {
                return Err(MiniAppPostError::AppNotInstalled(app_id.to_string()));
            };
            if let Some(reason) = crate::extensions::mcp::installable_package::ledger::quarantine_reason(&conn, &scope, app_id)
                .map_err(|e| MiniAppPostError::PersistenceFailed(format!("quarantine query error: {e:?}")))?
            {
                return Err(MiniAppPostError::AppNotInstalled(format!("{app_id} (quarantined: {reason})")));
            }
            if let Some(expected_ver) = version {
                if record.version != expected_ver {
                    return Err(MiniAppPostError::PackageDigestMismatch(format!(
                        "version mismatch: expected {expected_ver}, found {}", record.version
                    )));
                }
            }
            if let Some(expected_digest) = digest {
                if record.digest != expected_digest {
                    return Err(MiniAppPostError::PackageDigestMismatch(format!(
                        "digest mismatch: expected {expected_digest}, found {}", record.digest
                    )));
                }
            }
            Ok(())
        })
    }

    pub fn issue_miniapp_launch_session(
        &self,
        app_id: &str,
        conversation_id: &str,
        start_parameter: Option<&str>,
        version: Option<&str>,
        package_digest: Option<&str>,
    ) -> Result<serde_json::Value, MiniAppPostError> {
        let app_id = app_id.trim();
        if app_id.is_empty() {
            return Err(MiniAppPostError::InvalidRequestId);
        }

        // 1. Verify installed and reverified package against existing owner
        {
            let verifier_guard = self.mini_app_package_verifier.lock().map_err(|_| {
                MiniAppPostError::PersistenceFailed("package verifier lock poisoned".into())
            })?;
            if let Some(verifier) = verifier_guard.as_ref() {
                verifier(app_id, version, package_digest)?;
            } else {
                return Err(MiniAppPostError::AppNotInstalled(app_id.to_string()));
            }
        }

        let local_human = self
            .local_human_id()
            .map_err(|e| MiniAppPostError::PersistenceFailed(e))?;

        // 2. Verify participant in conversation
        let owner = self
            .open_human_conversation_db_owner(conversation_id)
            .map_err(|_| MiniAppPostError::ConversationNotFound(conversation_id.to_string()))?;
        let metadata = owner
            .read_metadata()
            .map_err(|e| MiniAppPostError::PersistenceFailed(e.to_string()))?;
        let is_participant = metadata
            .get("participantIds")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(local_human)));
        if !is_participant {
            return Err(MiniAppPostError::NotParticipant);
        }

        let now_ms = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()) as i64;

        // 3. Read explicit user grant; if none exists, effective permissions are empty
        let grant_opt = {
            let grants = self.miniapp_grants.lock().map_err(|_| {
                MiniAppPostError::PersistenceFailed("miniapp_grants lock poisoned".into())
            })?;
            grants.get(&(app_id.to_string(), local_human.to_string())).cloned()
        };

        let grant = grant_opt.unwrap_or_else(|| fabushi_messaging_core::MiniAppGrant {
            mini_app_id: app_id.to_string(),
            actor_id: fabushi_messaging_core::ActorId::new(local_human),
            permissions: vec![], // Empty permissions if no explicit user grant!
            granted_at_ms: now_ms,
            expires_at_ms: None,
        });

        // 4. Issue authoritative launch session
        let launch_session_id = format!("msess_{}", uuid::Uuid::new_v4().simple());
        let expires_at_ms = now_ms + 86_400_000; // 24h
        let granted_permissions = grant.permissions.clone();

        let session = fabushi_messaging_core::MiniAppSession {
            id: launch_session_id.clone(),
            mini_app_id: app_id.to_string(),
            actor_id: fabushi_messaging_core::ActorId::new(local_human),
            conversation_id: Some(fabushi_messaging_core::ConversationId::new(conversation_id)),
            start_parameter: start_parameter.map(|s| s.to_string()),
            granted_permissions,
            opened_at_ms: now_ms,
            expires_at_ms,
        };

        // 5. Save to Host authoritative registry
        let mut sessions = self.miniapp_launch_sessions.lock().map_err(|_| {
            MiniAppPostError::PersistenceFailed("miniapp_launch_sessions lock poisoned".into())
        })?;
        sessions.insert(
            launch_session_id.clone(),
            HostMiniAppLaunchSession { session, grant },
        );

        Ok(serde_json::json!({
            "launchSessionId": launch_session_id,
            "appId": app_id,
            "conversationId": conversation_id,
            "startParameter": start_parameter,
            "openedAtMs": now_ms,
            "expiresAtMs": expires_at_ms,
        }))
    }

    pub fn set_miniapp_grant(
        &self,
        grant: fabushi_messaging_core::MiniAppGrant,
    ) -> Result<(), String> {
        let mut grants = self
            .miniapp_grants
            .lock()
            .map_err(|_| "miniapp_grants lock poisoned".to_string())?;
        grants.insert(
            (grant.mini_app_id.clone(), grant.actor_id.as_str().to_string()),
            grant,
        );
        Ok(())
    }

    pub fn clear_miniapp_launch_sessions_for_test(&self) -> Result<(), String> {
        let mut sessions = self
            .miniapp_launch_sessions
            .lock()
            .map_err(|_| "miniapp_launch_sessions lock poisoned".to_string())?;
        sessions.clear();
        Ok(())
    }

    pub fn register_miniapp_launch_session_for_test(
        &self,
        session: fabushi_messaging_core::MiniAppSession,
        grant: fabushi_messaging_core::MiniAppGrant,
    ) -> Result<(), String> {
        let mut sessions = self
            .miniapp_launch_sessions
            .lock()
            .map_err(|_| "miniapp_launch_sessions lock poisoned".to_string())?;
        sessions.insert(session.id.clone(), HostMiniAppLaunchSession { session, grant });
        Ok(())
    }

    pub fn get_miniapp_service_call(
        &self,
        call_id: &str,
    ) -> Option<fabushi_messaging_core::MiniAppServiceCallSession> {
        self.miniapp_service_calls
            .lock()
            .ok()
            .and_then(|calls| calls.get(call_id).cloned())
    }

    pub fn post_miniapp_message(
        &self,
        launch_session_id: &str,
        request_id: &str,
        request: &fabushi_messaging_core::MiniAppRequest,
    ) -> Result<serde_json::Value, MiniAppPostError> {
        let launch_session_id = launch_session_id.trim();
        if launch_session_id.is_empty() {
            return Err(MiniAppPostError::SessionNotFound(launch_session_id.to_string()));
        }
        let request_id = request_id.trim();
        if request_id.is_empty() {
            return Err(MiniAppPostError::InvalidRequestId);
        }

        // 1. Look up Host-held launch session
        let launch_session = {
            let sessions = self.miniapp_launch_sessions.lock().map_err(|_| {
                MiniAppPostError::PersistenceFailed("miniapp_launch_sessions lock poisoned".into())
            })?;
            sessions
                .get(launch_session_id)
                .cloned()
                .ok_or_else(|| MiniAppPostError::SessionNotFound(launch_session_id.to_string()))?
        };

        // 2. Host clock
        let now_ms = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()) as i64;

        // 3. Pure authorization in core
        let action = fabushi_messaging_core::authorize_miniapp_request(
            &launch_session.session,
            &launch_session.grant,
            request,
            now_ms,
        )
        .map_err(|denial| match denial {
            fabushi_messaging_core::MiniAppDenial::SessionExpired { expired_at_ms, now_ms } => {
                MiniAppPostError::SessionExpired { expired_at_ms, now_ms }
            }
            fabushi_messaging_core::MiniAppDenial::GrantExpired { expired_at_ms, now_ms } => {
                MiniAppPostError::GrantExpired { expired_at_ms, now_ms }
            }
            fabushi_messaging_core::MiniAppDenial::GrantMismatch => MiniAppPostError::GrantMismatch,
            fabushi_messaging_core::MiniAppDenial::PermissionDenied { permission } => {
                MiniAppPostError::PermissionDenied(permission)
            }
            fabushi_messaging_core::MiniAppDenial::MissingConversationBinding => {
                MiniAppPostError::MissingConversationBinding
            }
        })?;

        // 4. Policy enforcement and state transition
        let (conversation_id, content) = match request {
            fabushi_messaging_core::MiniAppRequest::SendMessage { .. } => {
                // Point 1: Without Host-mediated user confirmation UI, SendMessage ALWAYS fails closed
                return Err(MiniAppPostError::UserConfirmationRequired);
            }
            fabushi_messaging_core::MiniAppRequest::SendData { .. } => {
                let conv_id = match action {
                    fabushi_messaging_core::MiniAppAction::SendData { conversation_id, .. } => conversation_id,
                    _ => return Err(MiniAppPostError::PersistenceFailed("unexpected action for SendData".into())),
                };
                let owner = self
                    .open_human_conversation_db_owner(conv_id.as_str())
                    .map_err(|_| MiniAppPostError::ConversationNotFound(conv_id.as_str().to_string()))?;
                let metadata = owner
                    .read_metadata()
                    .map_err(|e| MiniAppPostError::PersistenceFailed(e.to_string()))?;
                let has_bot_participant = metadata
                    .get("participantKinds")
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|kinds| kinds.values().any(|k| k.as_str() == Some("bot")));
                if !has_bot_participant {
                    return Err(MiniAppPostError::NoBotRecipient);
                }
                (conv_id, "[MiniApp Data Payload]".to_string())
            }
            fabushi_messaging_core::MiniAppRequest::OpenInvoice { invoice_id } => {
                let conv_id = launch_session
                    .session
                    .conversation_id
                    .clone()
                    .ok_or(MiniAppPostError::MissingConversationBinding)?;
                self.open_human_conversation_db_owner(conv_id.as_str())
                    .map_err(|_| MiniAppPostError::ConversationNotFound(conv_id.as_str().to_string()))?;
                let content = format!("[MiniApp Invoice Opened] invoice_id={invoice_id}");
                (conv_id, content)
            }
            fabushi_messaging_core::MiniAppRequest::StartServiceCall { call_id, mode } => {
                let conv_id = launch_session
                    .session
                    .conversation_id
                    .clone()
                    .ok_or(MiniAppPostError::MissingConversationBinding)?;
                self.open_human_conversation_db_owner(conv_id.as_str())
                    .map_err(|_| MiniAppPostError::ConversationNotFound(conv_id.as_str().to_string()))?;
                {
                    let mut calls = self.miniapp_service_calls.lock().map_err(|_| {
                        MiniAppPostError::PersistenceFailed("miniapp_service_calls lock poisoned".into())
                    })?;
                    if let Some(existing) = calls.get_mut(&call_id.0) {
                        if existing.state == fabushi_messaging_core::MiniAppServiceCallState::Connecting
                            || existing.state == fabushi_messaging_core::MiniAppServiceCallState::Held
                        {
                            existing
                                .activate(now_ms)
                                .map_err(|e| MiniAppPostError::PersistenceFailed(e.to_string()))?;
                        }
                    } else {
                        let mut call_session = fabushi_messaging_core::MiniAppServiceCallSession::new(
                            call_id.clone(),
                            launch_session.session.mini_app_id.clone(),
                            Some(launch_session.session.id.clone()),
                            conv_id.clone(),
                            launch_session.session.actor_id.clone(),
                            None,
                            *mode,
                            Vec::new(),
                            Vec::new(),
                            now_ms,
                        )
                        .map_err(|e| MiniAppPostError::PersistenceFailed(e.to_string()))?;
                        call_session
                            .activate(now_ms)
                            .map_err(|e| MiniAppPostError::PersistenceFailed(e.to_string()))?;
                        calls.insert(call_id.0.clone(), call_session);
                    }
                }
                let content = format!("[MiniApp ServiceCall Started] call_id={} mode={:?}", call_id.0, mode);
                (conv_id, content)
            }
            fabushi_messaging_core::MiniAppRequest::SubmitServiceCallInput { call_id, input } => {
                let conv_id = launch_session
                    .session
                    .conversation_id
                    .clone()
                    .ok_or(MiniAppPostError::MissingConversationBinding)?;
                self.open_human_conversation_db_owner(conv_id.as_str())
                    .map_err(|_| MiniAppPostError::ConversationNotFound(conv_id.as_str().to_string()))?;
                {
                    let mut calls = self.miniapp_service_calls.lock().map_err(|_| {
                        MiniAppPostError::PersistenceFailed("miniapp_service_calls lock poisoned".into())
                    })?;
                    let call_session = calls.get_mut(&call_id.0).ok_or_else(|| {
                        MiniAppPostError::PersistenceFailed(format!("MiniApp service call not found: {}", call_id.0))
                    })?;
                    call_session
                        .submit_input(launch_session.session.actor_id.clone(), input.clone(), now_ms)
                        .map_err(|e| MiniAppPostError::PersistenceFailed(e.to_string()))?;
                }
                let content = match input {
                    fabushi_messaging_core::MiniAppServiceCallInput::Dtmf { digits } => {
                        format!("[MiniApp ServiceCall DTMF] call_id={} digits={}", call_id.0, digits)
                    }
                    fabushi_messaging_core::MiniAppServiceCallInput::SpeechTranscript { text, is_final, .. } => {
                        format!("[MiniApp ServiceCall Speech] call_id={} text={} (final={})", call_id.0, text, is_final)
                    }
                    fabushi_messaging_core::MiniAppServiceCallInput::ChatText { text } => {
                        format!("[MiniApp ServiceCall Chat] call_id={} text={}", call_id.0, text)
                    }
                };
                (conv_id, content)
            }
            fabushi_messaging_core::MiniAppRequest::EndServiceCall { call_id } => {
                let conv_id = launch_session
                    .session
                    .conversation_id
                    .clone()
                    .ok_or(MiniAppPostError::MissingConversationBinding)?;
                self.open_human_conversation_db_owner(conv_id.as_str())
                    .map_err(|_| MiniAppPostError::ConversationNotFound(conv_id.as_str().to_string()))?;
                {
                    let mut calls = self.miniapp_service_calls.lock().map_err(|_| {
                        MiniAppPostError::PersistenceFailed("miniapp_service_calls lock poisoned".into())
                    })?;
                    let call_session = calls.get_mut(&call_id.0).ok_or_else(|| {
                        MiniAppPostError::PersistenceFailed(format!("MiniApp service call not found: {}", call_id.0))
                    })?;
                    if call_session.state != fabushi_messaging_core::MiniAppServiceCallState::Ended {
                        call_session
                            .end(now_ms)
                            .map_err(|e| MiniAppPostError::PersistenceFailed(e.to_string()))?;
                    }
                }
                let content = format!("[MiniApp ServiceCall Ended] call_id={}", call_id.0);
                (conv_id, content)
            }
            _ => {
                return Err(MiniAppPostError::PersistenceFailed(
                    "MiniApp action does not post to a conversation".into(),
                ));
            }
        };

        // 5. Deterministic CAS nonce
        let client_nonce = format!("miniapp:{}:{}", launch_session_id, request_id);
        if client_nonce.len() > 200 {
            return Err(MiniAppPostError::NonceTooLong);
        }

        // 6. Append to conversation: forward_context is None (Point 3)
        let mut entry = self
            .append_human_message_with_context(
                conversation_id.as_str(),
                &content,
                &client_nonce,
                Some(now_ms as f64),
                None,
                &[],
                None, // Point 3: DO NOT use forwardContext for miniapp!
                None,
                false,
            )
            .map_err(|err| {
                if err.contains("already identifies different content") {
                    MiniAppPostError::Conflict(err)
                } else {
                    MiniAppPostError::PersistenceFailed(err)
                }
            })?;
        if let Some(object) = entry.as_object_mut() {
            if !object.contains_key("conversationId") {
                object.insert(
                    "conversationId".to_string(),
                    serde_json::Value::String(conversation_id.as_str().to_string()),
                );
            }
        }
        Ok(entry)
    }

}
