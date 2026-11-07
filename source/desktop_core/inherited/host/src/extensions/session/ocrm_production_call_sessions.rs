use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::call_session::{CallSession, CallSignal, transition_target};
use super::native_messaging::{
    FabushiRemoteHumanCall, FabushiRemoteHumanCallEvent, fabushi_identity_text,
};
use super::production::ProductionSessionWorkers;

impl ProductionSessionWorkers {
    pub(crate) fn local_call_peer(&self, participant_ids: &[String]) -> Result<String, String> {
        let local = self
            .local_human_id
            .as_deref()
            .ok_or_else(|| "CallSession requires an authenticated Human identity".to_string())?;
        let mut participants = participant_ids
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        participants.sort_unstable();
        participants.dedup();
        if participants.len() != 2 || !participants.iter().any(|value| *value == local) {
            return Err("shipping Human calls require exactly the authenticated Human and one peer".into());
        }
        participants
            .into_iter()
            .find(|value| *value != local)
            .map(str::to_string)
            .ok_or_else(|| "shipping Human call peer is unavailable".to_string())
    }

    pub(crate) fn remote_call_peer(&self, remote: &FabushiRemoteHumanCall) -> Result<String, String> {
        let local = self
            .local_human_id
            .as_deref()
            .ok_or_else(|| "CallSession requires an authenticated Human identity".to_string())?;
        let creator = fabushi_identity_text(&remote.creator_user_id)?;
        let peer = fabushi_identity_text(&remote.peer_user_id)?;
        if creator == local && peer != local {
            Ok(peer)
        } else if peer == local && creator != local {
            Ok(creator)
        } else {
            Err("remote Human call does not belong to the authenticated Human identity".into())
        }
    }

    pub(crate) fn validate_remote_call(
        &self,
        remote: &FabushiRemoteHumanCall,
        local: &CallSession,
    ) -> Result<(), String> {
        if remote.call_id != local.id {
            return Err("remote Human call id does not match CallSession owner".into());
        }
        let peer = self.remote_call_peer(remote)?;
        if self.local_call_peer(&local.participant_ids)? != peer {
            return Err("remote Human call participants do not match CallSession owner".into());
        }
        Ok(())
    }

    fn stable_call_event_id(
        prefix: &str,
        generation: u64,
        payload: &serde_json::Value,
    ) -> Result<String, String> {
        let bytes = serde_json::to_vec(payload).map_err(|error| error.to_string())?;
        let digest = Sha256::digest(bytes);
        let suffix = digest
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Ok(format!("{prefix}:{generation}:{suffix}"))
    }

    pub(crate) fn apply_remote_call_event(
        &self,
        event: &FabushiRemoteHumanCallEvent,
    ) -> Result<(), String> {
        let current = self.authorized_call_session(&event.call_id)?;
        match event.kind.as_str() {
            "transition" => {
                let action = event
                    .payload
                    .get("action")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "remote call transition omitted action".to_string())?;
                let target_state = event
                    .payload
                    .get("state")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "remote call transition omitted state".to_string())?;
                if current.state == target_state && current.generation == event.generation {
                    return Ok(());
                }
                let terminal_reason = event
                    .payload
                    .get("terminalReason")
                    .and_then(serde_json::Value::as_str);
                let (projected_state, projected_generation, _, _) =
                    transition_target(&current, action, terminal_reason)?;
                if projected_state != target_state || projected_generation != event.generation {
                    return Err("remote call transition disagrees with canonical Host state machine".into());
                }
                self.call_sessions()?
                    .transition(&event.call_id, current.generation, action, terminal_reason)?;
            }
            "signal" => {
                let kind = event
                    .payload
                    .get("signalKind")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "remote call signal omitted signalKind".to_string())?;
                let payload = event
                    .payload
                    .get("signal")
                    .ok_or_else(|| "remote call signal omitted signal payload".to_string())?;
                self.call_sessions()?.append_remote_signal(
                    &event.call_id,
                    event.generation,
                    event.seq,
                    &event.device_id,
                    kind,
                    payload,
                )?;
            }
            "media" => {
                if current.generation != event.generation {
                    return Err("remote call media event has stale generation".into());
                }
                let capabilities = event
                    .payload
                    .get("mediaCapabilities")
                    .ok_or_else(|| "remote call media event omitted mediaCapabilities".to_string())?;
                let devices = event
                    .payload
                    .get("deviceSelection")
                    .ok_or_else(|| "remote call media event omitted deviceSelection".to_string())?;
                if current.media_capabilities == *capabilities && current.device_selection == *devices {
                    return Ok(());
                }
                self.call_sessions()?.update_media(
                    &event.call_id,
                    event.generation,
                    Some(capabilities),
                    Some(devices),
                )?;
            }
            "participant" => {
                return Err("remote call participant mutation is unsupported by the current CallSession owner".into());
            }
            _ => return Err(format!("unsupported remote call event kind: {}", event.kind)),
        }
        Ok(())
    }

    pub fn sync_call_session_from_remote(&self, call_id: &str) -> Result<CallSession, String> {
        self.sync_call_session_from_remote_with_recovery(call_id, None)
    }

    pub fn call_transport_identity(&self) -> Result<serde_json::Value, String> {
        let local_human_id = self.local_human_id()?.to_string();
        let client = self
            .shipping_native_messaging()?
            .ok_or_else(|| "shipping Human call transport requires native messaging".to_string())?;
        let identity = client.identity()?;
        if identity.user_id != local_human_id {
            return Err("Fabushi call credential identity does not match active Human identity".into());
        }
        Ok(serde_json::json!({
            "userId": identity.user_id,
            "deviceId": identity.device_id,
        }))
    }

    pub fn call_ice_servers(&self) -> Result<serde_json::Value, String> {
        let local_human_id = self.local_human_id()?.to_string();
        let client = self
            .shipping_native_messaging()?
            .ok_or_else(|| "shipping Human call media transport requires native messaging".to_string())?;
        let identity = client.identity()?;
        if identity.user_id != local_human_id {
            return Err("Fabushi call credential identity does not match active Human identity".into());
        }
        let (ice_servers, ttl_seconds) = client.get_human_call_ice_servers()?;
        Ok(serde_json::json!({
            "iceServers": ice_servers,
            "ttlSeconds": ttl_seconds,
        }))
    }

    pub fn call_transport_lease(&self, call_id: &str) -> Result<serde_json::Value, String> {
        let local_human_id = self.local_human_id()?.to_string();
        let local = self.authorized_call_session(call_id)?;
        let client = self
            .shipping_native_messaging()?
            .ok_or_else(|| "shipping Human call media transport requires native messaging".to_string())?;
        let identity = client.identity()?;
        if identity.user_id != local_human_id {
            return Err("Fabushi call credential identity does not match active Human identity".into());
        }
        let cursor = self.call_sessions()?.remote_event_seq(call_id)?;
        let (remote, _, _) = client.get_human_call(call_id, cursor, 1)?;
        self.validate_remote_call(&remote, &local)?;
        if remote.event_seq < cursor {
            return Err("Fabushi call backend event cursor regressed behind the canonical Host owner".into());
        }
        let creator = fabushi_identity_text(&remote.creator_user_id)?;
        let role = if creator == local_human_id { "creator" } else { "peer" };
        let claimed_device_id = if role == "creator" {
            remote.creator_device_id.as_deref()
        } else {
            remote.peer_device_id.as_deref()
        };
        Ok(serde_json::json!({
            "userId": local_human_id,
            "deviceId": identity.device_id,
            "role": role,
            "claimedDeviceId": claimed_device_id,
            "isOwner": claimed_device_id == Some(identity.device_id.as_str()),
            "claimAvailable": claimed_device_id.is_none(),
            "creatorDeviceId": remote.creator_device_id,
            "peerDeviceId": remote.peer_device_id,
            "state": remote.state,
            "generation": remote.generation,
            "eventSeq": remote.event_seq,
        }))
    }

    pub fn sync_human_calls(&self) -> Result<Vec<CallSession>, String> {
        self.sync_human_calls_with_recovery(None)
    }

    pub fn create_call_session(
        &self,
        scope_id: &str,
        participant_ids: &[String],
    ) -> Result<CallSession, String> {
        let creator_id = self
            .local_human_id
            .as_deref()
            .ok_or_else(|| "CallSession creation requires an authenticated Human identity".to_string())?;
        self.read_human_conversation_transcript(scope_id)?;
        let Some(client) = self.shipping_native_messaging()? else {
            return self.call_sessions()?.create(scope_id, creator_id, participant_ids);
        };
        let peer = self.local_call_peer(participant_ids)?;
        let identity = client.identity()?;
        if identity.user_id != creator_id {
            return Err("Fabushi call credential identity does not match CallSession creator".into());
        }
        let call_id = Uuid::new_v4().to_string();
        let remote = client.create_human_call(&call_id, &peer)?;
        if remote.call_id != call_id
            || fabushi_identity_text(&remote.creator_user_id)? != creator_id
            || self.remote_call_peer(&remote)? != peer
            || remote.state != "invited"
            || remote.generation != 0
            || remote.event_seq != 0
        {
            return Err("Fabushi call backend returned an invalid newly-created call".into());
        }
        self.call_sessions()?
            .create_with_id(&call_id, scope_id, creator_id, participant_ids)
    }

    pub(crate) fn authorized_call_session(&self, call_id: &str) -> Result<CallSession, String> {
        let local_human_id = self
            .local_human_id
            .as_deref()
            .ok_or_else(|| "CallSession access requires an authenticated Human identity".to_string())?;
        let call = self
            .call_sessions()?
            .get(call_id)?
            .ok_or_else(|| "call session not found".to_string())?;
        if !call.participant_ids.iter().any(|participant| participant == local_human_id) {
            return Err("call session does not belong to the authenticated Human identity".into());
        }
        Ok(call)
    }

    pub fn get_call_session(&self, call_id: &str) -> Result<Option<CallSession>, String> {
        match self.authorized_call_session(call_id) {
            Ok(call) => {
                if self.shipping_native_messaging()?.is_some() {
                    self.sync_call_session_from_remote(call_id).map(Some)
                } else {
                    Ok(Some(call))
                }
            }
            Err(error) if error == "call session not found" => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn list_call_sessions(
        &self,
        scope_id: &str,
        limit: usize,
    ) -> Result<Vec<CallSession>, String> {
        let local_human_id = self
            .local_human_id
            .as_deref()
            .ok_or_else(|| "CallSession access requires an authenticated Human identity".to_string())?;
        self.read_human_conversation_transcript(scope_id)?;
        if self.shipping_native_messaging()?.is_some() {
            self.sync_human_calls()?;
        }
        Ok(self
            .call_sessions()?
            .list_for_scope(scope_id, limit)?
            .into_iter()
            .filter(|call| call.participant_ids.iter().any(|participant| participant == local_human_id))
            .collect())
    }

    pub fn transition_call_session(
        &self,
        call_id: &str,
        expected_generation: u64,
        action: &str,
        terminal_reason: Option<&str>,
    ) -> Result<CallSession, String> {
        let current = if self.shipping_native_messaging()?.is_some() {
            self.sync_call_session_from_remote(call_id)?
        } else {
            self.authorized_call_session(call_id)?
        };
        let Some(client) = self.shipping_native_messaging()? else {
            return self.call_sessions()?
                .transition(call_id, expected_generation, action, terminal_reason);
        };
        if current.generation != expected_generation {
            return Err(format!(
                "stale call generation: expected {}, current {}",
                expected_generation, current.generation
            ));
        }
        let (next_state, next_generation, _, normalized_reason) =
            transition_target(&current, action, terminal_reason)?;
        let mut payload = serde_json::json!({
            "action": action,
            "state": next_state,
        });
        if let Some(reason) = normalized_reason {
            payload["terminalReason"] = serde_json::json!(reason);
        }
        let client_event_id = format!("transition:{expected_generation}:{action}");
        client.append_human_call_event(
            call_id,
            &client_event_id,
            next_generation,
            "transition",
            &payload,
        )?;
        self.sync_call_session_from_remote(call_id)
    }

    pub fn update_call_media(
        &self,
        call_id: &str,
        expected_generation: u64,
        media_capabilities: Option<&serde_json::Value>,
        device_selection: Option<&serde_json::Value>,
    ) -> Result<CallSession, String> {
        let current = if self.shipping_native_messaging()?.is_some() {
            self.sync_call_session_from_remote(call_id)?
        } else {
            self.authorized_call_session(call_id)?
        };
        let Some(client) = self.shipping_native_messaging()? else {
            return self.call_sessions()?.update_media(
                call_id,
                expected_generation,
                media_capabilities,
                device_selection,
            );
        };
        if current.generation != expected_generation {
            return Err(format!(
                "stale call generation: expected {}, current {}",
                expected_generation, current.generation
            ));
        }
        if matches!(current.state.as_str(), "ended" | "failed") {
            return Err("terminal call session cannot update media state".into());
        }
        if media_capabilities.is_none() && device_selection.is_none() {
            return Err("call media update requires capabilities or device selection".into());
        }
        let capabilities = media_capabilities.unwrap_or(&current.media_capabilities);
        let devices = device_selection.unwrap_or(&current.device_selection);
        if !capabilities.is_object() || !devices.is_object() {
            return Err("call media capabilities and device selection must be objects".into());
        }
        let payload = serde_json::json!({
            "mediaCapabilities": capabilities,
            "deviceSelection": devices,
        });
        let client_event_id = Self::stable_call_event_id("media", expected_generation, &payload)?;
        client.append_human_call_event(
            call_id,
            &client_event_id,
            expected_generation,
            "media",
            &payload,
        )?;
        self.sync_call_session_from_remote(call_id)
    }

    pub fn append_call_signal(
        &self,
        call_id: &str,
        expected_generation: u64,
        seq: u64,
        sender_device_id: &str,
        kind: &str,
        payload: &serde_json::Value,
    ) -> Result<CallSignal, String> {
        let current = if self.shipping_native_messaging()?.is_some() {
            self.sync_call_session_from_remote(call_id)?
        } else {
            self.authorized_call_session(call_id)?
        };
        let Some(client) = self.shipping_native_messaging()? else {
            return self.call_sessions()?.append_signal(
                call_id,
                expected_generation,
                seq,
                sender_device_id,
                kind,
                payload,
            );
        };
        if current.generation != expected_generation {
            return Err(format!(
                "stale call generation: expected {}, current {}",
                expected_generation, current.generation
            ));
        }
        if matches!(current.state.as_str(), "ended" | "failed") {
            return Err("terminal call session cannot accept signaling".into());
        }
        let identity = client.identity()?;
        if identity.device_id != sender_device_id {
            return Err("call signal senderDeviceId must match the authenticated Host device".into());
        }
        let remote_payload = serde_json::json!({
            "senderDeviceId": sender_device_id,
            "signalKind": kind,
            "signal": payload,
        });
        let id_payload = serde_json::json!({
            "deviceId": identity.device_id,
            "kind": kind,
            "signal": payload,
        });
        let client_event_id = Self::stable_call_event_id("signal", expected_generation, &id_payload)?;
        let (_, accepted_event) = client.append_human_call_event(
            call_id,
            &client_event_id,
            expected_generation,
            "signal",
            &remote_payload,
        )?;
        self.sync_call_session_from_remote(call_id)?;
        self.call_sessions()?
            .list_signals(
                call_id,
                expected_generation,
                accepted_event.seq.saturating_sub(1),
                1,
            )?
            .into_iter()
            .find(|signal| signal.seq == accepted_event.seq)
            .ok_or_else(|| "remote call signal did not materialize in canonical CallSession owner".to_string())
    }

    pub fn list_call_signals(
        &self,
        call_id: &str,
        generation: u64,
        after_seq: u64,
        limit: usize,
    ) -> Result<Vec<CallSignal>, String> {
        self.authorized_call_session(call_id)?;
        if self.shipping_native_messaging()?.is_some() {
            self.sync_call_session_from_remote(call_id)?;
        }
        self.call_sessions()?
            .list_signals(call_id, generation, after_seq, limit)
    }
}
