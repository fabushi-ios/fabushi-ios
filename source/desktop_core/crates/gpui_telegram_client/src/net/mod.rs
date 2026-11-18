pub mod api_acp;
pub mod api_auth;
pub mod api_chats;
pub mod api_computers;
pub mod api_e2ee;
pub mod api_features;
pub mod api_miniapps;
pub mod hermes_client;
pub mod http_client;
pub mod http_helpers;
pub mod proxy;
pub mod tailnet;
pub mod ws_channel;
pub mod ws_transport;

pub use api_chats::SendAsPeerDto;
pub use proxy::{ClientProxyConfig, ClientProxyType, ProxyHealthReport};

pub use api_computers::{
    detect_local_host_info, detect_local_hostname, detect_real_lan_ips, execute_local_computer_op,
};
pub use gpui_telegram_proto::tailnet_contracts::{
    DerpClientFrame, DerpServerFrame, DiscoPayload, PeerApiFrame, TailnetEndpoint,
    TailnetEndpointKind, TailnetNetMap, TailnetNodeStatus, TailnetPeerDescriptor,
    TailnetPeerLiveState, TailnetRegisterRequest, TailnetRelayMode, TailnetStunResponse,
};
pub use tailnet::{derive_peer_cipher_keys, TailnetEngine, TailnetIdentity};
pub use api_miniapps::{
    compute_telegram_init_data_hash, parse_init_data_check_string,
    verify_telegram_init_data_hmac, GlobalCatalogSearchResponse,
};
pub use gpui_telegram_proto::computer_contracts::{
    ApproveComputerCommandRequest, AuthorizedControllerEntry, AuthorizedControllerKind,
    BotComputerProfileDto, CloudAuthMethod, ComputerListResponse, ComputerNodeDto,
    ComputerNodeKind, ComputerRunPolicy, PendingComputerCommandDto, RegisterLocalComputerRequest,
    RemoteComputerCommandRequest, RemoteComputerCommandResult, RemoteComputerOp,
    UpdateBotComputerRequest, UpdateComputerPolicyRequest, UpsertCloudComputerRequest,
};
pub use gpui_telegram_proto::miniapp_contracts::{
    BotCommandSpec, BotRegistryEntry, BotTokenResponse, ChatCallbackRequest, CloudStorageItem,
    CreateBotRequest, CreateMiniAppRequest, InstallMiniAppRequest, LaunchMiniAppRequest,
    MiniAppActionButton, MiniAppActionKind, MiniAppActionRequest, MiniAppActionResponse,
    MiniAppEntry, MiniAppInitDataDto, MiniAppLaunchContext, MiniAppLaunchMode,
    MiniAppManifestDto, MiniAppThemeParams, MiniAppUiSchema, MiniAppUiWidget, MiniAppWidgetType,
    NaturalLanguageCreateRequest, NaturalLanguageCreateResponse, PeerBotActionResult,
    ValidateInitDataRequest, ValidateInitDataResponse,
};

pub use gpui_telegram_proto::contracts::{
    AuthResponse as ProtoAuthResponse, BlockedUserDto, CallInviteRequest, CallRecordDto,
    CallSignalRequest, ChannelStatisticsDto, ChatDetailDto, ChatFolderDto, ChunkedUploadInitRequest,
    ChunkedUploadStatusResponse, ContactDto,
    CreateChatRequest, CreatePollHttpRequest, DailyStatPoint, DraftDto, EditMessageHttpRequest,
    ForumTopicDto, GifResultDto, InlineKeyboardButton, InviteLinkDto, MediaUploadCompleteResponse,
    MediaUploadInitResponse, MemberDto, PhoneSendCodeRequest, PhoneSendCodeResponse,
    PhoneVerifyRequest, PinnedMessageDto, PollHttpResponse, QrPairAcceptRequest,
    QrPairInitResponse, QrPairPollResponse, ReactionHttpRequest, ReactionHttpResponse,
    ReadChatHttpRequest, ReadChatHttpResponse, SearchHitDto, SearchResponse,
    SendMessageHttpRequest as RichSendMessageRequest,
    SendMessageHttpResponse as RichSendMessageResponse, SessionDto, StarGiftRequest,
    StatusResponse, StickerItemDto, StickerSetDto, StoryDto, TwoFactorSetRequest,
    TwoFactorStatusResponse, TwoFactorVerifyRequest,
    UpdateProfileRequest as ProtoUpdateProfileRequest, UserPrivacySettingsDto, UserProfileDto,
    VotePollHttpRequest, WalletBalanceDto, WalletTransactionDto,
};
pub use http_client::{
    AddContactHttpRequest, AddContactHttpResponse, AgentRunRequest, AgentRunResponse,
    AlipayPollResponse, AlipayStartResponse, AlipayUserDto, AuthResponse, ChatDetails,
    ChatMemberItem, ChatSummary, CreateChatPayload, CreateChatResponse, EmailRegisterRequest,
    EmailSendCodeRequest, EmailSendCodeResponse, ForumTopicItem, HistoricalMessage,
    HistoricalMessageItem, HttpClient, HttpClientError, LoginRequest, RegisterRequest,
    SearchResultItemDto, SendMessageHttpRequest, SendMessageHttpResponse, SendMessageResponse,
    ServerStatusResponse, SyncDifferenceResponse, UpdateProfileRequest, UserDto, UserMeResponse,
};
pub use hermes_client::{
    HermesApprovalDecisionRequest, HermesApprovalDto, HermesCallInviteRequest,
    HermesCallInviteResponse, HermesClient, HermesCommandDescriptor, HermesCommandRequest,
    HermesCommandResponse, HermesCreateKanbanBoardRequest, HermesCreateKanbanCardRequest,
    HermesCronJobDto, HermesHeartbeatConfigDto, HermesHonchoRequest, HermesHonchoStateDto,
    HermesInlineButtonDto, HermesKanbanBoardDto, HermesKanbanCardDto, HermesMemoryEntryDto,
    HermesMemoryMutationRequest, HermesMemoryStateDto, HermesMoveKanbanCardRequest,
    HermesProviderConfigDto, HermesRichChatDto, HermesRichMessageDto, HermesRollbackRequest,
    HermesRollbackResponse, HermesSaveCronRequest, HermesSaveProviderRequest,
    HermesSaveSkillRequest, HermesSessionBranchRequest, HermesSessionDetailDto,
    HermesSessionDto, HermesSessionExportResponse, HermesSessionSearchRequest, HermesSkillDto,
    HermesSoulDto, HermesStatusDto, HermesTalkTurnRequest, HermesTalkTurnResponse,
    HermesTerminalProcessDto, HermesToolsetDto, HermesTurnRequest, HermesTurnResponse,
    HermesUpdateToolsetsRequest, HermesUsageSummaryDto, MagpieDeleteProviderResponse,
    MagpieGroupsRequest, MagpieGroupsResponse, MagpieImportRequest, MagpieImportResponse,
    MagpieMaskingConfigDto, MagpieMaskingHitDto, MagpieMaskingPreviewRequest,
    MagpieMaskingPreviewResponse, MagpieMaskingUpdateResponse, MagpiePresetDto,
    MagpiePresetRegionDto, MagpieProbeRequest, MagpieProbeResponse, MagpieProviderDto,
    MagpieRouteTraceDto, MagpieRoutingGroupDto, MagpieRoutingGroupMemberDto,
    MagpieSaveProviderRequest, MagpieSaveProviderResponse, MagpieStateDto, MagpieSubscriptionDto,
    MagpieSubscriptionsScanResponse, OpenAiChatCompletionsRequest, OpenAiChatMessage,
};
pub use ws_channel::WsChannel;
pub use ws_transport::{connect_ws_with_proxy, ConnectionStatus, WsConnectError, WsTransport};
