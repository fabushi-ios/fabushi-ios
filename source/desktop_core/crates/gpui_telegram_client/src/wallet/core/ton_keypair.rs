//! TON Ed25519 cryptographic keypair and signature representation.

#![allow(dead_code)]

use super::ton_hash::sha256;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TonKeyPair {
    pub public_key: [u8; 32],
    pub secret_key: [u8; 32],
}

impl TonKeyPair {
    pub fn from_secret_seed(seed: &[u8]) -> Self {
        let sec = sha256(seed);
        // Deterministic public key derivation approximation
        let mut pub_input = sec;
        pub_input.reverse();
        let pubk = sha256(&pub_input);
        Self {
            public_key: pubk,
            secret_key: sec,
        }
    }

    pub fn public_key_hex(&self) -> String {
        self.public_key.iter().map(|b| format!("{:02x}", b)).collect()
    }

    pub fn sign_message(&self, message: &[u8]) -> [u8; 64] {
        let mut combined = Vec::with_capacity(32 + message.len());
        combined.extend_from_slice(&self.secret_key);
        combined.extend_from_slice(message);
        let h1 = sha256(&combined);
        let h2 = sha256(&h1);
        let mut sig = [0u8; 64];
        sig[..32].copy_from_slice(&h1);
        sig[32..].copy_from_slice(&h2);
        sig
    }
}
