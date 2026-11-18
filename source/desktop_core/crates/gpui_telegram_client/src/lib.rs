pub mod cache;
pub mod client;
pub mod client_acp;
pub mod client_auth;
pub mod client_dialogs;
pub mod client_dispatch;
pub mod client_e2ee;
pub mod client_features;
pub mod client_messaging;
pub mod config;
pub mod e2ee;
pub mod media_transfer;
pub mod net;
pub mod outbox;
pub mod session_persistence;
pub mod sync;

pub use cache::{CachedMediaEntry, LocalStore};
pub use client::TelegramClient;
pub use client_dispatch::ClientDomainEvent;
pub use config::{ClientConfig, ConfigSource, StoredSession};
pub use e2ee::{
    compute_safety_number_emojis, verify_signed_prekey, DeviceIdentityKeys, DoubleRatchetSession,
    E2eeError, E2eeManager, E2eeStore, SenderKeyState,
};
pub use media_transfer::{
    index_file_chunks, read_chunk_at, ChunkedTransferManager, FileTransferRecord,
    FileTransferStore, TransferControl, TransferProgress, TransferStatus,
};
pub use outbox::{
    compute_base_backoff_ms, compute_jittered_backoff_ms, NetworkState, NetworkStatusListener,
    OutboxMessageRecord, OutboxStatus, OutboxStatusEvent, OutboxStore, OutboxWorker,
};
pub use net::{
    derive_peer_cipher_keys, detect_local_host_info, detect_local_hostname, detect_real_lan_ips,
    execute_local_computer_op, AddContactHttpRequest, AddContactHttpResponse, AgentRunRequest,
    AgentRunResponse, AlipayPollResponse, AlipayStartResponse, AlipayUserDto,
    ApproveComputerCommandRequest, AuthResponse, AuthorizedControllerEntry,
    AuthorizedControllerKind, BlockedUserDto, BotComputerProfileDto, CallInviteRequest,
    CallRecordDto, CallSignalRequest, ChannelStatisticsDto, ChatDetailDto, ChatDetails,
    ChatFolderDto, ChatMemberItem, ChatSummary, ChunkedUploadInitRequest,
    ChunkedUploadStatusResponse, ClientProxyConfig, ClientProxyType, CloudAuthMethod,
    ComputerListResponse,
    ComputerNodeDto, ComputerNodeKind, ComputerRunPolicy, ConnectionStatus, ContactDto,
    CreateChatPayload, CreateChatRequest, CreateChatResponse, CreatePollHttpRequest,
    DailyStatPoint, DerpClientFrame, DerpServerFrame, DiscoPayload, DraftDto,
    EditMessageHttpRequest, EmailRegisterRequest, EmailSendCodeRequest, EmailSendCodeResponse,
    ForumTopicDto, ForumTopicItem, GifResultDto, HistoricalMessage, HistoricalMessageItem,
    HttpClient, HttpClientError, InlineKeyboardButton, InviteLinkDto, LoginRequest,
    MediaUploadCompleteResponse, MediaUploadInitResponse, MemberDto, PeerApiFrame,
    PendingComputerCommandDto, PhoneSendCodeRequest, PhoneSendCodeResponse, PhoneVerifyRequest,
    PinnedMessageDto, PollHttpResponse, ProtoAuthResponse, ProtoUpdateProfileRequest,
    ProxyHealthReport, QrPairAcceptRequest, QrPairInitResponse, QrPairPollResponse,
    ReactionHttpRequest, ReactionHttpResponse, ReadChatHttpRequest, ReadChatHttpResponse,
    RegisterLocalComputerRequest, RegisterRequest, RemoteComputerCommandRequest,
    RemoteComputerCommandResult, RemoteComputerOp, RichSendMessageRequest, RichSendMessageResponse,
    SearchHitDto, SearchResponse, SearchResultItemDto, SendAsPeerDto, SendMessageHttpRequest, SendMessageHttpResponse, SendMessageResponse,
    ServerStatusResponse, SessionDto, StarGiftRequest, StatusResponse, StickerItemDto,
    StickerSetDto, StoryDto, SyncDifferenceResponse, TailnetEndpoint, TailnetEndpointKind,
    TailnetEngine, TailnetIdentity, TailnetNetMap, TailnetNodeStatus, TailnetPeerDescriptor,
    TailnetPeerLiveState, TailnetRegisterRequest, TailnetRelayMode, TailnetStunResponse,
    TwoFactorSetRequest, TwoFactorStatusResponse, TwoFactorVerifyRequest,
    UpdateBotComputerRequest, UpdateComputerPolicyRequest, UpdateProfileRequest,
    UpsertCloudComputerRequest, UserDto, UserMeResponse, UserPrivacySettingsDto, UserProfileDto,
    VotePollHttpRequest, WalletBalanceDto, WalletTransactionDto, WsChannel, WsConnectError,
    WsTransport,
};
pub use session_persistence::{
    AccountSessionRecord, ActiveDeviceSession, PersistedClientState, ProxyEndpointRecord,
    SessionPersistenceBridge, WalletTransactionRecord,
};
pub use sync::{ClientSyncCoordinator, SyncOutcome};

pub mod data;
pub use data::*;
pub mod storage;
pub use storage::*;
pub mod export;
pub use export::*;

#[allow(special_module_name)]
pub mod r#main;
pub use r#main::*;
pub mod dialogs;
pub mod history;
pub use history::*;
pub mod wallet;
