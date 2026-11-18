use crate::client::TelegramClient;
use crate::net::HttpClientError;

impl TelegramClient {
    pub async fn list_tasks(&self) -> std::result::Result<Vec<serde_json::Value>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_tasks(&token).await
    }

    pub async fn create_task(
        &self,
        payload: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.create_task(&token, payload).await
    }

    pub async fn approve_task(
        &self,
        task_id: &str,
        approved: bool,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.approve_task(&token, task_id, approved).await
    }

    pub async fn list_mcp_servers(
        &self,
    ) -> std::result::Result<Vec<serde_json::Value>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_mcp_servers(&token).await
    }

    pub async fn register_mcp_server(
        &self,
        payload: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.register_mcp_server(&token, payload).await
    }

    pub async fn list_mcp_tools(
        &self,
    ) -> std::result::Result<Vec<serde_json::Value>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_mcp_tools(&token).await
    }

    pub async fn call_mcp_tool(
        &self,
        payload: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.call_mcp_tool(&token, payload).await
    }

    pub async fn approve_mcp_permission(
        &self,
        payload: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.approve_mcp_permission(&token, payload).await
    }

    pub async fn acquire_computer_lease(
        &self,
        payload: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.acquire_computer_lease(&token, payload).await
    }

    pub async fn execute_computer_action(
        &self,
        payload: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.execute_computer_action(&token, payload).await
    }

    pub async fn create_talk_session(
        &self,
        payload: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.create_talk_session(&token, payload).await
    }

    pub async fn talk_barge_in(
        &self,
        payload: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.talk_barge_in(&token, payload).await
    }

    pub async fn enqueue_talk_audio(
        &self,
        payload: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.enqueue_talk_audio(&token, payload).await
    }
}
