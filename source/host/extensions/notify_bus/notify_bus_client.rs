use reqwest::Client;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::runtime::Builder;
use tokio::sync::watch;
use url::Url;

pub const HEALTHY_CONNECTION_MIN_LIFETIME_MS: u64 = 30_000;
pub const RECONNECT_INITIAL_DELAY_MS: u64 = 1_000;
pub const RECONNECT_MAX_DELAY_MS: u64 = 60_000;
pub const NOTIFY_STREAM_STALL_MS: u64 = 35_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum SandNotifyTopic {
    AutomationFires,
    ListenerEvents,
    XuserEvents,
}

impl SandNotifyTopic {
    pub const ALL: [Self; 3] = [
        Self::AutomationFires,
        Self::ListenerEvents,
        Self::XuserEvents,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AutomationFires => "automation-fires",
            Self::ListenerEvents => "listener-events",
            Self::XuserEvents => "xuser-events",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "automation-fires" => Some(Self::AutomationFires),
            "listener-events" => Some(Self::ListenerEvents),
            "xuser-events" => Some(Self::XuserEvents),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandNotifyFrame {
    Connected,
    Notify(SandNotifyTopic),
    Ignored,
}

pub fn parse_notify_frame(frame: &str) -> SandNotifyFrame {
    let mut data = None;
    for line in frame.split('\n') {
        if let Some(raw) = line.strip_prefix("data: ") {
            data = Some(raw);
        }
    }
    let Some(raw) = data else { return SandNotifyFrame::Ignored };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return SandNotifyFrame::Ignored;
    };
    if value.get("kind").and_then(serde_json::Value::as_str) == Some("connected") {
        return SandNotifyFrame::Connected;
    }
    if value.get("kind").and_then(serde_json::Value::as_str) == Some("notify") {
        if let Some(topic) = value
            .get("topic")
            .and_then(serde_json::Value::as_str)
            .and_then(SandNotifyTopic::parse)
        {
            return SandNotifyFrame::Notify(topic);
        }
    }
    SandNotifyFrame::Ignored
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyBusTiming {
    pub healthy_connection_min_lifetime_ms: u64,
    pub reconnect_initial_delay_ms: u64,
    pub reconnect_max_delay_ms: u64,
    pub stall_ms: u64,
}

impl Default for NotifyBusTiming {
    fn default() -> Self {
        Self {
            healthy_connection_min_lifetime_ms: HEALTHY_CONNECTION_MIN_LIFETIME_MS,
            reconnect_initial_delay_ms: RECONNECT_INITIAL_DELAY_MS,
            reconnect_max_delay_ms: RECONNECT_MAX_DELAY_MS,
            stall_ms: NOTIFY_STREAM_STALL_MS,
        }
    }
}

pub fn reconnect_delay_ms(attempt: u32, timing: &NotifyBusTiming) -> u64 {
    if attempt == 0 {
        return 0;
    }
    let exponent = attempt.saturating_sub(1).min(62);
    timing
        .reconnect_initial_delay_ms
        .saturating_mul(1_u64 << exponent)
        .min(timing.reconnect_max_delay_ms)
}

fn next_reconnect_attempt(
    connected_at_ms: Option<u64>,
    now_ms: u64,
    attempt: u32,
    timing: &NotifyBusTiming,
) -> u32 {
    if connected_at_ms.is_some_and(|at| {
        now_ms.saturating_sub(at) >= timing.healthy_connection_min_lifetime_ms
    }) {
        0
    } else {
        attempt.saturating_add(1)
    }
}

#[derive(Default)]
struct SseFrameDecoder {
    buffer: Vec<u8>,
}

impl SseFrameDecoder {
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut output = Vec::new();
        loop {
            let Some(boundary) = self.buffer.windows(2).position(|window| window == b"\n\n") else {
                break;
            };
            let frame = self.buffer.drain(..boundary).collect::<Vec<_>>();
            self.buffer.drain(..2);
            output.push(String::from_utf8_lossy(&frame).into_owned());
        }
        output
    }
}

pub struct NotifyBusClientDependencies {
    pub backend_url: String,
    pub access_token: String,
    pub client_type: String,
    pub client_version: String,
    pub namespace: String,
    pub on_connected: Arc<dyn Fn() + Send + Sync>,
    pub on_notify: Arc<dyn Fn(SandNotifyTopic) + Send + Sync>,
    pub on_stream_error: Arc<dyn Fn(String) + Send + Sync>,
    pub now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub timing: NotifyBusTiming,
}

impl NotifyBusClientDependencies {
    pub fn production(
        backend_url: String,
        access_token: String,
        client_type: String,
        client_version: String,
        namespace: String,
        on_connected: Arc<dyn Fn() + Send + Sync>,
        on_notify: Arc<dyn Fn(SandNotifyTopic) + Send + Sync>,
        on_stream_error: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Self {
        Self {
            backend_url,
            access_token,
            client_type,
            client_version,
            namespace,
            on_connected,
            on_notify,
            on_stream_error,
            now_ms: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .min(u64::MAX as u128) as u64
            }),
            timing: NotifyBusTiming::default(),
        }
    }
}

#[derive(Default)]
struct RuntimeState {
    connected_at_ms: Option<u64>,
}

pub struct SandNotifyBusClient {
    deps: Arc<NotifyBusClientDependencies>,
    http: Client,
    state: Arc<Mutex<RuntimeState>>,
    stop_tx: Mutex<Option<watch::Sender<bool>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl SandNotifyBusClient {
    pub fn new(deps: NotifyBusClientDependencies) -> Result<Self, String> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(|error| format!("build notify client: {error}"))?;
        Ok(Self {
            deps: Arc::new(deps),
            http,
            state: Arc::new(Mutex::new(RuntimeState::default())),
            stop_tx: Mutex::new(None),
            worker: Mutex::new(None),
        })
    }

    pub fn is_connected(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .connected_at_ms
            .is_some()
    }

    pub fn start(&self) {
        let mut slot = self.stop_tx.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_some() {
            return;
        }
        let (tx, rx) = watch::channel(false);
        *slot = Some(tx);
        drop(slot);
        let deps = Arc::clone(&self.deps);
        let http = self.http.clone();
        let state = Arc::clone(&self.state);
        if let Ok(worker) = thread::Builder::new()
            .name("ios-sand-notify-bus".into())
            .spawn(move || {
                match Builder::new_current_thread().enable_all().build() {
                    Ok(runtime) => runtime.block_on(run_loop(deps, http, state, rx)),
                    Err(error) => {
                        (deps.on_stream_error)(format!("notify runtime: {error}"));
                    }
                }
            })
        {
            *self.worker.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(worker);
        }
    }

    pub fn stop(&self) {
        if let Some(tx) = self.stop_tx.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() {
            let _ = tx.send(true);
        }
        if let Some(worker) = self.worker.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() {
            let _ = worker.join();
        }
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .connected_at_ms = None;
    }
}

impl Drop for SandNotifyBusClient {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn run_loop(
    deps: Arc<NotifyBusClientDependencies>,
    http: Client,
    state: Arc<Mutex<RuntimeState>>,
    mut cancel: watch::Receiver<bool>,
) {
    let mut attempt = 0_u32;
    while !*cancel.borrow() {
        let result = stream_once(&deps, &http, &state, &mut cancel).await;
        if !*cancel.borrow() {
            if let Err(error) = result {
                (deps.on_stream_error)(error);
            }
        }
        if *cancel.borrow() {
            break;
        }
        let now = (deps.now_ms)();
        let connected_at = {
            let mut guard = state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.connected_at_ms.take()
        };
        attempt = next_reconnect_attempt(connected_at, now, attempt, &deps.timing);
        if attempt > 0 {
            let delay = Duration::from_millis(reconnect_delay_ms(attempt, &deps.timing));
            tokio::select! {
                _ = cancel.changed() => {},
                _ = tokio::time::sleep(delay) => {},
            }
        }
    }
}

async fn stream_once(
    deps: &NotifyBusClientDependencies,
    http: &Client,
    state: &Mutex<RuntimeState>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let base = Url::parse(&deps.backend_url)
        .map_err(|error| format!("invalid notify backend: {error}"))?;
    let url = base
        .join("/sand/notify")
        .map_err(|error| format!("invalid notify URL: {error}"))?;
    let request = http
        .get(url)
        .bearer_auth(&deps.access_token)
        .header("accept", "text/event-stream")
        .header("x-cursor-client-type", &deps.client_type)
        .header("x-cursor-client-version", &deps.client_version)
        .header("x-sand-box-namespace", &deps.namespace)
        .send();
    let mut response = tokio::select! {
        _ = cancel.changed() => return Ok(()),
        result = tokio::time::timeout(Duration::from_millis(deps.timing.stall_ms), request) => {
            result.map_err(|_| "sand notify stream stalled during connect".to_string())?
                .map_err(|error| format!("sand notify transport failed: {error}"))?
        }
    };
    if !response.status().is_success() {
        return Err(format!("sand notify stream failed: {}", response.status().as_u16()));
    }
    let mut decoder = SseFrameDecoder::default();
    loop {
        let next = tokio::select! {
            _ = cancel.changed() => return Ok(()),
            result = tokio::time::timeout(
                Duration::from_millis(deps.timing.stall_ms),
                response.chunk()
            ) => {
                result.map_err(|_| "sand notify stream stalled".to_string())?
                    .map_err(|error| format!("sand notify transport failed: {error}"))?
            }
        };
        let Some(chunk) = next else {
            break;
        };
        for frame in decoder.push(&chunk) {
            match parse_notify_frame(&frame) {
                SandNotifyFrame::Connected => {
                    state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).connected_at_ms =
                        Some((deps.now_ms)());
                    (deps.on_connected)();
                }
                SandNotifyFrame::Notify(topic) => (deps.on_notify)(topic),
                SandNotifyFrame::Ignored => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_frames_and_topics_match_desktop_contract() {
        assert_eq!(
            parse_notify_frame("event: message\ndata: {\"kind\":\"connected\"}"),
            SandNotifyFrame::Connected
        );
        assert_eq!(
            parse_notify_frame("data: {\"kind\":\"notify\",\"topic\":\"xuser-events\"}"),
            SandNotifyFrame::Notify(SandNotifyTopic::XuserEvents)
        );
        assert_eq!(
            parse_notify_frame("data: {\"kind\":\"notify\",\"topic\":\"unknown\"}"),
            SandNotifyFrame::Ignored
        );
    }

    #[test]
    fn notify_reconnect_backoff_matches_desktop_frozen_contract() {
        let timing = NotifyBusTiming::default();
        assert_eq!(reconnect_delay_ms(0, &timing), 0);
        assert_eq!(reconnect_delay_ms(1, &timing), 1_000);
        assert_eq!(reconnect_delay_ms(2, &timing), 2_000);
        assert_eq!(reconnect_delay_ms(7, &timing), 60_000);
        assert_eq!(NOTIFY_STREAM_STALL_MS, 35_000);
        assert_eq!(HEALTHY_CONNECTION_MIN_LIFETIME_MS, 30_000);
    }
}
