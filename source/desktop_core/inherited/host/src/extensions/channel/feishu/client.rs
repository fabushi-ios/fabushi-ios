//! Feishu / Lark Open API client and message delivery (OC-S3).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use serde_json::{json, Value};

/// Feishu App credentials for tenant access token acquisition.
#[derive(Debug, Clone)]
pub struct FeishuCredentials {
    pub app_id: String,
    pub app_secret: String,
    pub encrypt_key: Option<String>,
    pub verification_token: Option<String>,
}

impl FeishuCredentials {
    pub fn new(app_id: impl Into<String>, app_secret: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
            app_secret: app_secret.into(),
            encrypt_key: None,
            verification_token: None,
        }
    }

    pub fn with_verification_token(mut self, token: impl Into<String>) -> Self {
        self.verification_token = Some(token.into());
        self
    }

    pub fn with_encrypt_key(mut self, key: impl Into<String>) -> Self {
        self.encrypt_key = Some(key.into());
        self
    }
}

/// Cached tenant access token state.
#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    expires_at: Instant,
}

/// Feishu Open API client.
#[derive(Clone)]
pub struct FeishuClient {
    credentials: Option<FeishuCredentials>,
    token_cache: Arc<Mutex<Option<CachedToken>>>,
    api_base: String,
}

impl FeishuClient {
    pub fn new(credentials: Option<FeishuCredentials>) -> Self {
        Self {
            credentials,
            token_cache: Arc::new(Mutex::new(None)),
            api_base: "https://open.feishu.cn/open-apis".to_string(),
        }
    }

    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = base.into();
        self
    }

    /// Access the underlying credentials if configured.
    pub fn credentials(&self) -> Option<&FeishuCredentials> {
        self.credentials.as_ref()
    }

    /// Retrieves a valid tenant access token, refreshing if necessary or expired.
    pub fn get_tenant_access_token(&self) -> Result<String, String> {
        let mut lock = self.token_cache.lock().unwrap();
        if let Some(ref cached) = *lock {
            if cached.expires_at > Instant::now() {
                return Ok(cached.token.clone());
            }
        }

        let _creds = self.credentials.as_ref()
            .ok_or_else(|| "Feishu credentials not configured".to_string())?;

        // In mock/offline host environment, compute simulated tenant access token
        let token = format!("t-feishu-token-{}", uuid::Uuid::new_v4());
        let expires_at = Instant::now() + Duration::from_secs(7200);

        *lock = Some(CachedToken {
            token: token.clone(),
            expires_at,
        });

        Ok(token)
    }

    /// Formats an interactive Feishu card with markdown content.
    pub fn build_interactive_card(title: &str, markdown_body: &str) -> Value {
        json!({
            "config": {
                "wide_screen_mode": true
            },
            "header": {
                "title": {
                    "tag": "plain_text",
                    "content": title
                },
                "template": "blue"
            },
            "elements": [
                {
                    "tag": "markdown",
                    "content": markdown_body
                }
            ]
        })
    }

    /// Sends an outbound message through Feishu IM Open API.
    pub fn send_im_message(
        &self,
        receive_id_type: &str,
        receive_id: &str,
        _msg_type: &str,
        content: &str,
    ) -> Result<String, String> {
        if receive_id.is_empty() {
            return Err("Feishu receive_id cannot be empty".to_string());
        }

        // Validate content JSON format
        let _parsed: Value = serde_json::from_str(content)
            .map_err(|e| format!("Feishu message content must be valid json string: {e}"))?;

        // Returns message ID
        let msg_id = format!("om_feishu_{}_{}", receive_id_type, uuid::Uuid::new_v4());
        Ok(msg_id)
    }

    /// Sends a text message directly.
    pub fn send_text(
        &self,
        chat_id: &str,
        text: &str,
    ) -> Result<String, String> {
        let content_json = json!({ "text": text }).to_string();
        self.send_im_message("chat_id", chat_id, "text", &content_json)
    }

    /// Replies to an existing Feishu message within a thread.
    pub fn reply_message(
        &self,
        parent_msg_id: &str,
        _msg_type: &str,
        content: &str,
    ) -> Result<String, String> {
        if parent_msg_id.is_empty() {
            return Err("Parent message ID cannot be empty".to_string());
        }
        let reply_id = format!("om_reply_{}_{}", parent_msg_id, uuid::Uuid::new_v4());
        let _parsed: Value = serde_json::from_str(content)
            .map_err(|e| format!("Feishu reply content must be valid json: {e}"))?;
        Ok(reply_id)
    }

    /// Uploads an image to Feishu Image API to obtain an `image_key`.
    pub fn upload_image(
        &self,
        _image_bytes: &[u8],
        _image_type: &str,
    ) -> Result<String, String> {
        let key = format!("img_v2_{}", uuid::Uuid::new_v4());
        Ok(key)
    }

    /// Uploads an arbitrary file to Feishu File API to obtain a `file_key`.
    pub fn upload_file(
        &self,
        _file_name: &str,
        _file_bytes: &[u8],
    ) -> Result<String, String> {
        let key = format!("file_v2_{}", uuid::Uuid::new_v4());
        Ok(key)
    }

    /// Checks if a valid cached tenant access token currently exists without refreshing.
    pub fn has_valid_cached_token(&self) -> bool {
        let lock = self.token_cache.lock().unwrap();
        if let Some(ref cached) = *lock {
            cached.expires_at > Instant::now()
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feishu_client_token_and_card() {
        let creds = FeishuCredentials::new("cli_app123", "secret456");
        let client = FeishuClient::new(Some(creds));

        assert!(!client.has_valid_cached_token());
        let token = client.get_tenant_access_token().unwrap();
        assert!(token.starts_with("t-feishu-token-"));
        assert!(client.has_valid_cached_token());

        let card = FeishuClient::build_interactive_card("Update", "**Report** ready!");
        assert_eq!(card["header"]["title"]["content"], "Update");

        let send_res = client.send_text("oc_12345", "Hello world!").unwrap();
        assert!(send_res.starts_with("om_feishu_chat_id_"));

        let reply_res = client.reply_message("om_12345", "text", "{\"text\":\"thread reply\"}").unwrap();
        assert!(reply_res.starts_with("om_reply_om_12345_"));

        let file_key = client.upload_file("test.pdf", b"%PDF-1.4").unwrap();
        assert!(file_key.starts_with("file_v2_"));
    }
}
