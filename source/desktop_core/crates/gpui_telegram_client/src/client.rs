use crate::cache::LocalStore;
use crate::client_dispatch::{apply_ws_frame, ClientDomainEvent};
use crate::config::ClientConfig;
use crate::e2ee::{E2eeManager, E2eeStore};
use crate::media_transfer::{ChunkedTransferManager, FileTransferStore};
use crate::net::{
    AuthResponse, BotComputerProfileDto, ClientProxyConfig, ComputerListResponse, ComputerNodeDto,
    ConnectionStatus,
    HermesApprovalDecisionRequest, HermesApprovalDto, HermesCallInviteRequest,
    HermesCallInviteResponse, HermesClient, HermesCommandDescriptor, HermesCommandRequest,
    HermesCommandResponse, HermesCreateKanbanBoardRequest, HermesCreateKanbanCardRequest,
    HermesCronJobDto, HermesHeartbeatConfigDto, HermesHonchoRequest, HermesHonchoStateDto,
    HermesKanbanBoardDto, HermesKanbanCardDto, HermesMemoryMutationRequest, HermesMemoryStateDto,
    HermesMoveKanbanCardRequest, HermesProviderConfigDto, HermesRichChatDto, HermesRichMessageDto,
    HermesRollbackRequest, HermesRollbackResponse, HermesSaveCronRequest,
    HermesSaveProviderRequest, HermesSaveSkillRequest, HermesSessionBranchRequest,
    HermesSessionDetailDto, HermesSessionDto, HermesSessionExportResponse,
    HermesSessionSearchRequest, HermesSkillDto, HermesSoulDto, HermesStatusDto,
    HermesTalkTurnRequest, HermesTalkTurnResponse, HermesTerminalProcessDto, HermesToolsetDto,
    HermesTurnRequest, HermesTurnResponse, HermesUpdateToolsetsRequest, HermesUsageSummaryDto,
    HttpClient, HttpClientError, OpenAiChatCompletionsRequest, PendingComputerCommandDto,
    RegisterLocalComputerRequest, RemoteComputerCommandRequest, RemoteComputerCommandResult,
    RemoteComputerOp, TailnetEngine, TailnetIdentity, TailnetNetMap, TailnetNodeStatus,
    UpdateBotComputerRequest, UpdateComputerPolicyRequest, UpsertCloudComputerRequest, WsChannel,
    WsTransport,
};
use crate::outbox::{NetworkState, NetworkStatusListener, OutboxStore, OutboxWorker, TokenProvider};
use crate::sync::ClientSyncCoordinator;
use gpui_telegram_core::error::Result;
use gpui_telegram_core::types::{ChatId, MessageId, UserId};
use gpui_telegram_proto::message::{MessageContent, MessageItem};
use gpui_telegram_proto::ws::{WsClientFrame, WsServerFrame};
use parking_lot::{Mutex, RwLock};
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;

pub struct TelegramClient {
    pub config: ClientConfig,
    pub local_store: LocalStore,
    pub sync_coordinator: ClientSyncCoordinator,
    pub http_client: HttpClient,
    pub hermes: HermesClient,
    pub ws_channel: WsChannel,
    pub ws_transport: WsTransport,
    pub tailnet: Arc<TailnetEngine>,
    pub outbox: Arc<OutboxWorker>,
    pub network_listener: Arc<NetworkStatusListener>,
    pub chunked_transfers: Arc<ChunkedTransferManager>,
    pub e2ee: Arc<E2eeManager>,
    runtime: Arc<Runtime>,
    auth_session: Arc<RwLock<Option<AuthResponse>>>,
    auth_token: Arc<RwLock<Option<String>>>,
    pending_events: Arc<Mutex<Vec<ClientDomainEvent>>>,
    gap_chats: Arc<Mutex<HashSet<ChatId>>>,
    next_nonce: Arc<AtomicU64>,
    next_client_msg_offset: Arc<AtomicU32>,
    ws_listener_attached: Arc<AtomicBool>,
}

impl TelegramClient {
    pub fn new(config: ClientConfig) -> Self {
        Self::with_store(config, LocalStore::new())
    }

    pub fn open_with_dir(config: ClientConfig, cache_dir: impl AsRef<Path>) -> Result<Self> {
        let store = LocalStore::open_with_dir(cache_dir)?;
        Ok(Self::with_store(config, store))
    }

    fn with_store(config: ClientConfig, local_store: LocalStore) -> Self {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("gpui-tokio-client")
                .enable_all()
                .build()
                .expect("Failed to initialize client Tokio runtime"),
        );

        let tailnet_identity = if let Some(dir) = local_store.cache_dir() {
            TailnetIdentity::load_or_create(&dir)
        } else if std::env::var_os("GPUI_TELEGRAM_CONFIG_DIR")
            .and_then(|v| (!v.to_string_lossy().trim().is_empty()).then_some(v))
            .is_some()
        {
            TailnetIdentity::load_or_create(&ClientConfig::get_app_dir())
        } else {
            TailnetIdentity::ephemeral()
        };

        let handle = runtime.handle().clone();
        let http_client = HttpClient::new_with_proxy(
            config.server_url.clone(),
            Duration::from_secs(config.http_timeout_secs),
            Some(handle.clone()),
            config.proxy.clone(),
        );
        http_client.set_device_id(tailnet_identity.device_id.clone());
        let tailnet = TailnetEngine::new(tailnet_identity);
        let hermes = HermesClient::new(http_client.clone());
        let ws_channel = WsChannel::new();
        let ws_transport = WsTransport::new_with_runtime(
            config.server_url.clone(),
            ws_channel.clone(),
            Some(handle),
        )
        .with_proxy(config.proxy.clone());

        let auth_session: Arc<RwLock<Option<AuthResponse>>> = Arc::new(RwLock::new(None));
        let auth_token: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
        let token_provider: TokenProvider = {
            let tok = Arc::clone(&auth_token);
            let sess = Arc::clone(&auth_session);
            Arc::new(move || {
                tok.read()
                    .clone()
                    .or_else(|| sess.read().as_ref().map(|s| s.token.clone()))
            })
        };
        let device_id_provider = {
            let hc = http_client.clone();
            Arc::new(move || hc.device_id())
        };
        let user_id_provider = {
            let sess = Arc::clone(&auth_session);
            Arc::new(move || sess.read().as_ref().map(|s| s.user_id))
        };

        let sync_coordinator = ClientSyncCoordinator::new();
        let network_listener = NetworkStatusListener::new(NetworkState::Online);

        let sqlite_opt = local_store.sqlite_cache();
        let outbox_store = match sqlite_opt.clone() {
            Some(db) => OutboxStore::from_cache(db),
            None => OutboxStore::in_memory().expect("Failed to open in-memory outbox store"),
        };
        let outbox = OutboxWorker::new(
            outbox_store,
            Arc::clone(&network_listener),
            http_client.clone(),
            Arc::clone(&token_provider),
            local_store.clone(),
            sync_coordinator.clone(),
        );

        let transfer_store = match sqlite_opt.clone() {
            Some(db) => FileTransferStore::from_cache(db),
            None => FileTransferStore::in_memory().expect("Failed to open in-memory transfer store"),
        };
        let chunked_transfers = ChunkedTransferManager::new(
            transfer_store,
            http_client.clone(),
            Arc::clone(&token_provider),
        );

        let e2ee_store = match sqlite_opt {
            Some(db) => E2eeStore::from_cache(db),
            None => E2eeStore::in_memory(),
        };
        let e2ee = E2eeManager::new(
            e2ee_store,
            http_client.clone(),
            token_provider,
            device_id_provider,
            user_id_provider,
        );

        let now_ms = chrono::Utc::now().timestamp_millis().max(1) as u64;
        let persisted_max = local_store.max_outbox_nonce();
        let safe_max = (i64::MAX as u64).saturating_sub(1_000_000);
        let seed_nonce = now_ms
            .min(safe_max)
            .max(persisted_max.saturating_add(1))
            .min(i64::MAX as u64);

        Self {
            config,
            local_store,
            sync_coordinator,
            http_client,
            hermes,
            ws_channel,
            ws_transport,
            tailnet,
            outbox,
            network_listener,
            chunked_transfers,
            e2ee,
            runtime,
            auth_session,
            auth_token,
            pending_events: Arc::new(Mutex::new(Vec::new())),
            gap_chats: Arc::new(Mutex::new(HashSet::new())),
            next_nonce: Arc::new(AtomicU64::new(seed_nonce)),
            next_client_msg_offset: Arc::new(AtomicU32::new(1)),
            ws_listener_attached: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn new_with_url(server_url: String) -> Self {
        let config = ClientConfig {
            server_url,
            ..Default::default()
        };
        Self::new(config)
    }

    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }

    pub fn outbox_worker(&self) -> &Arc<OutboxWorker> {
        &self.outbox
    }

    pub fn chunked_transfer_manager(&self) -> &Arc<ChunkedTransferManager> {
        &self.chunked_transfers
    }

    pub fn e2ee_manager(&self) -> &Arc<E2eeManager> {
        &self.e2ee
    }

    pub fn apply_proxy(
        &self,
        proxy: Option<ClientProxyConfig>,
    ) -> std::result::Result<(), HttpClientError> {
        self.http_client.set_proxy(proxy.clone())?;
        self.ws_transport.set_proxy(proxy);
        Ok(())
    }

    pub fn device_id(&self) -> String {
        self.http_client.device_id()
    }

    pub fn set_device_id(&self, id: impl Into<String>) {
        let trimmed = id.into().trim().to_string();
        if !trimmed.is_empty() {
            self.http_client.set_device_id(&trimmed);
            self.tailnet.set_device_id_override(Some(trimmed));
        }
    }

    pub fn bind_persistent_identity_dir(&self, dir: impl AsRef<Path>) {
        let prev_device_id = self.device_id();
        let was_running = self.tailnet.is_running();
        let id = TailnetIdentity::load_or_create(dir.as_ref());
        let next_device_id = id.device_id.clone();
        self.http_client.set_device_id(next_device_id.clone());
        self.tailnet.bind_identity(id);
        if was_running && prev_device_id != next_device_id {
            self.tailnet.stop();
            self.start_tailnet_mesh();
        }
    }

    pub fn start_tailnet_mesh(&self) {
        if let Some(token) = self.token().filter(|t| !t.trim().is_empty()) {
            self.tailnet.start(
                self.config.server_url.clone(),
                token,
                self.current_user_id(),
                self.http_client.clone(),
                self.runtime.handle().clone(),
            );
        }
    }

    pub fn stop_tailnet_mesh(&self) {
        self.tailnet.stop();
    }

    pub fn tailnet_status(&self) -> TailnetNodeStatus {
        self.tailnet.status()
    }

    pub fn tailnet_pending_approvals(&self) -> Vec<PendingComputerCommandDto> {
        self.tailnet.pending_approvals()
    }

    pub async fn decide_tailnet_approval_async(
        &self,
        command_id: &str,
        approved: bool,
    ) -> Option<std::result::Result<(), String>> {
        self.tailnet.decide_approval(command_id, approved)
    }

    pub fn set_tailnet_force_derp_only(&self, force: bool) {
        self.tailnet.set_force_derp_only(force);
    }

    pub async fn register_tailnet_async(&self) -> Result<TailnetNetMap> {
        let token = self.require_token()?;
        let user_id = match self.current_user_id() {
            Some(uid) if uid > 0 => uid,
            _ => self.http_client.get_me(&token).await?.user_id,
        };
        let req = self.tailnet.build_register_request(user_id);
        let netmap = self.http_client.register_tailnet(&token, &req).await?;
        self.tailnet.apply_netmap(netmap.clone());
        Ok(netmap)
    }

    pub async fn get_tailnet_netmap_async(&self) -> Result<TailnetNetMap> {
        let token = self.require_token()?;
        let netmap = self.http_client.get_tailnet_netmap(&token).await?;
        self.tailnet.apply_netmap(netmap.clone());
        Ok(netmap)
    }

    fn block_on_http<F, T>(&self, fut: F) -> Result<T>
    where
        F: std::future::Future<Output = std::result::Result<T, HttpClientError>> + Send,
        T: Send,
    {
        let res = if tokio::runtime::Handle::try_current().is_ok() {
            let runtime = Arc::clone(&self.runtime);
            std::thread::scope(|scope| scope.spawn(move || runtime.block_on(fut)).join()).map_err(
                |_| {
                    gpui_telegram_core::error::TelegramError::Internal(
                        "Client runtime scope thread panicked".to_string(),
                    )
                },
            )?
        } else {
            self.runtime.block_on(fut)
        };
        res.map_err(Into::into)
    }

    pub fn status(&self) -> ConnectionStatus {
        self.ws_transport.status()
    }

    pub fn is_connected(&self) -> bool {
        self.ws_transport.is_connected()
    }

    pub fn set_auth_session(&self, session: AuthResponse) {
        self.local_store
            .bind_account_scope(&self.config.server_url, session.user_id);
        self.set_token(Some(session.token.clone()));
        *self.auth_session.write() = Some(session);
        self.outbox.start(self.runtime.handle());
    }

    pub fn set_token(&self, token: Option<String>) {
        self.ws_transport.set_token(token.clone());
        *self.auth_token.write() = token;
    }

    pub fn token(&self) -> Option<String> {
        self.auth_token
            .read()
            .clone()
            .or_else(|| self.auth_session.read().as_ref().map(|s| s.token.clone()))
    }

    pub fn current_auth_session(&self) -> Option<AuthResponse> {
        self.auth_session.read().clone()
    }

    pub fn current_user_id(&self) -> Option<u64> {
        self.auth_session.read().as_ref().map(|s| s.user_id)
    }

    pub fn clear_session(&self) {
        self.outbox.stop();
        self.stop_tailnet_mesh();
        self.ws_transport.stop();
        self.ws_transport.set_token(None);
        *self.auth_session.write() = None;
        *self.auth_token.write() = None;
    }

    pub(crate) fn require_token(&self) -> std::result::Result<String, HttpClientError> {
        self.token()
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| HttpClientError::Unauthorized("No active session token".to_string()))
    }

    pub fn connect_ws(&self) {
        if !self.ws_listener_attached.swap(true, Ordering::SeqCst) {
            self.network_listener
                .attach_ws_status(self.ws_transport.subscribe_status(), self.runtime.handle());
        }
        self.outbox.start(self.runtime.handle());
        self.ws_transport.start();
    }

    pub fn disconnect_ws(&self) {
        self.ws_transport.stop();
    }

    pub fn send_ws_frame(&self, frame: WsClientFrame) -> bool {
        self.ws_channel.send(frame)
    }

    pub fn next_client_nonce(&self) -> u64 {
        self.next_nonce.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn next_provisional_msg_id(&self) -> MessageId {
        let offset = self.next_client_msg_offset.fetch_add(1, Ordering::Relaxed);
        gpui_telegram_core::types::MsgId::generate_client(offset)
    }

    pub fn get_messages(&self, chat_id: ChatId) -> Result<Vec<MessageItem>> {
        self.local_store.get_messages(chat_id)
    }

    fn handle_e2ee_side_effects(&self, events: &[ClientDomainEvent]) {
        for ev in events {
            if let ClientDomainEvent::NewMessage(msg) = ev {
                if let MessageContent::Encrypted(ref env) = msg.content {
                    if matches!(
                        env.kind,
                        gpui_telegram_proto::e2ee::EncryptedPayloadKind::SenderKeyDistribution { .. }
                    ) {
                        let _ = self.e2ee.decrypt_envelope(env);
                    }
                }
            }
        }
    }

    pub fn process_ws_frame(&self, frame: WsServerFrame) -> Vec<ClientDomainEvent> {
        let mut events = Vec::new();
        let viewer = self.current_user_id().map(UserId);
        let gaps = apply_ws_frame(
            &self.local_store,
            &self.sync_coordinator,
            viewer,
            frame,
            &mut events,
        );
        self.handle_e2ee_side_effects(&events);
        if !gaps.is_empty() {
            self.gap_chats.lock().extend(gaps);
        }
        if !events.is_empty() {
            self.pending_events.lock().extend(events.iter().cloned());
        }
        events
    }

    pub fn poll_events(&self) -> Vec<ClientDomainEvent> {
        let viewer = self.current_user_id().map(UserId);
        while let Some(frame) = self.ws_channel.try_recv() {
            let mut batch = Vec::new();
            let gaps = apply_ws_frame(
                &self.local_store,
                &self.sync_coordinator,
                viewer,
                frame,
                &mut batch,
            );
            self.handle_e2ee_side_effects(&batch);
            if !gaps.is_empty() {
                self.gap_chats.lock().extend(gaps);
            }
            if !batch.is_empty() {
                self.pending_events.lock().extend(batch);
            }
        }
        std::mem::take(&mut *self.pending_events.lock())
    }

    pub fn pending_gap_chats(&self) -> Vec<ChatId> {
        self.gap_chats.lock().iter().copied().collect()
    }

    pub(crate) fn clear_gap_chat(&self, chat_id: ChatId) {
        self.gap_chats.lock().remove(&chat_id);
    }

    pub fn hermes(&self) -> &HermesClient {
        &self.hermes
    }

    pub async fn hermes_status(&self) -> std::result::Result<HermesStatusDto, HttpClientError> {
        self.hermes_status_for_bot(None).await
    }

    pub async fn hermes_status_for_bot(
        &self,
        bot_id: Option<u64>,
    ) -> std::result::Result<HermesStatusDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.status_for_bot(&token, bot_id).await
    }

    pub async fn hermes_run_turn(
        &self,
        req: &HermesTurnRequest,
    ) -> std::result::Result<HermesTurnResponse, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.run_turn(&token, req).await
    }

    pub async fn hermes_run_command(
        &self,
        req: &HermesCommandRequest,
    ) -> std::result::Result<HermesCommandResponse, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.run_command(&token, req).await
    }

    pub async fn hermes_list_commands(
        &self,
        prefix: Option<&str>,
    ) -> std::result::Result<Vec<HermesCommandDescriptor>, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.list_commands(&token, prefix).await
    }

    pub async fn hermes_list_sessions(
        &self,
    ) -> std::result::Result<Vec<HermesSessionDto>, HttpClientError> {
        self.hermes_list_sessions_for_bot(None).await
    }

    pub async fn hermes_list_sessions_for_bot(
        &self,
        bot_id: Option<u64>,
    ) -> std::result::Result<Vec<HermesSessionDto>, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.list_sessions_for_bot(&token, bot_id).await
    }

    pub async fn hermes_get_session(
        &self,
        session_id: &str,
    ) -> std::result::Result<HermesSessionDetailDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.get_session(&token, session_id).await
    }

    pub async fn hermes_search_sessions(
        &self,
        req: &HermesSessionSearchRequest,
    ) -> std::result::Result<Vec<serde_json::Value>, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.search_sessions(&token, req).await
    }

    pub async fn hermes_branch_session(
        &self,
        session_id: &str,
        req: &HermesSessionBranchRequest,
    ) -> std::result::Result<HermesSessionDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.branch_session(&token, session_id, req).await
    }

    pub async fn hermes_export_session(
        &self,
        session_id: &str,
        format: Option<&str>,
    ) -> std::result::Result<HermesSessionExportResponse, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.export_session(&token, session_id, format).await
    }

    pub async fn hermes_get_memory(
        &self,
    ) -> std::result::Result<HermesMemoryStateDto, HttpClientError> {
        self.hermes_get_memory_for_bot(None).await
    }

    pub async fn hermes_get_memory_for_bot(
        &self,
        bot_id: Option<u64>,
    ) -> std::result::Result<HermesMemoryStateDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.get_memory_for_bot(&token, bot_id).await
    }

    pub async fn hermes_save_memory_for_bot(
        &self,
        bot_id: Option<u64>,
        memory_markdown: &str,
        user_markdown: Option<&str>,
    ) -> std::result::Result<HermesMemoryStateDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes
            .save_memory_for_bot(&token, bot_id, memory_markdown, user_markdown)
            .await
    }

    pub async fn hermes_mutate_memory(
        &self,
        req: &HermesMemoryMutationRequest,
    ) -> std::result::Result<HermesMemoryStateDto, HttpClientError> {
        self.hermes_mutate_memory_for_bot(None, req).await
    }

    pub async fn hermes_mutate_memory_for_bot(
        &self,
        bot_id: Option<u64>,
        req: &HermesMemoryMutationRequest,
    ) -> std::result::Result<HermesMemoryStateDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.mutate_memory_for_bot(&token, bot_id, req).await
    }

    pub async fn hermes_get_soul(
        &self,
        bot_id: Option<u64>,
    ) -> std::result::Result<HermesSoulDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.get_soul(&token, bot_id).await
    }

    pub async fn hermes_save_soul(
        &self,
        soul: &HermesSoulDto,
    ) -> std::result::Result<HermesSoulDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.save_soul(&token, soul).await
    }

    pub async fn hermes_get_honcho(
        &self,
    ) -> std::result::Result<HermesHonchoStateDto, HttpClientError> {
        self.hermes_get_honcho_for_bot(None).await
    }

    pub async fn hermes_get_honcho_for_bot(
        &self,
        bot_id: Option<u64>,
    ) -> std::result::Result<HermesHonchoStateDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.get_honcho_for_bot(&token, bot_id).await
    }

    pub async fn hermes_post_honcho(
        &self,
        req: &HermesHonchoRequest,
    ) -> std::result::Result<HermesHonchoStateDto, HttpClientError> {
        self.hermes_post_honcho_for_bot(None, req).await
    }

    pub async fn hermes_post_honcho_for_bot(
        &self,
        bot_id: Option<u64>,
        req: &HermesHonchoRequest,
    ) -> std::result::Result<HermesHonchoStateDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.post_honcho_for_bot(&token, bot_id, req).await
    }

    pub async fn hermes_list_skills(
        &self,
    ) -> std::result::Result<Vec<HermesSkillDto>, HttpClientError> {
        self.hermes_list_skills_for_bot(None).await
    }

    pub async fn hermes_list_skills_for_bot(
        &self,
        bot_id: Option<u64>,
    ) -> std::result::Result<Vec<HermesSkillDto>, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.list_skills_for_bot(&token, bot_id).await
    }

    pub async fn hermes_save_skill(
        &self,
        req: &HermesSaveSkillRequest,
    ) -> std::result::Result<HermesSkillDto, HttpClientError> {
        self.hermes_save_skill_for_bot(None, req).await
    }

    pub async fn hermes_save_skill_for_bot(
        &self,
        bot_id: Option<u64>,
        req: &HermesSaveSkillRequest,
    ) -> std::result::Result<HermesSkillDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.save_skill_for_bot(&token, bot_id, req).await
    }

    pub async fn hermes_delete_skill(
        &self,
        name: &str,
    ) -> std::result::Result<(), HttpClientError> {
        self.hermes_delete_skill_for_bot(None, name).await
    }

    pub async fn hermes_delete_skill_for_bot(
        &self,
        bot_id: Option<u64>,
        name: &str,
    ) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.hermes.delete_skill_for_bot(&token, bot_id, name).await
    }

    pub async fn hermes_list_toolsets(
        &self,
    ) -> std::result::Result<Vec<HermesToolsetDto>, HttpClientError> {
        self.hermes_list_toolsets_for_bot(None).await
    }

    pub async fn hermes_list_toolsets_for_bot(
        &self,
        bot_id: Option<u64>,
    ) -> std::result::Result<Vec<HermesToolsetDto>, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.list_toolsets_for_bot(&token, bot_id).await
    }

    pub async fn hermes_update_toolsets(
        &self,
        req: &HermesUpdateToolsetsRequest,
    ) -> std::result::Result<Vec<HermesToolsetDto>, HttpClientError> {
        self.hermes_update_toolsets_for_bot(None, req).await
    }

    pub async fn hermes_update_toolsets_for_bot(
        &self,
        bot_id: Option<u64>,
        req: &HermesUpdateToolsetsRequest,
    ) -> std::result::Result<Vec<HermesToolsetDto>, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.update_toolsets_for_bot(&token, bot_id, req).await
    }

    pub async fn hermes_list_approvals(
        &self,
    ) -> std::result::Result<Vec<HermesApprovalDto>, HttpClientError> {
        self.hermes_list_approvals_for_bot(None).await
    }

    pub async fn hermes_list_approvals_for_bot(
        &self,
        bot_id: Option<u64>,
    ) -> std::result::Result<Vec<HermesApprovalDto>, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.list_approvals_for_bot(&token, bot_id).await
    }

    pub async fn hermes_decide_approval(
        &self,
        approval_id: &str,
        req: &HermesApprovalDecisionRequest,
    ) -> std::result::Result<HermesApprovalDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.decide_approval(&token, approval_id, req).await
    }

    pub async fn hermes_list_cron(
        &self,
    ) -> std::result::Result<Vec<HermesCronJobDto>, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.list_cron(&token).await
    }

    pub async fn hermes_save_cron(
        &self,
        req: &HermesSaveCronRequest,
    ) -> std::result::Result<HermesCronJobDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.save_cron(&token, req).await
    }

    pub async fn hermes_delete_cron(
        &self,
        job_id: &str,
    ) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.hermes.delete_cron(&token, job_id).await
    }

    pub async fn hermes_run_cron(
        &self,
        job_id: &str,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.run_cron(&token, job_id).await
    }

    pub async fn hermes_get_heartbeat(
        &self,
    ) -> std::result::Result<HermesHeartbeatConfigDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.get_heartbeat(&token).await
    }

    pub async fn hermes_save_heartbeat(
        &self,
        cfg: &HermesHeartbeatConfigDto,
    ) -> std::result::Result<HermesHeartbeatConfigDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.save_heartbeat(&token, cfg).await
    }

    pub async fn hermes_kanban_boards(
        &self,
    ) -> std::result::Result<Vec<HermesKanbanBoardDto>, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.kanban_boards(&token).await
    }

    pub async fn hermes_kanban_create_board(
        &self,
        req: &HermesCreateKanbanBoardRequest,
    ) -> std::result::Result<HermesKanbanBoardDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.kanban_create_board(&token, req).await
    }

    pub async fn hermes_kanban_create_card(
        &self,
        req: &HermesCreateKanbanCardRequest,
    ) -> std::result::Result<HermesKanbanCardDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.kanban_create_card(&token, req).await
    }

    pub async fn hermes_kanban_move_card(
        &self,
        card_id: &str,
        req: &HermesMoveKanbanCardRequest,
    ) -> std::result::Result<HermesKanbanCardDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.kanban_move_card(&token, card_id, req).await
    }

    pub async fn hermes_usage_stats(
        &self,
    ) -> std::result::Result<HermesUsageSummaryDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.usage_stats(&token).await
    }

    pub async fn hermes_list_providers(
        &self,
    ) -> std::result::Result<Vec<HermesProviderConfigDto>, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.list_providers(&token).await
    }

    pub async fn hermes_save_provider(
        &self,
        req: &HermesSaveProviderRequest,
    ) -> std::result::Result<HermesProviderConfigDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.save_provider(&token, req).await
    }

    pub async fn hermes_rollback_turn(
        &self,
        req: &HermesRollbackRequest,
    ) -> std::result::Result<HermesRollbackResponse, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.rollback_turn(&token, req).await
    }

    pub async fn hermes_terminal_processes(
        &self,
    ) -> std::result::Result<Vec<HermesTerminalProcessDto>, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.terminal_processes(&token).await
    }

    pub async fn hermes_chat_completions(
        &self,
        req: &OpenAiChatCompletionsRequest,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.chat_completions(&token, req).await
    }

    pub async fn hermes_talk_turn(
        &self,
        req: &HermesTalkTurnRequest,
    ) -> std::result::Result<HermesTalkTurnResponse, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.talk_turn(&token, req).await
    }

    pub async fn hermes_call_invite(
        &self,
        req: &HermesCallInviteRequest,
    ) -> std::result::Result<HermesCallInviteResponse, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.call_invite(&token, req).await
    }

    pub async fn hermes_list_chats_rich(
        &self,
    ) -> std::result::Result<Vec<HermesRichChatDto>, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.list_chats_rich(&token).await
    }

    pub async fn hermes_get_chat_rich(
        &self,
        chat_id: i64,
    ) -> std::result::Result<HermesRichChatDto, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.get_chat_rich(&token, chat_id).await
    }

    pub async fn hermes_messages_history_rich(
        &self,
        chat_id: i64,
        limit: Option<usize>,
    ) -> std::result::Result<Vec<HermesRichMessageDto>, HttpClientError> {
        let token = self.require_token()?;
        self.hermes.messages_history_rich(&token, chat_id, limit).await
    }

    pub fn list_computers(&self) -> Result<ComputerListResponse> {
        let token = self.require_token()?;
        let http = self.http_client.clone();
        self.block_on_http(async move { http.list_computers(&token).await })
    }

    pub async fn list_computers_async(&self) -> Result<ComputerListResponse> {
        let token = self.require_token()?;
        Ok(self.http_client.list_computers(&token).await?)
    }

    pub fn register_local_computer(
        &self,
        req: &RegisterLocalComputerRequest,
    ) -> Result<ComputerNodeDto> {
        let token = self.require_token()?;
        let http = self.http_client.clone();
        self.block_on_http(async move { http.register_local_computer(&token, req).await })
    }

    pub async fn register_local_computer_async(
        &self,
        req: &RegisterLocalComputerRequest,
    ) -> Result<ComputerNodeDto> {
        let token = self.require_token()?;
        Ok(self.http_client.register_local_computer(&token, req).await?)
    }

    pub fn register_current_host_computer(&self) -> Result<ComputerNodeDto> {
        let token = self.require_token()?;
        let http = self.http_client.clone();
        self.block_on_http(async move { http.register_current_host_computer(&token).await })
    }

    pub async fn register_current_host_computer_async(&self) -> Result<ComputerNodeDto> {
        let token = self.require_token()?;
        Ok(self
            .http_client
            .register_current_host_computer(&token)
            .await?)
    }

    pub fn poll_local_computer_commands(&self) -> Result<Vec<PendingComputerCommandDto>> {
        let token = self.require_token()?;
        let http = self.http_client.clone();
        self.block_on_http(async move { http.poll_local_computer_commands(&token).await })
    }

    pub async fn poll_local_computer_commands_async(
        &self,
    ) -> Result<Vec<PendingComputerCommandDto>> {
        let token = self.require_token()?;
        Ok(self
            .http_client
            .poll_local_computer_commands(&token)
            .await?)
    }

    pub fn process_ready_local_computer_commands(&self) -> Result<Vec<PendingComputerCommandDto>> {
        let token = self.require_token()?;
        let http = self.http_client.clone();
        self.block_on_http(async move { http.process_ready_local_computer_commands(&token).await })
    }

    pub async fn process_ready_local_computer_commands_async(
        &self,
    ) -> Result<Vec<PendingComputerCommandDto>> {
        let token = self.require_token()?;
        Ok(self
            .http_client
            .process_ready_local_computer_commands(&token)
            .await?)
    }

    pub fn approve_computer_command(
        &self,
        command_id: &str,
        approved: bool,
    ) -> Result<PendingComputerCommandDto> {
        let token = self.require_token()?;
        let http = self.http_client.clone();
        self.block_on_http(async move {
            http.approve_computer_command(&token, command_id, approved)
                .await
        })
    }

    pub async fn approve_computer_command_async(
        &self,
        command_id: &str,
        approved: bool,
    ) -> Result<PendingComputerCommandDto> {
        let token = self.require_token()?;
        Ok(self
            .http_client
            .approve_computer_command(&token, command_id, approved)
            .await?)
    }

    pub fn complete_computer_command(
        &self,
        command_id: &str,
        result: &RemoteComputerCommandResult,
    ) -> Result<()> {
        let token = self.require_token()?;
        let http = self.http_client.clone();
        self.block_on_http(async move {
            http.complete_computer_command(&token, command_id, result)
                .await
        })
    }

    pub async fn complete_computer_command_async(
        &self,
        command_id: &str,
        result: &RemoteComputerCommandResult,
    ) -> Result<()> {
        let token = self.require_token()?;
        Ok(self
            .http_client
            .complete_computer_command(&token, command_id, result)
            .await?)
    }

    pub fn upsert_cloud_computer(
        &self,
        req: &UpsertCloudComputerRequest,
    ) -> Result<ComputerNodeDto> {
        let token = self.require_token()?;
        let http = self.http_client.clone();
        self.block_on_http(async move { http.upsert_cloud_computer(&token, req).await })
    }

    pub async fn upsert_cloud_computer_async(
        &self,
        req: &UpsertCloudComputerRequest,
    ) -> Result<ComputerNodeDto> {
        let token = self.require_token()?;
        Ok(self.http_client.upsert_cloud_computer(&token, req).await?)
    }

    pub fn update_computer_policy(
        &self,
        node_id: &str,
        req: &UpdateComputerPolicyRequest,
    ) -> Result<ComputerNodeDto> {
        let token = self.require_token()?;
        let http = self.http_client.clone();
        let updated = self
            .block_on_http(async move { http.update_computer_policy(&token, node_id, req).await })?;
        self.tailnet.sync_node_policy_from_dto(&updated);
        Ok(updated)
    }

    pub async fn update_computer_policy_async(
        &self,
        node_id: &str,
        req: &UpdateComputerPolicyRequest,
    ) -> Result<ComputerNodeDto> {
        let token = self.require_token()?;
        let updated = self
            .http_client
            .update_computer_policy(&token, node_id, req)
            .await?;
        self.tailnet.sync_node_policy_from_dto(&updated);
        Ok(updated)
    }

    pub fn delete_computer(&self, node_id: &str) -> Result<()> {
        let token = self.require_token()?;
        let http = self.http_client.clone();
        self.block_on_http(async move { http.delete_computer(&token, node_id).await })
    }

    pub async fn delete_computer_async(&self, node_id: &str) -> Result<()> {
        let token = self.require_token()?;
        Ok(self.http_client.delete_computer(&token, node_id).await?)
    }

    pub fn test_computer_connection(
        &self,
        node_id: &str,
    ) -> Result<RemoteComputerCommandResult> {
        let token = self.require_token()?;
        let http = self.http_client.clone();
        self.block_on_http(async move { http.test_computer_connection(&token, node_id).await })
    }

    pub async fn test_computer_connection_async(
        &self,
        node_id: &str,
    ) -> Result<RemoteComputerCommandResult> {
        let token = self.require_token()?;
        let p2p_req = RemoteComputerCommandRequest {
            node_id: node_id.to_string(),
            caller_device_id: Some(self.device_id()),
            caller_bot_id: None,
            caller_label: format!("Device {}", self.device_id()),
            operation: RemoteComputerOp::TestConnection,
        };
        if let Some(res) = self.tailnet.execute_peer_op(&p2p_req).await {
            return Ok(res);
        }
        Ok(self
            .http_client
            .test_computer_connection(&token, node_id)
            .await?)
    }

    pub fn execute_remote_computer_op(
        &self,
        req: &RemoteComputerCommandRequest,
    ) -> Result<RemoteComputerCommandResult> {
        let token = self.require_token()?;
        let http = self.http_client.clone();
        self.block_on_http(async move { http.execute_remote_computer_op(&token, req).await })
    }

    pub async fn execute_remote_computer_op_async(
        &self,
        req: &RemoteComputerCommandRequest,
    ) -> Result<RemoteComputerCommandResult> {
        let token = self.require_token()?;
        if let Some(res) = self.tailnet.execute_peer_op(req).await {
            return Ok(res);
        }
        Ok(self
            .http_client
            .execute_remote_computer_op(&token, req)
            .await?)
    }

    pub fn get_bot_computer_profile(&self, bot_id: u64) -> Result<BotComputerProfileDto> {
        let token = self.require_token()?;
        let http = self.http_client.clone();
        self.block_on_http(async move { http.get_bot_computer_profile(&token, bot_id).await })
    }

    pub async fn get_bot_computer_profile_async(
        &self,
        bot_id: u64,
    ) -> Result<BotComputerProfileDto> {
        let token = self.require_token()?;
        Ok(self
            .http_client
            .get_bot_computer_profile(&token, bot_id)
            .await?)
    }

    pub fn update_bot_computer_profile(
        &self,
        bot_id: u64,
        req: &UpdateBotComputerRequest,
    ) -> Result<BotComputerProfileDto> {
        let token = self.require_token()?;
        let http = self.http_client.clone();
        self.block_on_http(async move { http.update_bot_computer_profile(&token, bot_id, req).await })
    }

    pub async fn update_bot_computer_profile_async(
        &self,
        bot_id: u64,
        req: &UpdateBotComputerRequest,
    ) -> Result<BotComputerProfileDto> {
        let token = self.require_token()?;
        Ok(self
            .http_client
            .update_bot_computer_profile(&token, bot_id, req)
            .await?)
    }
}
