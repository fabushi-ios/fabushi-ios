use std::collections::HashMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::{Duration, Instant};

use ed25519_dalek::{Signer, SigningKey};
use futures_util::{SinkExt, StreamExt};
use rand::rngs::OsRng;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE, RANGE};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

static RUSTLS_INIT: Once = Once::new();

pub fn ensure_rustls_crypto_provider() {
    RUSTLS_INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

pub fn unique_tag(prefix: &str) -> String {
    let raw = Uuid::new_v4().simple().to_string();
    format!("{}_{}", prefix, &raw[..10])
}

fn resolve_scratch_root() -> PathBuf {
    for key in ["RUNNER_TEMP", "FABUSHI_E2E_TMP_ROOT", "TMPDIR"] {
        if let Ok(val) = std::env::var(key) {
            let trimmed = val.trim();
            if !trimmed.is_empty() && trimmed != "/tmp" {
                return PathBuf::from(trimmed);
            }
        }
    }
    PathBuf::from("/home/antigravity/.gemini/jetski/scratch/teamwork_tmp/e2e_runner")
}

fn allocate_ephemeral_port() -> u16 {
    let listener = TcpListener::bind("0.0.0.0:0").expect("bind ephemeral port on 0.0.0.0:0");
    listener.local_addr().expect("local_addr").port()
}

/// Manages either an attached loopback `gpui_telegram_server` (`FABUSHI_SERVER_URL`)
/// or an isolated per-test spawned `gpui_telegram_server` child process (`FABUSHI_SERVER_BIN`).
pub struct LoopbackServerCluster {
    base_url: String,
    data_dir: Option<PathBuf>,
    child: Option<Child>,
}

impl LoopbackServerCluster {
    /// Attaches to `FABUSHI_SERVER_URL` if present, otherwise spawns an isolated child server
    /// from `FABUSHI_SERVER_BIN`.
    pub async fn attach_or_spawn() -> Self {
        ensure_rustls_crypto_provider();
        if let Ok(url) = std::env::var("FABUSHI_SERVER_URL") {
            let trimmed = url.trim().trim_end_matches('/').to_string();
            if !trimmed.is_empty() {
                let cluster = Self {
                    base_url: trimmed,
                    data_dir: None,
                    child: None,
                };
                cluster.wait_until_healthy(Duration::from_secs(15)).await;
                return cluster;
            }
        }
        Self::spawn_isolated().await
    }

    /// Spawns a fresh isolated `gpui_telegram_server` instance with its own SQLite database,
    /// CAS media directory, and Hermes workspace when `FABUSHI_SERVER_BIN` is set; otherwise
    /// falls back to `FABUSHI_SERVER_URL`.
    pub async fn spawn_isolated() -> Self {
        ensure_rustls_crypto_provider();
        if let Ok(bin_raw) = std::env::var("FABUSHI_SERVER_BIN") {
            let bin = PathBuf::from(bin_raw.trim());
            if bin.is_file() {
                return Self::spawn_from_binary(&bin).await;
            }
        }
        if let Ok(url) = std::env::var("FABUSHI_SERVER_URL") {
            let trimmed = url.trim().trim_end_matches('/').to_string();
            if !trimmed.is_empty() {
                let cluster = Self {
                    base_url: trimmed,
                    data_dir: None,
                    child: None,
                };
                cluster.wait_until_healthy(Duration::from_secs(15)).await;
                return cluster;
            }
        }
        panic!(
            "Neither executable FABUSHI_SERVER_BIN nor reachable FABUSHI_SERVER_URL is configured"
        );
    }

    pub async fn spawn_from_binary(bin: &Path) -> Self {
        ensure_rustls_crypto_provider();
        let port = allocate_ephemeral_port();
        let root = resolve_scratch_root().join("isolated_servers");
        let run_dir = root.join(format!("srv_{}_{}", port, &Uuid::new_v4().simple().to_string()[..8]));
        let media_dir = run_dir.join("media_cas");
        let hermes_dir = run_dir.join("hermes_home");
        std::fs::create_dir_all(&media_dir).expect("create media_cas dir");
        std::fs::create_dir_all(&hermes_dir).expect("create hermes_home dir");
        let db_path = run_dir.join("e2e.db");

        let mut cmd = Command::new(bin);
        cmd.current_dir(&run_dir)
            .env("HOST", "127.0.0.1")
            .env("PORT", port.to_string())
            .env("SERVER_HOST", "127.0.0.1")
            .env("SERVER_PORT", port.to_string())
            .env("DATABASE_URL", format!("sqlite://{}", db_path.display()))
            .env("MEDIA_STORAGE_PATH", media_dir.display().to_string())
            .env("HERMES_HOME", hermes_dir.display().to_string())
            .env("GPUI_OFFLINE_TEST", "1")
            .kill_on_drop(true)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        let child = cmd.spawn().expect("spawn gpui_telegram_server child process");
        let cluster = Self {
            base_url: format!("http://127.0.0.1:{port}"),
            data_dir: Some(run_dir),
            child: Some(child),
        };
        cluster.wait_until_healthy(Duration::from_secs(20)).await;
        cluster
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn ws_url(&self, token: &str) -> String {
        let ws_base = if let Some(rest) = self.base_url.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.base_url.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            format!("ws://{}", self.base_url)
        };
        format!("{ws_base}/api/v1/ws?token={}", urlencoding::encode(token))
    }

    pub fn unauthenticated_actor(&self) -> OpaqueActorClient {
        OpaqueActorClient::new(&self.base_url)
    }

    pub async fn register_actor(&self, prefix: &str, password: &str) -> OpaqueActorClient {
        let mut actor = self.unauthenticated_actor();
        let uname = unique_tag(prefix);
        let resp = actor
            .post_json(
                "/api/v1/auth/register",
                &json!({
                    "username": uname,
                    "password": password,
                    "first_name": prefix,
                    "last_name": "E2E",
                    "device_name": format!("{prefix}-iPhone16Pro"),
                    "auto_welcome": true
                }),
            )
            .await;
        assert!(
            resp.status.is_success(),
            "register_actor failed ({}): {}",
            resp.status,
            resp.body_text()
        );
        let body = resp.json();
        let uid = body["user_id"].as_u64().expect("user_id u64");
        let sid = body["session_id"].as_str().expect("session_id str").to_string();
        let token = body["token"].as_str().expect("token str").to_string();
        actor.user_id = Some(uid);
        actor.username = Some(uname);
        actor.session_id = Some(sid);
        actor.bearer_token = Some(token);
        actor
    }

    async fn wait_until_healthy(&self, timeout: Duration) {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("build health reqwest client");
        let health_url = format!("{}/health", self.base_url);
        let status_url = format!("{}/api/v1/status", self.base_url);
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let (Ok(h), Ok(s)) = (client.get(&health_url).send().await, client.get(&status_url).send().await) {
                if h.status() == StatusCode::OK && s.status() == StatusCode::OK {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("Loopback server at {} did not become healthy within {:?}", self.base_url, timeout);
    }
}

impl Drop for LoopbackServerCluster {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
        if let Some(dir) = self.data_dir.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpEnvelope {
    pub status: StatusCode,
    pub headers: HashMap<String, String>,
    pub bytes: Vec<u8>,
}

impl HttpEnvelope {
    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.bytes).unwrap_or_else(|e| {
            panic!(
                "Expected JSON body (status {}), decode error: {}, raw: {}",
                self.status,
                e,
                self.body_text()
            )
        })
    }
}

#[derive(Clone)]
pub struct OpaqueActorClient {
    pub base_url: String,
    pub http: reqwest::Client,
    pub user_id: Option<u64>,
    pub username: Option<String>,
    pub session_id: Option<String>,
    pub bearer_token: Option<String>,
}

impl OpaqueActorClient {
    pub fn new(base_url: &str) -> Self {
        ensure_rustls_crypto_provider();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .expect("build actor reqwest client");
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
            user_id: None,
            username: None,
            session_id: None,
            bearer_token: None,
        }
    }

    pub fn uid(&self) -> u64 {
        self.user_id.expect("authenticated user_id")
    }

    pub fn token(&self) -> &str {
        self.bearer_token.as_deref().expect("authenticated bearer_token")
    }

    pub fn session_id(&self) -> &str {
        self.session_id.as_deref().expect("authenticated session_id")
    }

    pub async fn request(
        &self,
        method: Method,
        path: &str,
        extra_headers: &[(&str, &str)],
        body: Option<Vec<u8>>,
        content_type: Option<&str>,
    ) -> HttpEnvelope {
        let url = format!("{}{}", self.base_url, path);
        let mut headers = HeaderMap::new();
        if let Some(ref tok) = self.bearer_token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {tok}")).expect("auth header"),
            );
        }
        if let Some(ct) = content_type {
            headers.insert(CONTENT_TYPE, HeaderValue::from_str(ct).expect("content-type header"));
        }
        for (k, v) in extra_headers {
            let hn = HeaderName::from_bytes(k.as_bytes()).expect("valid header name");
            let hv = HeaderValue::from_str(v).expect("valid header value");
            headers.insert(hn, hv);
        }
        let mut req = self.http.request(method, &url).headers(headers);
        if let Some(b) = body {
            req = req.body(b);
        }
        let resp = req.send().await.unwrap_or_else(|e| panic!("HTTP request to {url} failed: {e}"));
        let status = resp.status();
        let mut map = HashMap::new();
        for (k, v) in resp.headers() {
            if let Ok(s) = v.to_str() {
                map.insert(k.as_str().to_ascii_lowercase(), s.to_string());
            }
        }
        let bytes = resp.bytes().await.expect("read response bytes").to_vec();
        HttpEnvelope {
            status,
            headers: map,
            bytes,
        }
    }

    pub async fn get(&self, path: &str) -> HttpEnvelope {
        self.request(Method::GET, path, &[], None, None).await
    }

    pub async fn get_with_headers(&self, path: &str, headers: &[(&str, &str)]) -> HttpEnvelope {
        self.request(Method::GET, path, headers, None, None).await
    }

    pub async fn get_range(&self, path: &str, range_spec: &str) -> HttpEnvelope {
        self.request(Method::GET, path, &[(RANGE.as_str(), range_spec)], None, None)
            .await
    }

    pub async fn post_json(&self, path: &str, payload: &Value) -> HttpEnvelope {
        let bytes = serde_json::to_vec(payload).expect("serialize JSON");
        self.request(Method::POST, path, &[], Some(bytes), Some("application/json"))
            .await
    }

    pub async fn put_json(&self, path: &str, payload: &Value) -> HttpEnvelope {
        let bytes = serde_json::to_vec(payload).expect("serialize JSON");
        self.request(Method::PUT, path, &[], Some(bytes), Some("application/json"))
            .await
    }

    pub async fn delete(&self, path: &str) -> HttpEnvelope {
        self.request(Method::DELETE, path, &[], None, None).await
    }

    pub async fn delete_json(&self, path: &str, payload: &Value) -> HttpEnvelope {
        let bytes = serde_json::to_vec(payload).expect("serialize JSON");
        self.request(Method::DELETE, path, &[], Some(bytes), Some("application/json"))
            .await
    }

    pub async fn upload_multipart(
        &self,
        path: &str,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> HttpEnvelope {
        let url = format!("{}{}", self.base_url, path);
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(file_name.to_string())
            .mime_str(mime_type)
            .expect("valid mime_str");
        let form = reqwest::multipart::Form::new().part("file", part);
        let mut req = self.http.post(&url).multipart(form);
        if let Some(ref tok) = self.bearer_token {
            req = req.bearer_auth(tok);
        }
        let resp = req.send().await.expect("multipart upload send");
        let status = resp.status();
        let mut map = HashMap::new();
        for (k, v) in resp.headers() {
            if let Ok(s) = v.to_str() {
                map.insert(k.as_str().to_ascii_lowercase(), s.to_string());
            }
        }
        let out = resp.bytes().await.expect("upload response bytes").to_vec();
        HttpEnvelope {
            status,
            headers: map,
            bytes: out,
        }
    }

    pub async fn connect_ws(&self) -> OpaqueWsConnection {
        let token = self.token();
        let ws_base = if let Some(rest) = self.base_url.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.base_url.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            format!("ws://{}", self.base_url)
        };
        let url = format!("{ws_base}/api/v1/ws?token={}", urlencoding::encode(token));
        let (stream, _) = connect_async(&url)
            .await
            .unwrap_or_else(|e| panic!("WebSocket connect to {url} failed: {e}"));
        OpaqueWsConnection { stream }
    }
}

pub struct OpaqueWsConnection {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl OpaqueWsConnection {
    pub async fn send_frame(&mut self, frame: &Value) {
        let text = serde_json::to_string(frame).expect("serialize WS frame");
        self.stream
            .send(WsMessage::Text(text.into()))
            .await
            .expect("send WS text frame");
    }

    pub async fn recv_next_json(&mut self, timeout: Duration) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(remaining, self.stream.next()).await {
                Ok(Some(Ok(WsMessage::Text(txt)))) => {
                    if let Ok(val) = serde_json::from_str::<Value>(&txt) {
                        return Some(val);
                    }
                }
                Ok(Some(Ok(WsMessage::Ping(p)))) => {
                    let _ = self.stream.send(WsMessage::Pong(p)).await;
                }
                Ok(Some(Ok(WsMessage::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => return None,
                Ok(Some(Ok(_))) => continue,
                Err(_) => return None,
            }
        }
        None
    }

    pub async fn recv_until<F>(&mut self, timeout: Duration, mut pred: F) -> Option<Value>
    where
        F: FnMut(&Value) -> bool,
    {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.recv_next_json(remaining).await {
                Some(v) if pred(&v) => return Some(v),
                Some(_) => continue,
                None => return None,
            }
        }
        None
    }

    pub async fn wait_for_disconnect_or_401(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(remaining, self.stream.next()).await {
                Ok(Some(Ok(WsMessage::Text(txt)))) => {
                    if let Ok(val) = serde_json::from_str::<Value>(&txt) {
                        if val["type"] == "Error" && val["payload"]["code"] == 401 {
                            return true;
                        }
                    }
                }
                Ok(Some(Ok(WsMessage::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => return true,
                Ok(Some(Ok(_))) => continue,
                Err(_) => return false,
            }
        }
        false
    }
}

pub struct Ed25519PasskeySigner {
    signing_key: SigningKey,
    pub credential_id: String,
}

impl Ed25519PasskeySigner {
    pub fn generate(label_prefix: &str) -> Self {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let credential_id = unique_tag(label_prefix);
        Self {
            signing_key,
            credential_id,
        }
    }

    pub fn public_key_hex(&self) -> String {
        hex::encode(self.signing_key.verifying_key().to_bytes())
    }

    pub fn sign_challenge_hex(&self, challenge: &str) -> String {
        let sig = self.signing_key.sign(challenge.as_bytes());
        hex::encode(sig.to_bytes())
    }
}
