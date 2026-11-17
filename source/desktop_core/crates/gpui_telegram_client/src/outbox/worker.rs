use crate::cache::LocalStore;
use crate::net::{HttpClient, HttpClientError, RichSendMessageRequest, RichSendMessageResponse};
use crate::outbox::network::NetworkStatusListener;
use crate::outbox::store::{OutboxMessageRecord, OutboxStatus, OutboxStore};
use crate::sync::ClientSyncCoordinator;
use gpui_telegram_core::error::Result;
use gpui_telegram_core::types::{ChatId, MessageId};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::{broadcast, Mutex as AsyncMutex, Notify};

pub const DEFAULT_OUTBOX_MAX_RETRIES: u32 = 10;

pub fn compute_base_backoff_ms(retry_count: u32) -> u64 {
    let exp = retry_count.min(6);
    500u64.saturating_mul(1u64 << exp).min(30_000)
}

pub fn compute_jittered_backoff_ms(retry_count: u32) -> u64 {
    let base = compute_base_backoff_ms(retry_count);
    let mut rand_bytes = [0u8; 4];
    let _ = getrandom::getrandom(&mut rand_bytes);
    let unit = u32::from_le_bytes(rand_bytes) as f64 / u32::MAX as f64;
    let factor = 0.75 + (unit * 0.50);
    let jittered = ((base as f64) * factor).round() as u64;
    jittered.clamp(250, 30_000)
}

#[derive(Debug, Clone)]
pub struct OutboxStatusEvent {
    pub client_nonce: u64,
    pub chat_id: i64,
    pub status: OutboxStatus,
    pub server_message_id: Option<i64>,
    pub retry_count: u32,
    pub last_error: Option<String>,
}

enum FailureClass {
    Transient(String),
    Permanent(String),
}

fn classify_http_error(err: &HttpClientError) -> FailureClass {
    match err {
        HttpClientError::Conflict(msg) => FailureClass::Permanent(format!("409 Conflict: {msg}")),
        HttpClientError::Api { status, message } => match *status {
            400 | 403 | 404 | 409 | 422 => {
                FailureClass::Permanent(format!("HTTP {status}: {message}"))
            }
            _ => FailureClass::Transient(format!("HTTP {status}: {message}")),
        },
        HttpClientError::Unauthorized(msg) => {
            FailureClass::Transient(format!("Unauthorized: {msg}"))
        }
        HttpClientError::Serialization(e) => {
            FailureClass::Permanent(format!("Serialization error: {e}"))
        }
        HttpClientError::Network(e) => FailureClass::Transient(format!("Network error: {e}")),
        HttpClientError::Proxy(e) => FailureClass::Transient(format!("Proxy error: {e}")),
    }
}

pub type TokenProvider = Arc<dyn Fn() -> Option<String> + Send + Sync>;

pub struct OutboxWorker {
    store: OutboxStore,
    network: Arc<NetworkStatusListener>,
    http_client: HttpClient,
    token_provider: TokenProvider,
    local_store: LocalStore,
    sync_coordinator: ClientSyncCoordinator,
    wake_notify: Arc<Notify>,
    flush_mutex: AsyncMutex<()>,
    max_retries: u32,
    is_running: Arc<RwLock<bool>>,
    events_tx: broadcast::Sender<OutboxStatusEvent>,
}

impl std::fmt::Debug for OutboxWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutboxWorker")
            .field("max_retries", &self.max_retries)
            .field("is_running", &*self.is_running.read())
            .finish()
    }
}

impl OutboxWorker {
    pub fn new(
        store: OutboxStore,
        network: Arc<NetworkStatusListener>,
        http_client: HttpClient,
        token_provider: TokenProvider,
        local_store: LocalStore,
        sync_coordinator: ClientSyncCoordinator,
    ) -> Arc<Self> {
        let (events_tx, _) = broadcast::channel(256);
        Arc::new(Self {
            store,
            network,
            http_client,
            token_provider,
            local_store,
            sync_coordinator,
            wake_notify: Arc::new(Notify::new()),
            flush_mutex: AsyncMutex::new(()),
            max_retries: DEFAULT_OUTBOX_MAX_RETRIES,
            is_running: Arc::new(RwLock::new(false)),
            events_tx,
        })
    }

    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries.max(1);
        self
    }

    pub fn store(&self) -> &OutboxStore {
        &self.store
    }

    pub fn network(&self) -> &Arc<NetworkStatusListener> {
        &self.network
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<OutboxStatusEvent> {
        self.events_tx.subscribe()
    }

    pub fn wake(&self) {
        self.wake_notify.notify_one();
    }

    pub fn enqueue(&self, req: &RichSendMessageRequest, nonce: u64) -> Result<OutboxMessageRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let rec = self.store.enqueue(req, nonce, now_ms)?;
        let _ = self.events_tx.send(OutboxStatusEvent {
            client_nonce: rec.client_nonce,
            chat_id: rec.chat_id,
            status: rec.status,
            server_message_id: rec.server_message_id,
            retry_count: rec.retry_count,
            last_error: rec.last_error.clone(),
        });
        self.wake();
        Ok(rec)
    }

    pub fn recover_in_flight(&self) -> Result<usize> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.store.recover_in_flight(now_ms)
    }

    pub fn start(self: &Arc<Self>, handle: &Handle) {
        let mut running = self.is_running.write();
        if *running {
            return;
        }
        *running = true;
        drop(running);

        let _ = self.recover_in_flight();

        let worker = Arc::clone(self);
        let reconnect_notify = self.network.reconnect_notify();
        let mut epoch_rx = self.network.subscribe_reconnect_epoch();
        let mut last_seen_epoch = self.network.reconnect_epoch();
        let wake_notify = Arc::clone(&self.wake_notify);
        let is_running = Arc::clone(&self.is_running);

        handle.spawn(async move {
            while *is_running.read() {
                let pre_flush_epoch = worker.network.reconnect_epoch();
                if pre_flush_epoch != last_seen_epoch {
                    last_seen_epoch = pre_flush_epoch;
                    let _ = *epoch_rx.borrow_and_update();
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    let _ = worker.store.reset_all_pending_retry_times(now_ms);
                }

                if worker.network.is_online() {
                    let _ = worker.flush_due().await;
                }
                if !*is_running.read() {
                    break;
                }

                let post_flush_epoch = worker.network.reconnect_epoch();
                if post_flush_epoch != last_seen_epoch {
                    last_seen_epoch = post_flush_epoch;
                    let _ = *epoch_rx.borrow_and_update();
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    let _ = worker.store.reset_all_pending_retry_times(now_ms);
                    continue;
                }

                tokio::select! {
                    res = epoch_rx.changed() => {
                        if res.is_ok() {
                            let new_epoch = *epoch_rx.borrow_and_update();
                            if new_epoch != last_seen_epoch {
                                last_seen_epoch = new_epoch;
                                let now_ms = chrono::Utc::now().timestamp_millis();
                                let _ = worker.store.reset_all_pending_retry_times(now_ms);
                            }
                        }
                    }
                    _ = reconnect_notify.notified() => {
                        last_seen_epoch = worker.network.reconnect_epoch();
                        let _ = *epoch_rx.borrow_and_update();
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        let _ = worker.store.reset_all_pending_retry_times(now_ms);
                    }
                    _ = wake_notify.notified() => {}
                    _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                }
            }
        });
    }

    pub fn stop(&self) {
        *self.is_running.write() = false;
        self.wake_notify.notify_waiters();
    }

    pub async fn notify_reconnected_and_flush(&self) -> Result<Vec<OutboxMessageRecord>> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.network.notify_reconnected();
        self.store.reset_all_pending_retry_times(now_ms)?;
        self.flush_due().await
    }

    pub async fn flush_due(&self) -> Result<Vec<OutboxMessageRecord>> {
        let _flush_guard = self.flush_mutex.lock().await;
        let mut processed = Vec::new();
        while self.network.is_online() {
            let now_ms = chrono::Utc::now().timestamp_millis();
            let Some(claimed) = self.store.claim_next_due(now_ms)? else {
                break;
            };
            let outcome = self.dispatch_claimed(&claimed).await;
            processed.push(outcome);
        }
        Ok(processed)
    }

    pub async fn send_or_enqueue_direct(
        &self,
        req: &RichSendMessageRequest,
        nonce: u64,
    ) -> std::result::Result<RichSendMessageResponse, HttpClientError> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        if !self.network.is_online() {
            let _ = self.enqueue(req, nonce);
            return Err(HttpClientError::Api {
                status: 503,
                message: "Client is offline; message queued in persistent outbox".into(),
            });
        }
        let Some(token) = (self.token_provider)().filter(|t| !t.trim().is_empty()) else {
            let _ = self.enqueue(req, nonce);
            return Err(HttpClientError::Unauthorized("No active session token".to_string()));
        };
        let _ = self.store.enqueue_in_flight(req, nonce, now_ms);
        match self.http_client.send_message_rich(&token, req).await {
            Ok(resp) => {
                let done_ms = chrono::Utc::now().timestamp_millis();
                if let Ok(updated) = self.store.mark_delivered(nonce, resp.message_id, done_ms) {
                    self.emit_event(&updated);
                }
                self.apply_delivery_ack(nonce, &resp);
                Ok(resp)
            }
            Err(err) => {
                let fail_ms = chrono::Utc::now().timestamp_millis();
                let _ = self.record_dispatch_error(nonce, 0, &err, fail_ms);
                Err(err)
            }
        }
    }

    async fn dispatch_claimed(&self, record: &OutboxMessageRecord) -> OutboxMessageRecord {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let req = match record.decode_request() {
            Ok(r) => r,
            Err(e) => {
                let updated = self
                    .store
                    .mark_failed(record.client_nonce, &e.to_string(), now_ms)
                    .unwrap_or_else(|_| record.clone());
                self.emit_event(&updated);
                return updated;
            }
        };

        let Some(token) = (self.token_provider)().filter(|t| !t.trim().is_empty()) else {
            let next_retry = now_ms + compute_jittered_backoff_ms(record.retry_count) as i64;
            let updated = self
                .store
                .schedule_retry(
                    record.client_nonce,
                    record.retry_count,
                    next_retry,
                    "Missing auth token",
                    now_ms,
                )
                .unwrap_or_else(|_| record.clone());
            self.emit_event(&updated);
            return updated;
        };

        match self.http_client.send_message_rich(&token, &req).await {
            Ok(resp) => {
                let done_ms = chrono::Utc::now().timestamp_millis();
                let updated = self
                    .store
                    .mark_delivered(record.client_nonce, resp.message_id, done_ms)
                    .unwrap_or_else(|_| record.clone());
                self.apply_delivery_ack(record.client_nonce, &resp);
                self.emit_event(&updated);
                updated
            }
            Err(err) => {
                let fail_ms = chrono::Utc::now().timestamp_millis();
                self.record_dispatch_error(record.client_nonce, record.retry_count, &err, fail_ms)
                    .unwrap_or_else(|_| record.clone())
            }
        }
    }

    fn record_dispatch_error(
        &self,
        client_nonce: u64,
        current_retry_count: u32,
        err: &HttpClientError,
        now_ms: i64,
    ) -> Result<OutboxMessageRecord> {
        let updated = match classify_http_error(err) {
            FailureClass::Permanent(msg) => self.store.mark_failed(client_nonce, &msg, now_ms)?,
            FailureClass::Transient(msg) => {
                let next_retry_count = current_retry_count.saturating_add(1);
                if next_retry_count >= self.max_retries {
                    self.store.mark_failed(
                        client_nonce,
                        &format!("Max retries ({}) exceeded: {msg}", self.max_retries),
                        now_ms,
                    )?
                } else {
                    let delay_ms = compute_jittered_backoff_ms(current_retry_count);
                    let next_retry_at_ms = now_ms.saturating_add(delay_ms as i64);
                    self.store.schedule_retry(
                        client_nonce,
                        next_retry_count,
                        next_retry_at_ms,
                        &msg,
                        now_ms,
                    )?
                }
            }
        };
        self.emit_event(&updated);
        Ok(updated)
    }

    fn apply_delivery_ack(&self, client_nonce: u64, resp: &RichSendMessageResponse) {
        self.local_store
            .ack_client_nonce(client_nonce, MessageId(resp.message_id));
        let cid = ChatId(resp.chat_id);
        if resp.pts > self.sync_coordinator.last_pts(cid) {
            self.sync_coordinator.set_pts(cid, resp.pts);
        }
    }

    fn emit_event(&self, rec: &OutboxMessageRecord) {
        let _ = self.events_tx.send(OutboxStatusEvent {
            client_nonce: rec.client_nonce,
            chat_id: rec.chat_id,
            status: rec.status,
            server_message_id: rec.server_message_id,
            retry_count: rec.retry_count,
            last_error: rec.last_error.clone(),
        });
    }
}
