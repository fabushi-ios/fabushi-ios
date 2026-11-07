use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use parking_lot::Mutex;

use gpui_telegram_agent::computer::ComputerLeaseBroker;
use gpui_telegram_agent::mcp::ToolPermissionDock;
use gpui_telegram_agent::talk::TalkCoordinator;
use gpui_telegram_agent::tasks::TaskFsm;
use gpui_telegram_client::client::TelegramClient;
use gpui_telegram_core::auth::GenerationFence;
use fabushi_messaging_core::engine::MessagingEngine;

use crate::dedup::MessageDedupStore;
use crate::events::{BridgeEventSink, TelegramBridgeEventCallback};

pub struct TelegramAppHost {
    pub storage_dir: PathBuf,
    pub storage_passphrase: Option<String>,
    pub tokio_rt: Arc<tokio::runtime::Runtime>,
    pub client: Arc<TelegramClient>,
    pub messaging_engine: Arc<Mutex<MessagingEngine>>,
    pub event_sink: Arc<Mutex<Option<BridgeEventSink>>>,
    pub task_registry: Arc<Mutex<HashMap<String, TaskFsm>>>,
    pub tool_dock: Arc<Mutex<ToolPermissionDock>>,
    pub cua_broker: Arc<Mutex<ComputerLeaseBroker>>,
    pub talk_coordinator: Arc<Mutex<TalkCoordinator>>,
    pub generation_fence: Arc<Mutex<GenerationFence>>,
    pub dedup_store: Arc<Mutex<MessageDedupStore>>,
}

impl TelegramAppHost {
    pub fn new(storage_dir: PathBuf, storage_passphrase: Option<String>) -> Result<Self, String> {
        if let Err(e) = std::fs::create_dir_all(&storage_dir) {
            return Err(format!("Failed to create storage directory {:?}: {e}", storage_dir));
        }

        let tokio_rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| format!("Failed to create Tokio runtime: {e}"))?;

        let client = TelegramClient::new_with_url("https://127.0.0.1:8443".to_string());
        let messaging_engine = MessagingEngine::new();

        Ok(Self {
            storage_dir,
            storage_passphrase,
            tokio_rt: Arc::new(tokio_rt),
            client: Arc::new(client),
            messaging_engine: Arc::new(Mutex::new(messaging_engine)),
            event_sink: Arc::new(Mutex::new(None)),
            task_registry: Arc::new(Mutex::new(HashMap::new())),
            tool_dock: Arc::new(Mutex::new(ToolPermissionDock::new())),
            cua_broker: Arc::new(Mutex::new(ComputerLeaseBroker::new())),
            talk_coordinator: Arc::new(Mutex::new(TalkCoordinator::new())),
            generation_fence: Arc::new(Mutex::new(GenerationFence::new(1))),
            dedup_store: Arc::new(Mutex::new(MessageDedupStore::new())),
        })
    }

    pub fn new_test(storage_dir: PathBuf) -> Result<Self, String> {
        Self::new(storage_dir, None)
    }

    pub fn register_event_callback(
        &self,
        callback: TelegramBridgeEventCallback,
        user_context: *mut std::ffi::c_void,
    ) {
        let mut guard = self.event_sink.lock();
        *guard = Some(BridgeEventSink::new(callback, user_context));
    }

    pub fn unregister_event_callback(&self) {
        let mut guard = self.event_sink.lock();
        *guard = None;
    }

    pub fn emit_event(&self, event_type: &str, payload_json: &str) {
        if let Some(sink) = self.event_sink.lock().as_ref() {
            sink.emit(event_type, payload_json);
        }
    }

    pub fn dispatch(&self, request_json: &str) -> String {
        crate::dispatch::route_command(self, request_json)
    }
}
