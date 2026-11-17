use crate::net::http_client::{HttpClient, HttpClientError};
use crate::net::http_helpers::percent_encode_rfc3986;
use reqwest::Method;
use serde::Serialize;

#[derive(Serialize)]
struct ApproveBody {
    approved: bool,
}

impl HttpClient {
    pub async fn list_tasks(
        &self,
        token: &str,
    ) -> Result<Vec<serde_json::Value>, HttpClientError> {
        self.request_vec_or_field(Method::GET, "/api/v1/tasks", Some(token), &["tasks", "items"])
            .await
    }

    pub async fn create_task(
        &self,
        token: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/tasks", Some(token), Some(payload))
            .await
    }

    pub async fn approve_task(
        &self,
        token: &str,
        task_id: &str,
        approved: bool,
    ) -> Result<serde_json::Value, HttpClientError> {
        let path = format!("/api/v1/tasks/{}/approve", percent_encode_rfc3986(task_id));
        let body = ApproveBody { approved };
        self.request_json(Method::POST, &path, Some(token), Some(&body))
            .await
    }

    pub async fn list_mcp_servers(
        &self,
        token: &str,
    ) -> Result<Vec<serde_json::Value>, HttpClientError> {
        self.request_vec_or_field(
            Method::GET,
            "/api/v1/mcp/servers",
            Some(token),
            &["servers", "items"],
        )
        .await
    }

    pub async fn register_mcp_server(
        &self,
        token: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/mcp/servers", Some(token), Some(payload))
            .await
    }

    pub async fn list_mcp_tools(
        &self,
        token: &str,
    ) -> Result<Vec<serde_json::Value>, HttpClientError> {
        self.request_vec_or_field(
            Method::GET,
            "/api/v1/mcp/tools",
            Some(token),
            &["tools", "items"],
        )
        .await
    }

    pub async fn call_mcp_tool(
        &self,
        token: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/mcp/call", Some(token), Some(payload))
            .await
    }

    pub async fn approve_mcp_permission(
        &self,
        token: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, HttpClientError> {
        self.request_json(
            Method::POST,
            "/api/v1/mcp/permission/approve",
            Some(token),
            Some(payload),
        )
        .await
    }

    pub async fn acquire_computer_lease(
        &self,
        token: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, HttpClientError> {
        self.request_json(
            Method::POST,
            "/api/v1/computer/lease",
            Some(token),
            Some(payload),
        )
        .await
    }

    pub async fn execute_computer_action(
        &self,
        token: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, HttpClientError> {
        self.request_json(
            Method::POST,
            "/api/v1/computer/action",
            Some(token),
            Some(payload),
        )
        .await
    }

    pub async fn create_talk_session(
        &self,
        token: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, HttpClientError> {
        self.request_json(
            Method::POST,
            "/api/v1/talk/session",
            Some(token),
            Some(payload),
        )
        .await
    }

    pub async fn talk_barge_in(
        &self,
        token: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, HttpClientError> {
        self.request_json(
            Method::POST,
            "/api/v1/talk/barge_in",
            Some(token),
            Some(payload),
        )
        .await
    }

    pub async fn enqueue_talk_audio(
        &self,
        token: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, HttpClientError> {
        self.request_json(
            Method::POST,
            "/api/v1/talk/audio",
            Some(token),
            Some(payload),
        )
        .await
    }
}
