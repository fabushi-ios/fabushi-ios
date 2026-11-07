use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::OnceLock;
use std::time::Duration;
use thiserror::Error;
use tokio::runtime::{Handle, Runtime};

pub(crate) fn ensure_runtime_handle(explicit: Option<Handle>) -> Handle {
    if let Some(h) = explicit {
        return h;
    }
    if let Ok(h) = Handle::try_current() {
        return h;
    }
    static FALLBACK_RT: OnceLock<Runtime> = OnceLock::new();
    FALLBACK_RT
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("gpui-tokio-fallback")
                .enable_all()
                .build()
                .expect("Failed to initialize fallback Tokio runtime")
        })
        .handle()
        .clone()
}

#[derive(Error, Debug)]
pub enum HttpClientError {
    #[error("Network connection error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("API error ({status}): {message}")]
    Api { status: u16, message: String },
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Conflict (409): {0}")]
    Conflict(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatusResponse {
    pub server: String,
    pub status: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    pub first_name: String,
    pub device_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub device_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub user_id: u64,
    pub username: String,
    pub session_id: String,
    pub token: String,
    pub first_name: Option<String>,
    pub created_at: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMeResponse {
    pub user_id: u64,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub session_id: String,
    pub device_name: Option<String>,
    pub account_generation: Option<u64>,
    pub created_at: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunRequest {
    pub prompt: String,
    pub session_id: String,
    pub model_id: Option<String>,
    pub thinking_budget_tokens: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunResponse {
    pub run_id: Option<String>,
    pub session_id: String,
    pub user_id: Option<u64>,
    pub status: String,
    pub prompt: String,
    pub response: String,
    #[serde(default)]
    pub outputs: Vec<serde_json::Value>,
}

#[derive(Clone)]
pub struct HttpClient {
    pub base_url: String,
    client: Client,
    runtime_handle: Handle,
}

impl HttpClient {
    pub fn new(base_url: String) -> Self {
        Self::new_with_timeout(base_url, Duration::from_secs(15))
    }

    pub fn new_with_timeout(base_url: String, timeout: Duration) -> Self {
        Self::new_with_runtime(base_url, timeout, None)
    }

    pub fn new_with_runtime(
        base_url: String,
        timeout: Duration,
        runtime_handle: Option<Handle>,
    ) -> Self {
        let handle = ensure_runtime_handle(runtime_handle);
        let _guard = handle.enter();
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
            runtime_handle: handle,
        }
    }

    async fn run_on_tokio<F, T>(&self, fut: F) -> Result<T, HttpClientError>
    where
        F: Future<Output = Result<T, HttpClientError>> + Send + 'static,
        T: Send + 'static,
    {
        if Handle::try_current().is_ok() {
            fut.await
        } else {
            let handle = self.runtime_handle.clone();
            handle
                .spawn(fut)
                .await
                .map_err(|e| HttpClientError::Api {
                    status: 500,
                    message: format!("Tokio runtime task join error: {}", e),
                })?
        }
    }

    /// Checks server /health endpoint.
    pub async fn check_health(&self) -> Result<bool, HttpClientError> {
        let client = self.client.clone();
        let url = format!("{}/health", self.base_url);
        self.run_on_tokio(async move {
            let resp = client.get(&url).send().await?;
            Ok(resp.status().is_success())
        })
        .await
    }

    /// Checks server /api/v1/status endpoint.
    pub async fn check_status(&self) -> Result<ServerStatusResponse, HttpClientError> {
        let client = self.client.clone();
        let url = format!("{}/api/v1/status", self.base_url);
        self.run_on_tokio(async move {
            let resp = client.get(&url).send().await?;
            if resp.status().is_success() {
                let data = resp.json::<ServerStatusResponse>().await?;
                Ok(data)
            } else {
                Err(HttpClientError::Api {
                    status: resp.status().as_u16(),
                    message: resp.text().await.unwrap_or_default(),
                })
            }
        })
        .await
    }

    /// Registers a new user account via POST /api/v1/auth/register.
    pub async fn register(&self, req: &RegisterRequest) -> Result<AuthResponse, HttpClientError> {
        let client = self.client.clone();
        let url = format!("{}/api/v1/auth/register", self.base_url);
        let req_owned = req.clone();
        self.run_on_tokio(async move {
            let resp = client.post(&url).json(&req_owned).send().await?;
            let status = resp.status();
            if status == StatusCode::CREATED || status == StatusCode::OK {
                let data = resp.json::<AuthResponse>().await?;
                Ok(data)
            } else if status == StatusCode::CONFLICT {
                let msg = resp.text().await.unwrap_or_default();
                Err(HttpClientError::Conflict(msg))
            } else {
                let body = resp.text().await.unwrap_or_default();
                Err(HttpClientError::Api {
                    status: status.as_u16(),
                    message: body,
                })
            }
        })
        .await
    }

    /// Authenticates user credentials via POST /api/v1/auth/login.
    pub async fn login(&self, req: &LoginRequest) -> Result<AuthResponse, HttpClientError> {
        let client = self.client.clone();
        let url = format!("{}/api/v1/auth/login", self.base_url);
        let req_owned = req.clone();
        self.run_on_tokio(async move {
            let resp = client.post(&url).json(&req_owned).send().await?;
            let status = resp.status();
            if status == StatusCode::OK {
                let data = resp.json::<AuthResponse>().await?;
                Ok(data)
            } else if status == StatusCode::UNAUTHORIZED {
                let msg = resp.text().await.unwrap_or_default();
                Err(HttpClientError::Unauthorized(msg))
            } else {
                let body = resp.text().await.unwrap_or_default();
                Err(HttpClientError::Api {
                    status: status.as_u16(),
                    message: body,
                })
            }
        })
        .await
    }

    /// Validates current session token and retrieves user profile via GET /api/v1/auth/me.
    pub async fn get_me(&self, token: &str) -> Result<UserMeResponse, HttpClientError> {
        let client = self.client.clone();
        let url = format!("{}/api/v1/auth/me", self.base_url);
        let token_owned = token.to_string();
        self.run_on_tokio(async move {
            let resp = client
                .get(&url)
                .bearer_auth(&token_owned)
                .send()
                .await?;
            let status = resp.status();
            if status == StatusCode::OK {
                let data = resp.json::<UserMeResponse>().await?;
                Ok(data)
            } else if status == StatusCode::UNAUTHORIZED {
                let msg = resp.text().await.unwrap_or_default();
                Err(HttpClientError::Unauthorized(msg))
            } else {
                let body = resp.text().await.unwrap_or_default();
                Err(HttpClientError::Api {
                    status: status.as_u16(),
                    message: body,
                })
            }
        })
        .await
    }

    /// Dispatches an AI Bot execution request via POST /api/v1/agent/runs.
    pub async fn agent_runs(
        &self,
        token: &str,
        req: &AgentRunRequest,
    ) -> Result<AgentRunResponse, HttpClientError> {
        let client = self.client.clone();
        let url = format!("{}/api/v1/agent/runs", self.base_url);
        let token_owned = token.to_string();
        let req_owned = req.clone();
        self.run_on_tokio(async move {
            let resp = client
                .post(&url)
                .bearer_auth(&token_owned)
                .json(&req_owned)
                .send()
                .await?;
            let status = resp.status();
            if status == StatusCode::OK {
                let data = resp.json::<AgentRunResponse>().await?;
                Ok(data)
            } else if status == StatusCode::UNAUTHORIZED {
                let msg = resp.text().await.unwrap_or_default();
                Err(HttpClientError::Unauthorized(msg))
            } else {
                let body = resp.text().await.unwrap_or_default();
                Err(HttpClientError::Api {
                    status: status.as_u16(),
                    message: body,
                })
            }
        })
        .await
    }

    /// Convenience wrapper to dispatch agent run prompt and parse result.
    pub async fn dispatch_agent_run(
        &self,
        token: &str,
        session_id: &str,
        prompt: &str,
        model_id: Option<&str>,
    ) -> std::result::Result<AgentRunResponse, String> {
        let req = AgentRunRequest {
            prompt: prompt.to_string(),
            session_id: session_id.to_string(),
            model_id: model_id.map(String::from).or_else(|| Some("grok-3".to_string())),
            thinking_budget_tokens: Some(1024),
        };
        self.agent_runs(token, &req)
            .await
            .map_err(|e| e.to_string())
    }

    /// Revokes an active session via DELETE /api/v1/auth/sessions/{session_id}.
    pub async fn revoke_session(
        &self,
        token: &str,
        session_id: &str,
    ) -> Result<bool, HttpClientError> {
        let client = self.client.clone();
        let url = format!("{}/api/v1/auth/sessions/{}", self.base_url, session_id);
        let token_owned = token.to_string();
        self.run_on_tokio(async move {
            let resp = client
                .delete(&url)
                .bearer_auth(&token_owned)
                .send()
                .await?;
            if resp.status().is_success() {
                Ok(true)
            } else if resp.status() == StatusCode::UNAUTHORIZED {
                Err(HttpClientError::Unauthorized(
                    resp.text().await.unwrap_or_default(),
                ))
            } else {
                Err(HttpClientError::Api {
                    status: resp.status().as_u16(),
                    message: resp.text().await.unwrap_or_default(),
                })
            }
        })
        .await
    }
}
