use crate::e2ee::double_ratchet::DoubleRatchetSession;
use crate::e2ee::keys::{
    b64_decode_32, b64_encode, random_32, x25519_pub_from_secret, DeviceIdentityKeys, E2eeError,
};
use crate::e2ee::sender_keys::SenderKeyState;
use crate::e2ee::store::E2eeStore;
use crate::e2ee::x3dh::{x3dh_initiator, x3dh_responder};
use crate::net::{HttpClient, HttpClientError};
use crate::outbox::TokenProvider;
use gpui_telegram_proto::e2ee::{
    DevicePreKeyBundle, EncryptedEnvelope, EncryptedPayloadKind, PerDeviceCiphertext,
    PublishPreKeysResponse, SenderKeyDistributionMessage,
};
use gpui_telegram_proto::message::{MessageContent, MessageItem};
use std::collections::HashSet;
use std::sync::Arc;

pub type DeviceIdProvider = Arc<dyn Fn() -> String + Send + Sync>;
pub type UserIdProvider = Arc<dyn Fn() -> Option<u64> + Send + Sync>;

#[derive(Clone)]
pub struct E2eeManager {
    store: E2eeStore,
    http_client: HttpClient,
    token_provider: TokenProvider,
    device_id_provider: DeviceIdProvider,
    user_id_provider: UserIdProvider,
}

impl std::fmt::Debug for E2eeManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("E2eeManager")
            .field("store", &self.store)
            .finish()
    }
}

impl E2eeManager {
    pub fn new(
        store: E2eeStore,
        http_client: HttpClient,
        token_provider: TokenProvider,
        device_id_provider: DeviceIdProvider,
        user_id_provider: UserIdProvider,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            http_client,
            token_provider,
            device_id_provider,
            user_id_provider,
        })
    }

    pub fn store(&self) -> &E2eeStore {
        &self.store
    }

    pub fn identity(&self) -> Result<DeviceIdentityKeys, E2eeError> {
        self.store.get_or_create_identity()
    }

    pub async fn init_and_publish_prekeys(
        &self,
        otpk_batch_size: usize,
    ) -> Result<PublishPreKeysResponse, E2eeError> {
        let token = (self.token_provider)()
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| E2eeError::Network("Missing session token for E2EE prekeys".into()))?;
        let device_id = (self.device_id_provider)();
        let req = self
            .store
            .build_publish_request(&device_id, Some("GPUI Desktop".into()), otpk_batch_size)?;
        self.http_client
            .publish_e2ee_prekeys(&token, &req)
            .await
            .map_err(|e| E2eeError::Network(e.to_string()))
    }

    pub async fn replenish_prekeys_if_low(
        &self,
        min_threshold: usize,
        target_batch: usize,
    ) -> Result<Option<PublishPreKeysResponse>, E2eeError> {
        let token = (self.token_provider)()
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| E2eeError::Network("Missing session token".into()))?;
        let device_id = (self.device_id_provider)();
        let count = self
            .http_client
            .get_e2ee_prekey_count(&token, Some(&device_id))
            .await
            .map_err(|e| E2eeError::Network(e.to_string()))?;
        if !count.has_signed_prekey || count.remaining_one_time_prekeys < min_threshold {
            let resp = self.init_and_publish_prekeys(target_batch).await?;
            return Ok(Some(resp));
        }
        Ok(None)
    }

    pub fn establish_session_from_bundle(
        &self,
        bundle: &DevicePreKeyBundle,
    ) -> Result<DoubleRatchetSession, E2eeError> {
        let identity = self.store.get_or_create_identity()?;
        let ek_sec = random_32()?;
        let ek_pub = x25519_pub_from_secret(&ek_sec);
        let x3dh_out = x3dh_initiator(&identity.x25519_sec, &ek_sec, bundle)?;
        let spk_pub = b64_decode_32(&bundle.signed_prekey.public_key)?;
        let mut session =
            DoubleRatchetSession::init_alice(*x3dh_out.shared_secret, spk_pub, x3dh_out.associated_data)?;
        session.set_pending_x3dh_prekey_info(
            ek_pub,
            bundle.signed_prekey.key_id,
            bundle.one_time_prekey.as_ref().map(|k| k.key_id),
        );
        self.store
            .save_session(bundle.user_id, &bundle.device_id, &session)?;
        Ok(session)
    }

    async fn fetch_target_bundles(
        &self,
        user_ids: &[u64],
    ) -> Result<Vec<DevicePreKeyBundle>, E2eeError> {
        let token = (self.token_provider)()
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| E2eeError::Network("Missing session token".into()))?;
        let self_uid = (self.user_id_provider)().unwrap_or(0);
        let self_dev = (self.device_id_provider)();

        let mut seen_uid = HashSet::new();
        let mut all_bundles = Vec::new();
        for &uid in user_ids {
            if uid == 0 || !seen_uid.insert(uid) {
                continue;
            }
            match self
                .http_client
                .fetch_user_prekey_bundles(&token, uid, None)
                .await
            {
                Ok(resp) => {
                    for b in resp.devices {
                        if b.user_id == self_uid && b.device_id == self_dev {
                            continue;
                        }
                        all_bundles.push(b);
                    }
                }
                Err(HttpClientError::Api { status: 404, .. }) if uid == self_uid => {
                    // Self user may have no other E2EE devices yet; ignore 404 for self
                }
                Err(e) => {
                    return Err(E2eeError::Network(format!(
                        "Failed to fetch prekeys for user {uid}: {e}"
                    )));
                }
            }
        }
        Ok(all_bundles)
    }

    async fn ensure_pairwise_targets_for_users(
        &self,
        user_ids: &[u64],
    ) -> Result<Vec<(u64, String)>, E2eeError> {
        let self_uid = (self.user_id_provider)().unwrap_or(0);
        let self_dev = (self.device_id_provider)();

        // Always fetch active devices/bundles from server for users lacking sessions or to discover newly added devices
        let bundles = self.fetch_target_bundles(user_ids).await?;
        let mut targets = Vec::new();
        let mut seen = HashSet::new();
        for b in bundles {
            if b.user_id == self_uid && b.device_id == self_dev {
                continue;
            }
            if self.store.get_session(b.user_id, &b.device_id)?.is_none() {
                self.establish_session_from_bundle(&b)?;
            }
            if seen.insert((b.user_id, b.device_id.clone())) {
                targets.push((b.user_id, b.device_id));
            }
        }
        // Also include any locally cached sessions for those users
        for &uid in user_ids {
            for (dev, _) in self.store.list_peer_sessions(uid)? {
                if uid == self_uid && dev == self_dev {
                    continue;
                }
                if seen.insert((uid, dev.clone())) {
                    targets.push((uid, dev));
                }
            }
        }
        Ok(targets)
    }

    pub fn encrypt_pairwise_for_targets(
        &self,
        targets: &[(u64, String)],
        plaintext: &[u8],
        is_sender_key_dist: bool,
    ) -> Result<EncryptedEnvelope, E2eeError> {
        let self_uid = (self.user_id_provider)().unwrap_or(0);
        let self_dev = (self.device_id_provider)();
        let identity = self.store.get_or_create_identity()?;

        let mut recipients = Vec::with_capacity(targets.len());
        for (uid, dev_id) in targets {
            if *uid == self_uid && *dev_id == self_dev {
                continue;
            }
            let mut session = self
                .store
                .get_session(*uid, dev_id)?
                .ok_or_else(|| E2eeError::SessionNotFound(format!("{uid}:{dev_id}")))?;
            let (header, ciphertext) = session.encrypt(plaintext)?;
            let ek = session.pending_x3dh_ek_pub.map(|b| b64_encode(&b));
            let spk_id = session.pending_x3dh_spk_id;
            let otpk_id = session.pending_x3dh_otpk_id;
            self.store.save_session(*uid, dev_id, &session)?;

            recipients.push(PerDeviceCiphertext {
                recipient_user_id: *uid,
                recipient_device_id: dev_id.clone(),
                ephemeral_key: ek,
                used_signed_prekey_id: spk_id,
                used_one_time_prekey_id: otpk_id,
                header,
                ciphertext,
            });
        }

        if recipients.is_empty() {
            return Err(E2eeError::SessionNotFound(
                "No target recipient devices available for pairwise E2EE encryption".into(),
            ));
        }

        let kind = if is_sender_key_dist {
            EncryptedPayloadKind::SenderKeyDistribution { recipients }
        } else {
            EncryptedPayloadKind::PairwiseFanout { recipients }
        };

        Ok(EncryptedEnvelope {
            version: 1,
            sender_user_id: self_uid,
            sender_device_id: self_dev,
            sender_identity_key: b64_encode(&identity.x25519_pub),
            sender_signing_key: b64_encode(&identity.ed25519_pub),
            kind,
        })
    }

    pub async fn encrypt_dm(
        &self,
        recipient_user_id: u64,
        plaintext: &[u8],
    ) -> Result<EncryptedEnvelope, E2eeError> {
        let self_uid = (self.user_id_provider)().unwrap_or(0);
        let mut users = vec![recipient_user_id];
        if self_uid > 0 && self_uid != recipient_user_id {
            users.push(self_uid);
        }
        let targets = self.ensure_pairwise_targets_for_users(&users).await?;
        self.encrypt_pairwise_for_targets(&targets, plaintext, false)
    }

    pub fn rotate_group_sender_key(&self, chat_id: i64) -> Result<(), E2eeError> {
        let self_uid = (self.user_id_provider)().unwrap_or(0);
        let self_dev = (self.device_id_provider)();
        self.store
            .delete_own_sender_key(chat_id, self_uid, &self_dev)
    }

    pub async fn encrypt_group(
        &self,
        chat_id: i64,
        member_user_ids: &[u64],
        plaintext: &[u8],
    ) -> Result<(Option<EncryptedEnvelope>, EncryptedEnvelope), E2eeError> {
        let self_uid = (self.user_id_provider)().unwrap_or(0);
        let mut all_members = member_user_ids.to_vec();
        if self_uid > 0 && !all_members.contains(&self_uid) {
            all_members.push(self_uid);
        }
        let targets = self.ensure_pairwise_targets_for_users(&all_members).await?;
        self.encrypt_group_for_targets(chat_id, &targets, plaintext)
    }

    pub fn encrypt_group_for_targets(
        &self,
        chat_id: i64,
        targets: &[(u64, String)],
        plaintext: &[u8],
    ) -> Result<(Option<EncryptedEnvelope>, EncryptedEnvelope), E2eeError> {
        let self_uid = (self.user_id_provider)().unwrap_or(0);
        let self_dev = (self.device_id_provider)();
        let identity = self.store.get_or_create_identity()?;

        let mut own_sk = match self.store.get_sender_key(chat_id, self_uid, &self_dev)? {
            Some(sk) => sk,
            None => {
                let (sk, _) = SenderKeyState::create_own(chat_id, self_uid, self_dev.clone())?;
                self.store.save_sender_key(&sk)?;
                sk
            }
        };

        let mut need_dist = Vec::new();
        for (uid, dev) in targets {
            if *uid == self_uid && *dev == self_dev {
                continue;
            }
            if !self
                .store
                .is_sender_key_distributed_to(chat_id, own_sk.chain_id, *uid, dev)?
            {
                need_dist.push((*uid, dev.clone()));
            }
        }

        let dist_envelope = if !need_dist.is_empty() {
            let dist_msg = own_sk.distribution_message();
            let dist_bytes = serde_json::to_vec(&dist_msg)
                .map_err(|e| E2eeError::Serialization(e.to_string()))?;
            let env = self.encrypt_pairwise_for_targets(&need_dist, &dist_bytes, true)?;
            for (uid, dev) in &need_dist {
                self.store.mark_sender_key_distributed_to(
                    chat_id,
                    own_sk.chain_id,
                    *uid,
                    dev,
                )?;
            }
            Some(env)
        } else {
            None
        };

        let group_ct = own_sk.encrypt_group_message(plaintext)?;
        self.store.save_sender_key(&own_sk)?;

        let payload_env = EncryptedEnvelope {
            version: 1,
            sender_user_id: self_uid,
            sender_device_id: self_dev,
            sender_identity_key: b64_encode(&identity.x25519_pub),
            sender_signing_key: b64_encode(&identity.ed25519_pub),
            kind: EncryptedPayloadKind::GroupSenderKey(group_ct),
        };
        Ok((dist_envelope, payload_env))
    }

    pub fn decrypt_envelope(&self, envelope: &EncryptedEnvelope) -> Result<Vec<u8>, E2eeError> {
        let self_uid = (self.user_id_provider)().unwrap_or(0);
        let self_dev = (self.device_id_provider)();

        match &envelope.kind {
            EncryptedPayloadKind::PairwiseFanout { recipients } => {
                let entry = Self::find_matching_recipient(recipients, self_uid, &self_dev)
                    .ok_or_else(|| {
                        E2eeError::SessionNotFound(format!(
                            "No PerDeviceCiphertext addressed to {self_uid}:{self_dev}"
                        ))
                    })?;
                self.decrypt_pairwise_entry(envelope, entry)
            }
            EncryptedPayloadKind::SenderKeyDistribution { recipients } => {
                let entry = Self::find_matching_recipient(recipients, self_uid, &self_dev)
                    .ok_or_else(|| {
                        E2eeError::SessionNotFound(format!(
                            "No SenderKeyDistribution addressed to {self_uid}:{self_dev}"
                        ))
                    })?;
                let bytes = self.decrypt_pairwise_entry(envelope, entry)?;
                let dist: SenderKeyDistributionMessage = serde_json::from_slice(&bytes)
                    .map_err(|e| E2eeError::Serialization(e.to_string()))?;
                if dist.sender_user_id != envelope.sender_user_id
                    || dist.sender_device_id != envelope.sender_device_id
                {
                    return Err(E2eeError::InvalidKey(format!(
                        "SenderKeyDistribution sender {}:{} does not match envelope sender {}:{}",
                        dist.sender_user_id,
                        dist.sender_device_id,
                        envelope.sender_user_id,
                        envelope.sender_device_id
                    )));
                }
                let peer_sk = SenderKeyState::from_distribution(&dist)?;
                self.store.save_sender_key(&peer_sk)?;
                Ok(Vec::new())
            }
            EncryptedPayloadKind::GroupSenderKey(group_ct) => {
                let mut sk = self
                    .store
                    .get_sender_key(
                        group_ct.chat_id,
                        envelope.sender_user_id,
                        &envelope.sender_device_id,
                    )?
                    .ok_or_else(|| {
                        E2eeError::SessionNotFound(format!(
                            "Missing SenderKey for chat {} sender {}:{}",
                            group_ct.chat_id, envelope.sender_user_id, envelope.sender_device_id
                        ))
                    })?;
                let plaintext = sk.decrypt_group_message(group_ct)?;
                self.store.save_sender_key(&sk)?;
                Ok(plaintext)
            }
        }
    }

    fn find_matching_recipient<'a>(
        recipients: &'a [PerDeviceCiphertext],
        self_uid: u64,
        self_dev: &str,
    ) -> Option<&'a PerDeviceCiphertext> {
        recipients.iter().find(|r| {
            r.recipient_device_id == self_dev
                && (self_uid == 0 || r.recipient_user_id == 0 || r.recipient_user_id == self_uid)
        })
    }

    fn decrypt_pairwise_entry(
        &self,
        envelope: &EncryptedEnvelope,
        entry: &PerDeviceCiphertext,
    ) -> Result<Vec<u8>, E2eeError> {
        let sender_uid = envelope.sender_user_id;
        let sender_dev = &envelope.sender_device_id;

        if let Some(mut session) = self.store.get_session(sender_uid, sender_dev)? {
            let pt = session.decrypt(&entry.header, &entry.ciphertext)?;
            self.store.save_session(sender_uid, sender_dev, &session)?;
            return Ok(pt);
        }

        let ek_b64 = entry.ephemeral_key.as_deref().ok_or_else(|| {
            E2eeError::SessionNotFound(format!(
                "No session for {sender_uid}:{sender_dev} and message lacks X3DH ephemeral_key"
            ))
        })?;
        let ek_a_pub = b64_decode_32(ek_b64)?;
        let ik_a_pub = b64_decode_32(&envelope.sender_identity_key)?;
        let identity = self.store.get_or_create_identity()?;

        let spk = if let Some(spk_id) = entry.used_signed_prekey_id {
            self.store
                .get_signed_prekey(spk_id)?
                .ok_or_else(|| E2eeError::InvalidKey(format!("Unknown SPK id {spk_id}")))?
        } else {
            self.store
                .latest_signed_prekey()?
                .ok_or_else(|| E2eeError::InvalidKey("No local SPK available".into()))?
        };

        let otpk = if let Some(otpk_id) = entry.used_one_time_prekey_id {
            self.store.get_one_time_prekey(otpk_id)?
        } else {
            None
        };

        let x3dh_out = x3dh_responder(
            &identity.x25519_sec,
            &spk.secret,
            otpk.as_ref().map(|k| &k.secret),
            &ik_a_pub,
            &ek_a_pub,
        )?;
        let mut session =
            DoubleRatchetSession::init_bob(*x3dh_out.shared_secret, spk.secret, x3dh_out.associated_data);
        let plaintext = session.decrypt(&entry.header, &entry.ciphertext)?;

        // Commit session and consume OTPK only after AEAD authentication succeeds.
        if let Some(otpk_id) = entry.used_one_time_prekey_id {
            let _ = self.store.consume_one_time_prekey(otpk_id);
        }
        self.store.save_session(sender_uid, sender_dev, &session)?;
        Ok(plaintext)
    }

    pub fn try_decrypt_message_item(&self, item: &MessageItem) -> Option<String> {
        let MessageContent::Encrypted(ref env) = item.content else {
            return None;
        };
        if env.sender_user_id != item.sender_id.0 {
            return None;
        }
        let bytes = self.decrypt_envelope(env).ok()?;
        String::from_utf8(bytes).ok()
    }
}
