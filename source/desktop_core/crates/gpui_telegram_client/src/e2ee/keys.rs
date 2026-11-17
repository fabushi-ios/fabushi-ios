use base64::{engine::general_purpose::STANDARD as BASE64_STD, Engine as _};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use thiserror::Error;
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const SPK_SIGN_DOMAIN: &[u8] = b"gpui-e2ee-spk-v1";

#[derive(Error, Debug)]
pub enum E2eeError {
    #[error("Invalid E2EE key material: {0}")]
    InvalidKey(String),
    #[error("Signed prekey or message Ed25519 signature verification failed: {0}")]
    SignatureVerificationFailed(String),
    #[error("X3DH key agreement failed: {0}")]
    X3dhFailed(String),
    #[error("AEAD decryption failed: {0}")]
    DecryptionFailed(String),
    #[error("Replay or duplicate message detected")]
    ReplayDetected,
    #[error("E2EE session not found for target device: {0}")]
    SessionNotFound(String),
    #[error("Skipped message key limit exceeded")]
    OutOfOrderLimit,
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("E2EE storage error: {0}")]
    Storage(String),
    #[error("E2EE network error: {0}")]
    Network(String),
}

pub fn random_32() -> Result<[u8; 32], E2eeError> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|e| E2eeError::InvalidKey(format!("CSPRNG failed: {e}")))?;
    Ok(bytes)
}

pub fn b64_encode(bytes: &[u8]) -> String {
    BASE64_STD.encode(bytes)
}

pub fn b64_decode_vec(s: &str) -> Result<Vec<u8>, E2eeError> {
    BASE64_STD
        .decode(s.trim())
        .map_err(|e| E2eeError::InvalidKey(format!("Invalid base64: {e}")))
}

pub fn b64_decode_32(s: &str) -> Result<[u8; 32], E2eeError> {
    let bytes = b64_decode_vec(s)?;
    if bytes.len() != 32 {
        return Err(E2eeError::InvalidKey(format!(
            "Expected 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

pub fn b64_decode_64(s: &str) -> Result<[u8; 64], E2eeError> {
    let bytes = b64_decode_vec(s)?;
    if bytes.len() != 64 {
        return Err(E2eeError::InvalidKey(format!(
            "Expected 64 bytes, got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; 64];
    out.copy_from_slice(&bytes);
    Ok(out)
}

pub fn x25519_pub_from_secret(sec: &[u8; 32]) -> [u8; 32] {
    let static_sec = StaticSecret::from(*sec);
    X25519PublicKey::from(&static_sec).to_bytes()
}

pub fn x25519_dh(sec: &[u8; 32], pub_bytes: &[u8; 32]) -> Result<[u8; 32], E2eeError> {
    let s = StaticSecret::from(*sec);
    let p = X25519PublicKey::from(*pub_bytes);
    let shared = s.diffie_hellman(&p).to_bytes();
    if shared.iter().all(|&b| b == 0) {
        return Err(E2eeError::X3dhFailed(
            "Low-order X25519 point produced all-zero shared secret".into(),
        ));
    }
    Ok(shared)
}

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct DeviceIdentityKeys {
    pub x25519_sec: [u8; 32],
    #[zeroize(skip)]
    pub x25519_pub: [u8; 32],
    pub ed25519_sec: [u8; 32],
    #[zeroize(skip)]
    pub ed25519_pub: [u8; 32],
}

impl std::fmt::Debug for DeviceIdentityKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceIdentityKeys")
            .field("x25519_pub", &b64_encode(&self.x25519_pub))
            .field("ed25519_pub", &b64_encode(&self.ed25519_pub))
            .finish()
    }
}

impl DeviceIdentityKeys {
    pub fn generate() -> Result<Self, E2eeError> {
        let x25519_sec = random_32()?;
        let ed25519_sec = random_32()?;
        Ok(Self::from_secrets(x25519_sec, ed25519_sec))
    }

    pub fn from_secrets(x25519_sec: [u8; 32], ed25519_sec: [u8; 32]) -> Self {
        let x25519_pub = x25519_pub_from_secret(&x25519_sec);
        let signing = SigningKey::from_bytes(&ed25519_sec);
        let ed25519_pub = signing.verifying_key().to_bytes();
        Self {
            x25519_sec,
            x25519_pub,
            ed25519_sec,
            ed25519_pub,
        }
    }

    pub fn sign_signed_prekey(&self, spk_pub: &[u8; 32]) -> [u8; 64] {
        let signing = SigningKey::from_bytes(&self.ed25519_sec);
        let mut msg = Vec::with_capacity(SPK_SIGN_DOMAIN.len() + 32);
        msg.extend_from_slice(SPK_SIGN_DOMAIN);
        msg.extend_from_slice(spk_pub);
        signing.sign(&msg).to_bytes()
    }
}

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct LocalSignedPreKey {
    pub key_id: u32,
    pub secret: [u8; 32],
    #[zeroize(skip)]
    pub public: [u8; 32],
    #[zeroize(skip)]
    pub signature: [u8; 64],
    pub created_at_ms: i64,
}

impl LocalSignedPreKey {
    pub fn generate(key_id: u32, identity: &DeviceIdentityKeys, created_at_ms: i64) -> Result<Self, E2eeError> {
        let secret = random_32()?;
        let public = x25519_pub_from_secret(&secret);
        let signature = identity.sign_signed_prekey(&public);
        Ok(Self {
            key_id,
            secret,
            public,
            signature,
            created_at_ms,
        })
    }
}

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct LocalOneTimePreKey {
    pub key_id: u32,
    pub secret: [u8; 32],
    #[zeroize(skip)]
    pub public: [u8; 32],
    pub consumed: bool,
    pub created_at_ms: i64,
}

impl LocalOneTimePreKey {
    pub fn generate(key_id: u32, created_at_ms: i64) -> Result<Self, E2eeError> {
        let secret = random_32()?;
        let public = x25519_pub_from_secret(&secret);
        Ok(Self {
            key_id,
            secret,
            public,
            consumed: false,
            created_at_ms,
        })
    }
}

pub fn verify_signed_prekey(
    signing_pub: &[u8; 32],
    spk_pub: &[u8; 32],
    sig: &[u8; 64],
) -> Result<(), E2eeError> {
    let verifying = VerifyingKey::from_bytes(signing_pub)
        .map_err(|e| E2eeError::InvalidKey(format!("Invalid Ed25519 verifying key: {e}")))?;
    let signature = Signature::from_bytes(sig);
    let mut msg = Vec::with_capacity(SPK_SIGN_DOMAIN.len() + 32);
    msg.extend_from_slice(SPK_SIGN_DOMAIN);
    msg.extend_from_slice(spk_pub);
    verifying
        .verify(&msg, &signature)
        .map_err(|e| E2eeError::SignatureVerificationFailed(e.to_string()))
}

const SAFETY_EMOJIS: [&str; 32] = [
    "🔐", "🛡️", "🗝️", "🦊", "🐋", "🦉", "🐢", "🐬", "🦅", "🐝", "🌵", "🌻", "🍄", "🌊", "🔥", "❄️",
    "⚡", "🌙", "⭐", "💎", "🧭", "⚓", "🎯", "🧩", "🎻", "🚀", "🛸", "🏔️", "🌋", "🍀", "🍁", "🪷",
];

pub fn compute_safety_number_emojis(local_ik: &[u8; 32], remote_ik: &[u8; 32]) -> String {
    let (a, b) = if local_ik <= remote_ik {
        (local_ik, remote_ik)
    } else {
        (remote_ik, local_ik)
    };
    let mut hasher = Sha256::new();
    hasher.update(b"gpui-e2ee-safety-v1");
    hasher.update(a);
    hasher.update(b);
    let digest = hasher.finalize();
    let mut out = String::new();
    for idx in 0..4 {
        let emoji = SAFETY_EMOJIS[(digest[idx] as usize) % SAFETY_EMOJIS.len()];
        out.push_str(emoji);
    }
    out
}
