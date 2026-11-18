use crate::temp_dir::TempTestDir;
use axum::Router;
use gpui_telegram_server::{build_router_with_state, server::AppState, ServerConfig};
use reqwest::{Client, Method, Response};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// Ephemeral file-backed Axum server bound to `127.0.0.1:0` with RAII temp
/// SQLite WAL + CAS + Tantivy directories and clean restart support.
pub struct EphemeralTestServer {
    dir: Arc<TempTestDir>,
    db_name: String,
    addr: SocketAddr,
    state: Option<Arc<AppState>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    server_task: Option<JoinHandle<()>>,
}

impl EphemeralTestServer {
    /// Boots a new durable `AppState` and listens on `127.0.0.1:0`.
    pub async fn start() -> anyhow::Result<Self> {
        let dir = Arc::new(TempTestDir::new("tier4_e2e")?);
        let db_name = "e2e_server.db".to_string();
        let (addr, state, shutdown_tx, server_task) = Self::spawn_instance(&dir, &db_name).await?;
        Ok(Self {
            dir,
            db_name,
            addr,
            state: Some(state),
            shutdown_tx: Some(shutdown_tx),
            server_task: Some(server_task),
        })
    }

    async fn spawn_instance(
        dir: &TempTestDir,
        db_name: &str,
    ) -> anyhow::Result<(
        SocketAddr,
        Arc<AppState>,
        oneshot::Sender<()>,
        JoinHandle<()>,
    )> {
        let cfg = ServerConfig {
            database_url: dir.child(db_name).to_string_lossy().into_owned(),
            media_storage_path: dir.child("cas").to_string_lossy().into_owned(),
            ..Default::default()
        };
        let state = Arc::new(AppState::try_new(cfg).map_err(|e| anyhow::anyhow!(e))?);
        let router: Router = build_router_with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (tx, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });
        Ok((addr, state, tx, task))
    }

    /// Gracefully stops the server, drops `AppState`, and restarts on the same
    /// SQLite database, media CAS directory, and Tantivy index.
    pub async fn restart(&mut self) -> anyhow::Result<()> {
        self.stop().await;
        let (addr, state, shutdown_tx, server_task) =
            Self::spawn_instance(&self.dir, &self.db_name).await?;
        self.addr = addr;
        self.state = Some(state);
        self.shutdown_tx = Some(shutdown_tx);
        self.server_task = Some(server_task);
        Ok(())
    }

    /// Shuts down the HTTP/WS listener and drops `AppState`.
    pub async fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.server_task.take() {
            let _ = task.await;
        }
        if let Some(state) = self.state.take() {
            let weak_state = Arc::downgrade(&state);
            let weak_hub = Arc::downgrade(&state.gateway_hub);
            let weak_idx = Arc::downgrade(&state.search_indexer);
            drop(state);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while (weak_state.strong_count() > 0
                || weak_hub.strong_count() > 0
                || weak_idx.strong_count() > 0)
                && std::time::Instant::now() < deadline
            {
                tokio::time::sleep(std::time::Duration::from_millis(15)).await;
            }
        }
    }

    /// Returns the HTTP base URL (`http://127.0.0.1:<port>`).
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Returns the WebSocket URL (`/api/v1/ws?token=...`).
    pub fn ws_url(&self, token: &str) -> String {
        format!("ws://{}/api/v1/ws?token={}", self.addr, token)
    }

    /// Returns a reference to the active `AppState`.
    pub fn state(&self) -> Option<&Arc<AppState>> {
        self.state.as_ref()
    }
}

impl Drop for EphemeralTestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.server_task.take() {
            task.abort();
        }
    }
}

/// Multi-node E2E HTTP/WS test driver targeting an `EphemeralTestServer` or live URL.
pub struct E2eSystemDriver {
    pub base_url: String,
    pub http: Client,
}

impl E2eSystemDriver {
    /// Connects a new driver to `base_url`.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("reqwest client"),
        }
    }

    /// Sends a JSON HTTP request and returns `(status_code, parsed_json)`.
    pub async fn json_req(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> anyhow::Result<(u16, Value)> {
        let mut req = self
            .http
            .request(method, format!("{}{}", self.base_url, path));
        if let Some(tok) = token {
            req = req.bearer_auth(tok);
        }
        if let Some(payload) = body {
            req = req.json(&payload);
        }
        let resp: Response = req.send().await?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await?;
        let parsed = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Ok((status, parsed))
    }

    /// Registers a new user and returns `(user_id, token)`.
    pub async fn register_user(&self, username: &str, password: &str) -> anyhow::Result<(u64, String)> {
        let (st, v) = self
            .json_req(
                Method::POST,
                "/api/v1/auth/register",
                None,
                Some(json!({
                    "username": username,
                    "password": password,
                    "first_name": username,
                    "device_name": "Tier4Harness"
                })),
            )
            .await?;
        anyhow::ensure!(st == 201 || st == 200, "register failed {st}: {v}");
        let uid = v["user_id"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("missing user_id: {v}"))?;
        let tok = v["token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing token: {v}"))?
            .to_string();
        Ok((uid, tok))
    }
}
