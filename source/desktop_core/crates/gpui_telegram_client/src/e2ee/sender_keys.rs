use crate::e2ee::keys::{
    b64_decode_32, b64_decode_64, b64_decode_vec, b64_encode, random_32, E2eeError,
};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use gpui_telegram_proto::e2ee::{GroupCiphertext, SenderKeyDistributionMessage};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::{HashMap, HashSet, VecDeque};

type HmacSha256 = Hmac<Sha256>;

const SENDER_KEY_SIGN_DOMAIN: &[u8] = b"gpui-e2ee-sk-v1";
const MAX_GROUP_SKIP: u32 = 256;
const MAX_GROUP_SKIPPED_STORAGE: usize = 2000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SenderKeyState {
    pub chat_id: i64,
    pub sender_user_id: u64,
    pub sender_device_id: String,
    pub chain_id: u32,
    pub iteration: u32,
    pub chain_key: [u8; 32],
    pub signing_key_sec: Option<[u8; 32]>,
    pub signing_key_pub: [u8; 32],
    pub skipped_keys: HashMap<u32, [u8; 32]>,
    pub skipped_order: VecDeque<u32>,
    pub seen_iterations: HashSet<u32>,
}

impl SenderKeyState {
    pub fn to_json(&self) -> Result<String, E2eeError> {
        serde_json::to_string(self).map_err(|e| E2eeError::Serialization(e.to_string()))
    }

    pub fn from_json(json: &str) -> Result<Self, E2eeError> {
        serde_json::from_str(json).map_err(|e| E2eeError::Serialization(e.to_string()))
    }

    pub fn create_own(
        chat_id: i64,
        sender_user_id: u64,
        sender_device_id: String,
    ) -> Result<(Self, SenderKeyDistributionMessage), E2eeError> {
        let mut chain_id_bytes = [0u8; 4];
        getrandom::getrandom(&mut chain_id_bytes)
            .map_err(|e| E2eeError::InvalidKey(format!("CSPRNG failed: {e}")))?;
        let chain_id = u32::from_le_bytes(chain_id_bytes).max(1);
        let chain_key = random_32()?;
        let signing_sec = random_32()?;
        let signing_pub = SigningKey::from_bytes(&signing_sec)
            .verifying_key()
            .to_bytes();

        let state = Self {
            chat_id,
            sender_user_id,
            sender_device_id: sender_device_id.clone(),
            chain_id,
            iteration: 0,
            chain_key,
            signing_key_sec: Some(signing_sec),
            signing_key_pub: signing_pub,
            skipped_keys: HashMap::new(),
            skipped_order: VecDeque::new(),
            seen_iterations: HashSet::new(),
        };
        let dist = state.distribution_message();
        Ok((state, dist))
    }

    pub fn distribution_message(&self) -> SenderKeyDistributionMessage {
        SenderKeyDistributionMessage {
            chat_id: self.chat_id,
            sender_user_id: self.sender_user_id,
            sender_device_id: self.sender_device_id.clone(),
            chain_id: self.chain_id,
            iteration: self.iteration,
            chain_key: b64_encode(&self.chain_key),
            signing_public_key: b64_encode(&self.signing_key_pub),
        }
    }

    pub fn from_distribution(msg: &SenderKeyDistributionMessage) -> Result<Self, E2eeError> {
        let chain_key = b64_decode_32(&msg.chain_key)?;
        let signing_key_pub = b64_decode_32(&msg.signing_public_key)?;
        let _ = VerifyingKey::from_bytes(&signing_key_pub)
            .map_err(|e| E2eeError::InvalidKey(format!("Invalid Ed25519 sender key: {e}")))?;
        Ok(Self {
            chat_id: msg.chat_id,
            sender_user_id: msg.sender_user_id,
            sender_device_id: msg.sender_device_id.clone(),
            chain_id: msg.chain_id,
            iteration: msg.iteration,
            chain_key,
            signing_key_sec: None,
            signing_key_pub,
            skipped_keys: HashMap::new(),
            skipped_order: VecDeque::new(),
            seen_iterations: HashSet::new(),
        })
    }

    pub fn encrypt_group_message(&mut self, plaintext: &[u8]) -> Result<GroupCiphertext, E2eeError> {
        let signing_sec = self.signing_key_sec.ok_or_else(|| {
            E2eeError::InvalidKey("Cannot encrypt with peer's receive-only SenderKeyState".into())
        })?;
        let iter = self.iteration;
        let (mk, next_ck) = derive_sender_message_key(&self.chain_key, iter)?;

        let nonce = group_nonce(iter);
        let aad = build_group_aad(
            self.chat_id,
            self.chain_id,
            iter,
            self.sender_user_id,
            &self.sender_device_id,
        );
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&mk));
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| E2eeError::DecryptionFailed("Group AEAD encryption failed".into()))?;

        let sig_input = build_group_sig_input(self.chat_id, self.chain_id, iter, &ciphertext);
        let signature = SigningKey::from_bytes(&signing_sec)
            .sign(&sig_input)
            .to_bytes();

        self.chain_key = next_ck;
        self.iteration = iter.saturating_add(1);

        Ok(GroupCiphertext {
            chat_id: self.chat_id,
            chain_id: self.chain_id,
            iteration: iter,
            ciphertext: b64_encode(&ciphertext),
            signature: b64_encode(&signature),
        })
    }

    pub fn decrypt_group_message(&mut self, msg: &GroupCiphertext) -> Result<Vec<u8>, E2eeError> {
        if msg.chat_id != self.chat_id || msg.chain_id != self.chain_id {
            return Err(E2eeError::SessionNotFound(format!(
                "SenderKey chat/chain mismatch (expected {}/{}, got {}/{})",
                self.chat_id, self.chain_id, msg.chat_id, msg.chain_id
            )));
        }

        let ciphertext = b64_decode_vec(&msg.ciphertext)?;
        if ciphertext.len() < 16 {
            return Err(E2eeError::DecryptionFailed(
                "Group ciphertext shorter than AEAD tag".into(),
            ));
        }
        let sig_bytes = b64_decode_64(&msg.signature)?;
        let verifying = VerifyingKey::from_bytes(&self.signing_key_pub)
            .map_err(|e| E2eeError::InvalidKey(e.to_string()))?;
        let sig_input = build_group_sig_input(msg.chat_id, msg.chain_id, msg.iteration, &ciphertext);
        verifying
            .verify(&sig_input, &Signature::from_bytes(&sig_bytes))
            .map_err(|e| E2eeError::SignatureVerificationFailed(e.to_string()))?;

        if self.seen_iterations.contains(&msg.iteration) {
            return Err(E2eeError::ReplayDetected);
        }

        let mut staged = self.clone();
        let aad = build_group_aad(
            staged.chat_id,
            staged.chain_id,
            msg.iteration,
            staged.sender_user_id,
            &staged.sender_device_id,
        );
        let nonce = group_nonce(msg.iteration);

        let mk = if let Some(skipped_mk) = staged.skipped_keys.remove(&msg.iteration) {
            skipped_mk
        } else {
            if msg.iteration < staged.iteration {
                return Err(E2eeError::ReplayDetected);
            }
            if msg.iteration.saturating_sub(staged.iteration) > MAX_GROUP_SKIP {
                return Err(E2eeError::OutOfOrderLimit);
            }
            while staged.iteration < msg.iteration {
                let (skip_mk, next_ck) =
                    derive_sender_message_key(&staged.chain_key, staged.iteration)?;
                if staged.skipped_order.len() >= MAX_GROUP_SKIPPED_STORAGE {
                    if let Some(oldest) = staged.skipped_order.pop_front() {
                        staged.skipped_keys.remove(&oldest);
                    }
                }
                staged.skipped_keys.insert(staged.iteration, skip_mk);
                staged.skipped_order.push_back(staged.iteration);
                staged.chain_key = next_ck;
                staged.iteration = staged.iteration.saturating_add(1);
            }
            let (curr_mk, next_ck) =
                derive_sender_message_key(&staged.chain_key, staged.iteration)?;
            staged.chain_key = next_ck;
            staged.iteration = staged.iteration.saturating_add(1);
            curr_mk
        };

        let cipher = ChaCha20Poly1305::new(Key::from_slice(&mk));
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| E2eeError::DecryptionFailed("Group AEAD tag verification failed".into()))?;

        staged.seen_iterations.insert(msg.iteration);
        *self = staged;
        Ok(plaintext)
    }
}

fn derive_sender_message_key(
    ck: &[u8; 32],
    iteration: u32,
) -> Result<([u8; 32], [u8; 32]), E2eeError> {
    let mut mac_mk = <HmacSha256 as Mac>::new_from_slice(ck)
        .map_err(|_| E2eeError::InvalidKey("HMAC error".into()))?;
    mac_mk.update(b"msg");
    mac_mk.update(&iteration.to_be_bytes());
    let mk_bytes = mac_mk.finalize().into_bytes();

    let mut mac_ck = <HmacSha256 as Mac>::new_from_slice(ck)
        .map_err(|_| E2eeError::InvalidKey("HMAC error".into()))?;
    mac_ck.update(b"chain");
    let ck_bytes = mac_ck.finalize().into_bytes();

    let mut mk = [0u8; 32];
    let mut next_ck = [0u8; 32];
    mk.copy_from_slice(&mk_bytes);
    next_ck.copy_from_slice(&ck_bytes);
    Ok((mk, next_ck))
}

fn group_nonce(iteration: u32) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[8..12].copy_from_slice(&iteration.to_be_bytes());
    nonce
}

fn build_group_aad(
    chat_id: i64,
    chain_id: u32,
    iteration: u32,
    sender_user_id: u64,
    sender_device_id: &str,
) -> Vec<u8> {
    let mut aad = Vec::with_capacity(32 + sender_device_id.len());
    aad.extend_from_slice(&chat_id.to_be_bytes());
    aad.extend_from_slice(&chain_id.to_be_bytes());
    aad.extend_from_slice(&iteration.to_be_bytes());
    aad.extend_from_slice(&sender_user_id.to_be_bytes());
    aad.extend_from_slice(sender_device_id.as_bytes());
    aad
}

fn build_group_sig_input(
    chat_id: i64,
    chain_id: u32,
    iteration: u32,
    ciphertext: &[u8],
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(SENDER_KEY_SIGN_DOMAIN.len() + 16 + ciphertext.len());
    buf.extend_from_slice(SENDER_KEY_SIGN_DOMAIN);
    buf.extend_from_slice(&chat_id.to_be_bytes());
    buf.extend_from_slice(&chain_id.to_be_bytes());
    buf.extend_from_slice(&iteration.to_be_bytes());
    buf.extend_from_slice(ciphertext);
    buf
}
