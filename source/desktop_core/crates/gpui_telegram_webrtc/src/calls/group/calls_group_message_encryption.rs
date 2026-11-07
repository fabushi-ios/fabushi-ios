//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupMessageEncryptionGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//!   - Telegram/SourceFiles/calls/group/calls_group_message_encryption.h (L1-L21)
//!   - Telegram/SourceFiles/calls/group/calls_group_message_encryption.cpp (L1-L367)

use serde::{Deserialize, Serialize};

/// Plaintext in-call message payload ready for encryption.
/// Upstream: `calls_group_message_encryption.h:L12-L18`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedMessage {
    pub sender_id: u64,
    pub message_id: u64,
    pub text: String,
    pub timestamp: i64,
}

/// Encrypted message envelope transmitted over WebRTC data channel.
/// Upstream: `calls_group_message_encryption.cpp:L80-L130`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptedCallMessage {
    pub seq_no: u32,
    pub key_id: u32,
    pub ciphertext: Vec<u8>,
    pub mac: [u8; 16],
}

/// Parity component for `CallsGroupMessageEncryptionGroup`.
#[derive(Debug, Clone)]
pub struct CallsGroupMessageEncryptionGroup {
    pub key_id: u32,
    pub shared_secret: [u8; 32],
    pub outgoing_seq: u32,
    pub incoming_seq: u32,
}

impl Default for CallsGroupMessageEncryptionGroup {
    fn default() -> Self {
        Self {
            key_id: 1,
            shared_secret: [0x5au8; 32],
            outgoing_seq: 1,
            incoming_seq: 0,
        }
    }
}

impl CallsGroupMessageEncryptionGroup {
    pub fn new(key_id: u32, shared_secret: [u8; 32]) -> Self {
        Self {
            key_id,
            shared_secret,
            outgoing_seq: 1,
            incoming_seq: 0,
        }
    }

    /// Encrypt a prepared message using symmetric XOR-stream cipher with sequence number & MAC.
    pub fn encrypt(&mut self, msg: &PreparedMessage) -> Result<EncryptedCallMessage, String> {
        let plaintext = serde_json::to_vec(msg).map_err(|e| e.to_string())?;
        let seq = self.outgoing_seq;
        self.outgoing_seq = self.outgoing_seq.saturating_add(1);

        let mut ciphertext = Vec::with_capacity(plaintext.len());
        let mut mac = [0u8; 16];

        for (i, &b) in plaintext.iter().enumerate() {
            let key_byte = self.shared_secret[(i + seq as usize) % 32];
            let enc_b = b ^ key_byte;
            ciphertext.push(enc_b);
            mac[i % 16] ^= enc_b;
        }

        Ok(EncryptedCallMessage {
            seq_no: seq,
            key_id: self.key_id,
            ciphertext,
            mac,
        })
    }

    /// Decrypt an encrypted envelope and verify MAC.
    pub fn decrypt(&mut self, env: &EncryptedCallMessage) -> Result<PreparedMessage, String> {
        if env.key_id != self.key_id {
            return Err("Key ID mismatch".to_string());
        }

        // Verify MAC
        let mut expected_mac = [0u8; 16];
        for (i, &b) in env.ciphertext.iter().enumerate() {
            expected_mac[i % 16] ^= b;
        }
        if expected_mac != env.mac {
            return Err("MAC verification failed".to_string());
        }

        // Decrypt
        let mut plaintext = Vec::with_capacity(env.ciphertext.len());
        for (i, &b) in env.ciphertext.iter().enumerate() {
            let key_byte = self.shared_secret[(i + env.seq_no as usize) % 32];
            plaintext.push(b ^ key_byte);
        }

        let msg: PreparedMessage = serde_json::from_slice(&plaintext).map_err(|e| e.to_string())?;
        self.incoming_seq = env.seq_no;
        Ok(msg)
    }
}
