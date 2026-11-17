use crate::e2ee::keys::{
    b64_decode_32, b64_decode_vec, b64_encode, random_32, x25519_dh, x25519_pub_from_secret,
    E2eeError,
};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use gpui_telegram_proto::e2ee::RatchetHeaderDto;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::{HashMap, HashSet, VecDeque};

type HmacSha256 = Hmac<Sha256>;

const RATCHET_RK_INFO: &[u8] = b"gpui-e2ee-rk-v1";
pub const MAX_SKIP: u32 = 256;
pub const MAX_SKIPPED_STORAGE: usize = 2000;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SkippedEntrySerde {
    dh_pub: [u8; 32],
    msg_num: u32,
    mk: [u8; 32],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ReplayEntrySerde {
    dh_pub: [u8; 32],
    msg_num: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DoubleRatchetWire {
    dh_self_sec: [u8; 32],
    dh_self_pub: [u8; 32],
    dh_remote: Option<[u8; 32]>,
    root_key: [u8; 32],
    send_chain_key: Option<[u8; 32]>,
    recv_chain_key: Option<[u8; 32]>,
    send_n: u32,
    recv_n: u32,
    prev_send_n: u32,
    skipped: Vec<SkippedEntrySerde>,
    replay: Vec<ReplayEntrySerde>,
    ad: Vec<u8>,
    pending_x3dh_ek_pub: Option<[u8; 32]>,
    pending_x3dh_spk_id: Option<u32>,
    pending_x3dh_otpk_id: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct DoubleRatchetSession {
    pub dh_self_sec: [u8; 32],
    pub dh_self_pub: [u8; 32],
    pub dh_remote: Option<[u8; 32]>,
    pub root_key: [u8; 32],
    pub send_chain_key: Option<[u8; 32]>,
    pub recv_chain_key: Option<[u8; 32]>,
    pub send_n: u32,
    pub recv_n: u32,
    pub prev_send_n: u32,
    skipped_keys: HashMap<([u8; 32], u32), [u8; 32]>,
    skipped_order: VecDeque<([u8; 32], u32)>,
    seen_replay: HashSet<([u8; 32], u32)>,
    pub ad: Vec<u8>,
    pub pending_x3dh_ek_pub: Option<[u8; 32]>,
    pub pending_x3dh_spk_id: Option<u32>,
    pub pending_x3dh_otpk_id: Option<u32>,
}

impl DoubleRatchetSession {
    pub fn to_json(&self) -> Result<String, E2eeError> {
        let skipped = self
            .skipped_order
            .iter()
            .filter_map(|k| {
                self.skipped_keys.get(k).map(|mk| SkippedEntrySerde {
                    dh_pub: k.0,
                    msg_num: k.1,
                    mk: *mk,
                })
            })
            .collect();
        let replay = self
            .seen_replay
            .iter()
            .map(|&(dh_pub, msg_num)| ReplayEntrySerde { dh_pub, msg_num })
            .collect();
        let wire = DoubleRatchetWire {
            dh_self_sec: self.dh_self_sec,
            dh_self_pub: self.dh_self_pub,
            dh_remote: self.dh_remote,
            root_key: self.root_key,
            send_chain_key: self.send_chain_key,
            recv_chain_key: self.recv_chain_key,
            send_n: self.send_n,
            recv_n: self.recv_n,
            prev_send_n: self.prev_send_n,
            skipped,
            replay,
            ad: self.ad.clone(),
            pending_x3dh_ek_pub: self.pending_x3dh_ek_pub,
            pending_x3dh_spk_id: self.pending_x3dh_spk_id,
            pending_x3dh_otpk_id: self.pending_x3dh_otpk_id,
        };
        serde_json::to_string(&wire).map_err(|e| E2eeError::Serialization(e.to_string()))
    }

    pub fn from_json(json: &str) -> Result<Self, E2eeError> {
        let wire: DoubleRatchetWire =
            serde_json::from_str(json).map_err(|e| E2eeError::Serialization(e.to_string()))?;
        let mut skipped_keys = HashMap::with_capacity(wire.skipped.len());
        let mut skipped_order = VecDeque::with_capacity(wire.skipped.len());
        for entry in wire.skipped {
            let k = (entry.dh_pub, entry.msg_num);
            skipped_keys.insert(k, entry.mk);
            skipped_order.push_back(k);
        }
        let mut seen_replay = HashSet::with_capacity(wire.replay.len());
        for entry in wire.replay {
            seen_replay.insert((entry.dh_pub, entry.msg_num));
        }
        Ok(Self {
            dh_self_sec: wire.dh_self_sec,
            dh_self_pub: wire.dh_self_pub,
            dh_remote: wire.dh_remote,
            root_key: wire.root_key,
            send_chain_key: wire.send_chain_key,
            recv_chain_key: wire.recv_chain_key,
            send_n: wire.send_n,
            recv_n: wire.recv_n,
            prev_send_n: wire.prev_send_n,
            skipped_keys,
            skipped_order,
            seen_replay,
            ad: wire.ad,
            pending_x3dh_ek_pub: wire.pending_x3dh_ek_pub,
            pending_x3dh_spk_id: wire.pending_x3dh_spk_id,
            pending_x3dh_otpk_id: wire.pending_x3dh_otpk_id,
        })
    }

    pub fn init_alice(
        shared_secret: [u8; 32],
        bob_dh_pub: [u8; 32],
        ad: Vec<u8>,
    ) -> Result<Self, E2eeError> {
        let dh_self_sec = random_32()?;
        let dh_self_pub = x25519_pub_from_secret(&dh_self_sec);
        let dh_out = x25519_dh(&dh_self_sec, &bob_dh_pub)?;
        let (root_key, send_ck) = kdf_rk(&shared_secret, &dh_out)?;
        Ok(Self {
            dh_self_sec,
            dh_self_pub,
            dh_remote: Some(bob_dh_pub),
            root_key,
            send_chain_key: Some(send_ck),
            recv_chain_key: None,
            send_n: 0,
            recv_n: 0,
            prev_send_n: 0,
            skipped_keys: HashMap::new(),
            skipped_order: VecDeque::new(),
            seen_replay: HashSet::new(),
            ad,
            pending_x3dh_ek_pub: None,
            pending_x3dh_spk_id: None,
            pending_x3dh_otpk_id: None,
        })
    }

    pub fn init_bob(
        shared_secret: [u8; 32],
        bob_dh_sec: [u8; 32],
        ad: Vec<u8>,
    ) -> Self {
        let bob_dh_pub = x25519_pub_from_secret(&bob_dh_sec);
        Self {
            dh_self_sec: bob_dh_sec,
            dh_self_pub: bob_dh_pub,
            dh_remote: None,
            root_key: shared_secret,
            send_chain_key: None,
            recv_chain_key: None,
            send_n: 0,
            recv_n: 0,
            prev_send_n: 0,
            skipped_keys: HashMap::new(),
            skipped_order: VecDeque::new(),
            seen_replay: HashSet::new(),
            ad,
            pending_x3dh_ek_pub: None,
            pending_x3dh_spk_id: None,
            pending_x3dh_otpk_id: None,
        }
    }

    pub fn set_pending_x3dh_prekey_info(
        &mut self,
        ek_pub: [u8; 32],
        spk_id: u32,
        otpk_id: Option<u32>,
    ) {
        self.pending_x3dh_ek_pub = Some(ek_pub);
        self.pending_x3dh_spk_id = Some(spk_id);
        self.pending_x3dh_otpk_id = otpk_id;
    }

    pub fn clear_pending_x3dh_prekey_info(&mut self) {
        self.pending_x3dh_ek_pub = None;
        self.pending_x3dh_spk_id = None;
        self.pending_x3dh_otpk_id = None;
    }

    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<(RatchetHeaderDto, String), E2eeError> {
        let Some(ck_s) = self.send_chain_key else {
            return Err(E2eeError::SessionNotFound(
                "Send chain key not initialized yet".into(),
            ));
        };
        let (mk, next_ck_s) = kdf_ck(&ck_s)?;
        let header = RatchetHeaderDto {
            dh_pub: b64_encode(&self.dh_self_pub),
            prev_chain_len: self.prev_send_n,
            msg_num: self.send_n,
        };
        let aad = build_header_aad(&self.ad, &self.dh_self_pub, header.prev_chain_len, header.msg_num);
        let nonce = msg_nonce(header.msg_num);
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&mk));
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| E2eeError::DecryptionFailed("AEAD encryption error".into()))?;

        self.send_chain_key = Some(next_ck_s);
        self.send_n = self.send_n.saturating_add(1);
        Ok((header, b64_encode(&ciphertext)))
    }

    pub fn decrypt(
        &mut self,
        header: &RatchetHeaderDto,
        ciphertext_b64: &str,
    ) -> Result<Vec<u8>, E2eeError> {
        let remote_dh_pub = b64_decode_32(&header.dh_pub)?;
        let ciphertext = b64_decode_vec(ciphertext_b64)?;
        let replay_key = (remote_dh_pub, header.msg_num);
        if self.seen_replay.contains(&replay_key) {
            return Err(E2eeError::ReplayDetected);
        }

        let aad = build_header_aad(&self.ad, &remote_dh_pub, header.prev_chain_len, header.msg_num);
        let nonce = msg_nonce(header.msg_num);

        // Operate on a staged copy so failed AEAD verification never mutates ratchet state.
        let mut staged = self.clone();

        if let Some(mk) = staged.skipped_keys.remove(&replay_key) {
            let plaintext = aead_decrypt(&mk, &nonce, &ciphertext, &aad)?;
            staged.seen_replay.insert(replay_key);
            *self = staged;
            return Ok(plaintext);
        }

        if staged.dh_remote != Some(remote_dh_pub) {
            staged.skip_message_keys(header.prev_chain_len)?;
            staged.dh_ratchet(remote_dh_pub)?;
            staged.clear_pending_x3dh_prekey_info();
        }

        staged.skip_message_keys(header.msg_num)?;
        let ck_r = staged.recv_chain_key.ok_or_else(|| {
            E2eeError::DecryptionFailed("Missing receive chain key after DH ratchet".into())
        })?;
        let (mk, next_ck_r) = kdf_ck(&ck_r)?;
        staged.recv_chain_key = Some(next_ck_r);
        staged.recv_n = staged.recv_n.saturating_add(1);

        let plaintext = aead_decrypt(&mk, &nonce, &ciphertext, &aad)?;
        staged.seen_replay.insert(replay_key);
        *self = staged;
        Ok(plaintext)
    }

    fn skip_message_keys(&mut self, until: u32) -> Result<(), E2eeError> {
        if self.recv_n.saturating_add(MAX_SKIP) < until {
            return Err(E2eeError::OutOfOrderLimit);
        }
        if let Some(mut ck_r) = self.recv_chain_key {
            let remote = self.dh_remote.ok_or_else(|| {
                E2eeError::DecryptionFailed("Missing dh_remote while skipping keys".into())
            })?;
            while self.recv_n < until {
                let (mk, next_ck_r) = kdf_ck(&ck_r)?;
                ck_r = next_ck_r;
                let key = (remote, self.recv_n);
                if !self.skipped_keys.contains_key(&key) {
                    if self.skipped_order.len() >= MAX_SKIPPED_STORAGE {
                        if let Some(oldest) = self.skipped_order.pop_front() {
                            self.skipped_keys.remove(&oldest);
                        }
                    }
                    self.skipped_keys.insert(key, mk);
                    self.skipped_order.push_back(key);
                }
                self.recv_n = self.recv_n.saturating_add(1);
            }
            self.recv_chain_key = Some(ck_r);
        }
        Ok(())
    }

    fn dh_ratchet(&mut self, remote_dh_pub: [u8; 32]) -> Result<(), E2eeError> {
        self.prev_send_n = self.send_n;
        self.send_n = 0;
        self.recv_n = 0;
        self.dh_remote = Some(remote_dh_pub);

        let dh_recv = x25519_dh(&self.dh_self_sec, &remote_dh_pub)?;
        let (rk1, ck_r) = kdf_rk(&self.root_key, &dh_recv)?;
        self.root_key = rk1;
        self.recv_chain_key = Some(ck_r);

        let next_self_sec = random_32()?;
        let next_self_pub = x25519_pub_from_secret(&next_self_sec);
        let dh_send = x25519_dh(&next_self_sec, &remote_dh_pub)?;
        let (rk2, ck_s) = kdf_rk(&self.root_key, &dh_send)?;
        self.dh_self_sec = next_self_sec;
        self.dh_self_pub = next_self_pub;
        self.root_key = rk2;
        self.send_chain_key = Some(ck_s);
        Ok(())
    }
}

fn kdf_rk(rk: &[u8; 32], dh_out: &[u8; 32]) -> Result<([u8; 32], [u8; 32]), E2eeError> {
    let mut out = [0u8; 64];
    Hkdf::<Sha256>::new(Some(rk), dh_out)
        .expand(RATCHET_RK_INFO, &mut out)
        .map_err(|_| E2eeError::X3dhFailed("Ratchet HKDF expand failed".into()))?;
    let mut next_rk = [0u8; 32];
    let mut next_ck = [0u8; 32];
    next_rk.copy_from_slice(&out[..32]);
    next_ck.copy_from_slice(&out[32..]);
    Ok((next_rk, next_ck))
}

fn kdf_ck(ck: &[u8; 32]) -> Result<([u8; 32], [u8; 32]), E2eeError> {
    let mut mac_mk = <HmacSha256 as Mac>::new_from_slice(ck)
        .map_err(|_| E2eeError::InvalidKey("HMAC key error".into()))?;
    mac_mk.update(&[0x01]);
    let mk_bytes = mac_mk.finalize().into_bytes();

    let mut mac_ck = <HmacSha256 as Mac>::new_from_slice(ck)
        .map_err(|_| E2eeError::InvalidKey("HMAC key error".into()))?;
    mac_ck.update(&[0x02]);
    let ck_bytes = mac_ck.finalize().into_bytes();

    let mut mk = [0u8; 32];
    let mut next_ck = [0u8; 32];
    mk.copy_from_slice(&mk_bytes);
    next_ck.copy_from_slice(&ck_bytes);
    Ok((mk, next_ck))
}

fn msg_nonce(msg_num: u32) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[8..12].copy_from_slice(&msg_num.to_be_bytes());
    nonce
}

fn build_header_aad(ad: &[u8], dh_pub: &[u8; 32], prev_chain_len: u32, msg_num: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(ad.len() + 40);
    out.extend_from_slice(ad);
    out.extend_from_slice(dh_pub);
    out.extend_from_slice(&prev_chain_len.to_be_bytes());
    out.extend_from_slice(&msg_num.to_be_bytes());
    out
}

fn aead_decrypt(
    mk: &[u8; 32],
    nonce: &[u8; 12],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, E2eeError> {
    if ciphertext.len() < 16 {
        return Err(E2eeError::DecryptionFailed(
            "Ciphertext shorter than AEAD authentication tag".into(),
        ));
    }
    let cipher = ChaCha20Poly1305::new(Key::from_slice(mk));
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| E2eeError::DecryptionFailed("AEAD tag verification failed".into()))
}
