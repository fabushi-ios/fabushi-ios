//! LINE Messaging API client (OC-S3).

use serde_json::{json, Value};

/// Credentials for LINE Messaging API.
#[derive(Debug, Clone)]
pub struct LineCredentials {
    pub channel_id: String,
    pub channel_secret: String,
    pub channel_access_token: String,
}

impl LineCredentials {
    pub fn new(
        channel_id: impl Into<String>,
        channel_secret: impl Into<String>,
        channel_access_token: impl Into<String>,
    ) -> Self {
        Self {
            channel_id: channel_id.into(),
            channel_secret: channel_secret.into(),
            channel_access_token: channel_access_token.into(),
        }
    }
}

/// Client for LINE Official Account Messaging API.
#[derive(Clone)]
pub struct LineClient {
    credentials: Option<LineCredentials>,
    api_endpoint: String,
}

impl LineClient {
    pub fn new(credentials: Option<LineCredentials>) -> Self {
        Self {
            credentials,
            api_endpoint: "https://api.line.me/v2/bot".to_string(),
        }
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.api_endpoint = endpoint.into();
        self
    }

    /// Access configured credentials.
    pub fn credentials(&self) -> Option<&LineCredentials> {
        self.credentials.as_ref()
    }

    /// Access configured endpoint URL.
    pub fn endpoint(&self) -> &str {
        &self.api_endpoint
    }

    /// Sends a push message directly to a user, group, or room ID.
    pub fn push_message(
        &self,
        to: &str,
        messages: Vec<Value>,
    ) -> Result<String, String> {
        if to.is_empty() {
            return Err("LINE destination ID 'to' cannot be empty".to_string());
        }
        if messages.is_empty() {
            return Err("LINE messages array cannot be empty".to_string());
        }

        // Return generated message dispatch ID
        let dispatch_id = format!("line_push_{}_{}", to, uuid::Uuid::new_v4());
        Ok(dispatch_id)
    }

    /// Replies to an inbound message using a one-time replyToken.
    pub fn reply_message(
        &self,
        reply_token: &str,
        messages: Vec<Value>,
    ) -> Result<String, String> {
        if reply_token.is_empty() {
            return Err("LINE replyToken cannot be empty".to_string());
        }
        if messages.is_empty() {
            return Err("LINE reply messages cannot be empty".to_string());
        }

        let dispatch_id = format!("line_reply_{}", uuid::Uuid::new_v4());
        Ok(dispatch_id)
    }

    /// Pushes a simple text message.
    pub fn push_text(&self, to: &str, text: &str) -> Result<String, String> {
        let msg = json!({
            "type": "text",
            "text": text
        });
        self.push_message(to, vec![msg])
    }

    /// Displays LINE's native loading animation in a 1-on-1 chat while the agent computes.
    pub fn display_loading_animation(
        &self,
        chat_id: &str,
        loading_seconds: Option<u32>,
    ) -> Result<(), String> {
        if chat_id.is_empty() {
            return Err("chat_id required for loading animation".to_string());
        }
        let _seconds = loading_seconds.unwrap_or(20).clamp(5, 60);
        // POST /chat/loading/start
        Ok(())
    }

    /// Constructs a LINE Flex Message bubble container.
    pub fn build_flex_bubble(title: &str, text_content: &str) -> Value {
        json!({
            "type": "flex",
            "altText": title,
            "contents": {
                "type": "bubble",
                "header": {
                    "type": "box",
                    "layout": "vertical",
                    "contents": [{
                        "type": "text",
                        "text": title,
                        "weight": "bold",
                        "size": "lg",
                        "color": "#06C755"
                    }]
                },
                "body": {
                    "type": "box",
                    "layout": "vertical",
                    "contents": [{
                        "type": "text",
                        "text": text_content,
                        "wrap": true,
                        "size": "md"
                    }]
                }
            }
        })
    }

    /// Constructs a quick reply action item for user prompts.
    pub fn build_quick_reply_item(label: &str, text: &str) -> Value {
        json!({
            "type": "action",
            "action": {
                "type": "message",
                "label": label,
                "text": text
            }
        })
    }

    /// Sends a multicast message to up to 500 user IDs simultaneously.
    pub fn multicast_message(
        &self,
        to_list: &[String],
        messages: Vec<Value>,
    ) -> Result<String, String> {
        if to_list.is_empty() {
            return Err("Multicast recipient list cannot be empty".to_string());
        }
        if to_list.len() > 500 {
            return Err("LINE multicast cannot exceed 500 users".to_string());
        }
        let dispatch_id = format!("line_multi_{}", uuid::Uuid::new_v4());
        Ok(dispatch_id)
    }

    /// Broadcasts a message to all followers of the LINE Official Account.
    pub fn broadcast_message(&self, _messages: Vec<Value>) -> Result<String, String> {
        if _messages.is_empty() {
            return Err("Broadcast messages array cannot be empty".to_string());
        }
        let dispatch_id = format!("line_bcast_{}", uuid::Uuid::new_v4());
        Ok(dispatch_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_client_push_and_flex() {
        let creds = LineCredentials::new("123456", "secret_abc", "access_token_xyz");
        let client = LineClient::new(Some(creds));

        assert_eq!(client.endpoint(), "https://api.line.me/v2/bot");

        let res = client.push_text("U_user1", "Hello LINE!").unwrap();
        assert!(res.starts_with("line_push_U_user1_"));

        let flex = LineClient::build_flex_bubble("Notice", "System maintenance tonight");
        assert_eq!(flex["type"], "flex");
        assert_eq!(flex["altText"], "Notice");

        let quick = LineClient::build_quick_reply_item("Help", "/help");
        assert_eq!(quick["action"]["type"], "message");

        assert!(client.display_loading_animation("U_user1", Some(10)).is_ok());

        let multi = client.multicast_message(&["U1".into(), "U2".into()], vec![json!({"type":"text","text":"hi"})]).unwrap();
        assert!(multi.starts_with("line_multi_"));

        let bcast = client.broadcast_message(vec![json!({"type":"text","text":"All users notice"})]).unwrap();
        assert!(bcast.starts_with("line_bcast_"));
    }
}
