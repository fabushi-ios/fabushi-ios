use crate::client::TelegramClient;
use crate::e2ee::{compute_safety_number_emojis, E2eeError};
use gpui_telegram_proto::e2ee::{EncryptedEnvelope, PublishPreKeysResponse};

impl TelegramClient {
    pub async fn e2ee_init_and_publish_prekeys(
        &self,
        otpk_batch_size: usize,
    ) -> Result<PublishPreKeysResponse, E2eeError> {
        self.e2ee.init_and_publish_prekeys(otpk_batch_size).await
    }

    pub async fn e2ee_encrypt_dm(
        &self,
        recipient_user_id: u64,
        plaintext: &[u8],
    ) -> Result<EncryptedEnvelope, E2eeError> {
        self.e2ee.encrypt_dm(recipient_user_id, plaintext).await
    }

    pub async fn e2ee_encrypt_group(
        &self,
        chat_id: i64,
        member_user_ids: &[u64],
        plaintext: &[u8],
    ) -> Result<(Option<EncryptedEnvelope>, EncryptedEnvelope), E2eeError> {
        self.e2ee
            .encrypt_group(chat_id, member_user_ids, plaintext)
            .await
    }

    pub fn e2ee_decrypt_envelope(&self, envelope: &EncryptedEnvelope) -> Result<Vec<u8>, E2eeError> {
        self.e2ee.decrypt_envelope(envelope)
    }

    pub fn e2ee_safety_fingerprint(
        &self,
        remote_identity_key_b64: &str,
    ) -> Result<String, E2eeError> {
        let id = self.e2ee.identity()?;
        let remote = crate::e2ee::keys::b64_decode_32(remote_identity_key_b64)?;
        Ok(compute_safety_number_emojis(&id.x25519_pub, &remote))
    }
}
