use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct CanonicalTokenPayload<'a> {
    token_id: &'a str,
    agent_id: &'a str,
    chat_id: i64,
    allowed_actions: Vec<&'a str>,
    expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopedCapabilityToken {
    pub token_id: String,
    pub agent_id: String,
    pub chat_id: i64,
    pub allowed_actions: Vec<String>,
    pub expires_at: i64,
    pub signature_hex: String,
}

impl ScopedCapabilityToken {
    pub fn canonical_payload(
        token_id: &str,
        agent_id: &str,
        chat_id: i64,
        actions: &[String],
        expires_at: i64,
    ) -> Vec<u8> {
        let mut sorted_actions: Vec<&str> = actions.iter().map(String::as_str).collect();
        sorted_actions.sort_unstable();
        serde_json::to_vec(&CanonicalTokenPayload {
            token_id,
            agent_id,
            chat_id,
            allowed_actions: sorted_actions,
            expires_at,
        })
        .expect("canonical token payload serialization")
    }

    pub fn issue(
        key: &SigningKey,
        agent_id: &str,
        chat_id: i64,
        actions: Vec<String>,
        ttl_secs: i64,
    ) -> Self {
        let token_id = uuid::Uuid::new_v4().to_string();
        let expires_at = chrono::Utc::now().timestamp() + ttl_secs;
        let payload = Self::canonical_payload(&token_id, agent_id, chat_id, &actions, expires_at);
        let signature = key.sign(&payload);
        let signature_hex = encode_hex(signature.to_bytes());

        Self {
            token_id,
            agent_id: agent_id.to_string(),
            chat_id,
            allowed_actions: actions,
            expires_at,
            signature_hex,
        }
    }

    pub fn verify(&self, verifying_key: &VerifyingKey, now_ts: i64) -> bool {
        if now_ts > self.expires_at {
            return false;
        }

        let sig_bytes = match decode_hex(&self.signature_hex) {
            Ok(b) if b.len() == 64 => b,
            _ => return false,
        };

        let mut sig_arr = [0u8; 64];
        sig_arr.copy_from_slice(&sig_bytes);
        let signature = Signature::from_bytes(&sig_arr);

        let payload = Self::canonical_payload(
            &self.token_id,
            &self.agent_id,
            self.chat_id,
            &self.allowed_actions,
            self.expires_at,
        );

        verifying_key.verify(&payload, &signature).is_ok()
    }

    pub fn verify_for_scope(
        &self,
        verifying_key: &VerifyingKey,
        now_ts: i64,
        expected_agent_id: &str,
        expected_chat_id: i64,
        action: &str,
    ) -> bool {
        self.verify(verifying_key, now_ts)
            && self.agent_id == expected_agent_id
            && self.chat_id == expected_chat_id
            && self.permits_action(action)
    }

    pub fn permits_action(&self, action: &str) -> bool {
        self.allowed_actions.iter().any(|a| a == action || a == "*")
    }
}

fn encode_hex(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn decode_hex(hex: &str) -> Result<Vec<u8>, ()> {
    if hex.len() % 2 != 0 {
        return Err(());
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        let byte = u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| ())?;
        out.push(byte);
    }
    Ok(out)
}
