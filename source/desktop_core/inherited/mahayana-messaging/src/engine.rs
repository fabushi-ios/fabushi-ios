use crate::actor::{Actor, ActorId, ActorKind, Participant, ParticipantRole, Presence, PresenceStatus};
use crate::bot::{BotExecution, BotInvocation, BotProfile, BotRegistry};
use crate::community::{
    AdminRights, CommunityError, CommunityMember,
    CommunityState, ForumTopicState, InviteLink, JoinRequest, MemberStatus,
};
use crate::conversation::{
    Conversation, ConversationChildIdentity, ConversationChildRuntimeState,
    ConversationDestination, ConversationDraft, ConversationFolder, ConversationId,
    ConversationKind, ConversationMessagePosition, NotificationSettings, TopicDraft,
};
use crate::message::{
    ClientMessageId, DeliveryState, ForwardPrivacy, Message, MessageContent, MessageId,
    PendingPresenceSend, PresenceSendTrigger, ReactionSummary,
};
use crate::miniapp::{
    authorize_miniapp_request, MiniAppAction, MiniAppDenial, MiniAppGrant, MiniAppManifest,
    MiniAppPermission, MiniAppRequest, MiniAppResponse, MiniAppSession,
};
use crate::payment::{CustomerInfo, Invoice, Money, PaymentOrder, PaymentStatus};
use crate::privacy::{PrivacyKey, PrivacySettings};
use crate::search::{
    authorize_destination, DestinationAuthorizationDenial, RecipientAuthorizationInput,
    RecipientContentKind, RecipientSearchRequirements,
};
use crate::story::{Story, StoryError, StoryId};
use crate::wallet::{LedgerEntry, WalletAccountId, WalletError, WalletLedger};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub use crate::engine_tdrp_dialogs::*;
pub use crate::engine_tdrp_media_stories::*;
pub use crate::engine_tdrp_moderation::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum Command {
    UpsertActor {
        actor: Actor,
    },
    UpsertPrivacySettings {
        actor_id: ActorId,
        settings: PrivacySettings,
    },
    SetPresence {
        actor_id: ActorId,
        presence: Presence,
    },
    UpsertConversation {
        conversation: Conversation,
    },
    UpdateConversationInfo {
        conversation_id: ConversationId,
        title: String,
        description: Option<String>,
    },
    SetConversationParticipant {
        conversation_id: ConversationId,
        participant: Participant,
    },
    RemoveConversationParticipant {
        conversation_id: ConversationId,
        actor_id: ActorId,
    },
    ArchiveConversation {
        conversation_id: ConversationId,
        archived: bool,
    },
    PinConversation {
        conversation_id: ConversationId,
        pinned: bool,
    },
    SetMarkedUnread {
        conversation_id: ConversationId,
        actor_id: ActorId,
        marked_unread: bool,
    },
    SetDraft {
        draft: ConversationDraft,
    },
    SetConversationNotifications {
        conversation_id: ConversationId,
        settings: NotificationSettings,
    },
    UpsertFolder {
        folder: ConversationFolder,
    },
    DeleteFolder {
        folder_id: String,
    },
    QueueMessage {
        conversation_id: ConversationId,
        local_message_id: MessageId,
        client_message_id: ClientMessageId,
        sender_id: ActorId,
        content: MessageContent,
        reply_to_message_id: Option<MessageId>,
        thread_root_message_id: Option<MessageId>,
        created_at_ms: i64,
        scheduled_at_ms: Option<i64>,
        silent: bool,
        protected_content: bool,
    },
    QueuePresenceTriggeredSend {
        pending: PendingPresenceSend,
    },
    CancelPresenceTriggeredSend {
        actor_id: ActorId,
        client_message_id: ClientMessageId,
    },
    FailPresenceTriggeredSend {
        client_message_id: ClientMessageId,
        failed_at_ms: i64,
    },
    RemovePresenceTriggeredSend {
        client_message_id: ClientMessageId,
    },
    ForwardMessage {
        source_conversation_id: ConversationId,
        message_id: MessageId,
        destination_conversation_id: ConversationId,
        local_message_id: MessageId,
        client_message_id: ClientMessageId,
        sender_id: ActorId,
        thread_root_message_id: Option<MessageId>,
        created_at_ms: i64,
        scheduled_at_ms: Option<i64>,
        silent: bool,
        privacy: ForwardPrivacy,
    },
    AcknowledgeMessage {
        conversation_id: ConversationId,
        local_message_id: MessageId,
        server_message_id: MessageId,
        accepted_at_ms: i64,
    },
    SetDeliveryState {
        conversation_id: ConversationId,
        message_id: MessageId,
        state: DeliveryState,
    },
    EditMessage {
        conversation_id: ConversationId,
        message_id: MessageId,
        content: MessageContent,
        edited_at_ms: i64,
    },
    DeleteMessages {
        conversation_id: ConversationId,
        message_ids: Vec<MessageId>,
    },
    MarkRead {
        conversation_id: ConversationId,
        actor_id: ActorId,
        message_id: MessageId,
    },
    MarkTopicRead {
        conversation_id: ConversationId,
        topic_id: String,
        actor_id: ActorId,
        message_id: MessageId,
    },
    MarkConversationChildRead {
        destination: ConversationDestination,
        actor_id: ActorId,
        message_id: MessageId,
    },
    SetTopicDraft {
        draft: TopicDraft,
    },
    SetConversationChildDraft {
        destination: ConversationDestination,
        actor_id: ActorId,
        text: String,
        reply_to_message_id: Option<MessageId>,
        updated_at_ms: i64,
    },
    ReplaceConversationChildWindow {
        destination: ConversationDestination,
        actor_id: ActorId,
        message_ids: Vec<MessageId>,
        skipped_before: Option<u32>,
        skipped_after: Option<u32>,
        full_count: Option<u32>,
    },
    SetConversationChildPinned {
        destination: ConversationDestination,
        actor_id: ActorId,
        pinned: bool,
    },
    SetConversationChildActive {
        destination: ConversationDestination,
        actor_id: ActorId,
        active: bool,
    },
    SetConversationChildMarkedUnread {
        destination: ConversationDestination,
        actor_id: ActorId,
        marked_unread: bool,
    },
    SetConversationChildNoPaidMessages {
        destination: ConversationDestination,
        actor_id: ActorId,
        no_paid_messages: bool,
    },
    DestroyConversationChild {
        destination: ConversationDestination,
        actor_id: ActorId,
    },
    SetReaction {
        conversation_id: ConversationId,
        message_id: MessageId,
        reaction: ReactionSummary,
    },
    PinMessage {
        conversation_id: ConversationId,
        message_id: MessageId,
        pinned: bool,
    },
    VotePoll {
        conversation_id: ConversationId,
        message_id: MessageId,
        actor_id: ActorId,
        option_ids: Vec<String>,
    },
    CreateInvoice {
        invoice: Invoice,
    },
    UpsertOrder {
        order: PaymentOrder,
    },
    CheckoutInvoice {
        invoice_id: String,
        order_id: String,
        buyer_id: ActorId,
        customer: Option<CustomerInfo>,
        created_at_ms: i64,
    },
    RefundOrder {
        order_id: String,
        seller_id: ActorId,
        request_id: String,
        refunded_at_ms: i64,
    },
    CreditWalletSettlement {
        request_id: String,
        owner_id: ActorId,
        amount: Money,
        reference: Option<String>,
        settled_at_ms: i64,
    },
    PublishStory {
        actor_id: ActorId,
        story: Story,
    },
    DeleteStory {
        actor_id: ActorId,
        story_id: StoryId,
    },
    ViewStory {
        actor_id: ActorId,
        story_id: StoryId,
        viewed_at_ms: i64,
    },
    ReactStory {
        actor_id: ActorId,
        story_id: StoryId,
        reaction: Option<String>,
    },
    UpdateCommunity {
        actor_id: ActorId,
        community: CommunityState,
    },
    SubscribeChannel {
        actor_id: ActorId,
        conversation_id: ConversationId,
        subscribed_at_ms: i64,
    },
    UnsubscribeChannel {
        actor_id: ActorId,
        conversation_id: ConversationId,
        unsubscribed_at_ms: i64,
    },
    SetCommunitySlowMode {
        actor_id: ActorId,
        conversation_id: ConversationId,
        seconds: Option<u32>,
        changed_at_ms: i64,
    },
    ModerateCommunityMember {
        actor_id: ActorId,
        conversation_id: ConversationId,
        member: CommunityMember,
        reason: Option<String>,
        decided_at_ms: i64,
    },
    SetCommunityMember {
        actor_id: ActorId,
        conversation_id: ConversationId,
        member: CommunityMember,
    },
    CreateInviteLink {
        actor_id: ActorId,
        invite: InviteLink,
    },
    RevokeInviteLink {
        actor_id: ActorId,
        conversation_id: ConversationId,
        invite_id: String,
        revoked_at_ms: i64,
    },
    RequestCommunityJoin {
        actor_id: ActorId,
        request: JoinRequest,
    },
    RespondCommunityJoin {
        actor_id: ActorId,
        conversation_id: ConversationId,
        requester_id: ActorId,
        approved: bool,
        decided_at_ms: i64,
    },
    UpsertForumTopic {
        actor_id: ActorId,
        topic: ForumTopicState,
    },
    DeleteForumTopic {
        actor_id: ActorId,
        conversation_id: ConversationId,
        topic_id: String,
    },
    RegisterBot {
        actor_id: ActorId,
        profile: BotProfile,
    },
    BeginBotInvocation {
        actor_id: ActorId,
        invocation: BotInvocation,
        created_at_ms: i64,
    },
    FinishBotExecution {
        actor_id: ActorId,
        execution_id: String,
        success: bool,
        finished_at_ms: i64,
        error: Option<String>,
    },
    InstallMiniApp {
        manifest: MiniAppManifest,
    },
    GrantMiniApp {
        grant: MiniAppGrant,
    },
    OpenMiniApp {
        session: MiniAppSession,
    },
    MiniAppCall {
        session_id: String,
        request_id: String,
        request: MiniAppRequest,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum Event {
    ActorUpserted {
        actor: Actor,
    },
    PrivacySettingsUpdated {
        actor_id: ActorId,
        settings: PrivacySettings,
    },
    PresenceUpdated {
        actor_id: ActorId,
        presence: Presence,
    },
    ConversationUpserted {
        conversation: Conversation,
    },
    ConversationInfoUpdated {
        conversation_id: ConversationId,
        title: String,
        description: Option<String>,
    },
    ConversationParticipantUpserted {
        conversation_id: ConversationId,
        participant: Participant,
    },
    ConversationParticipantRemoved {
        conversation_id: ConversationId,
        actor_id: ActorId,
    },
    ConversationArchived {
        conversation_id: ConversationId,
        archived: bool,
    },
    ConversationPinned {
        conversation_id: ConversationId,
        pinned: bool,
    },
    ConversationMarkedUnread {
        conversation_id: ConversationId,
        actor_id: ActorId,
        marked_unread: bool,
    },
    DraftChanged {
        draft: ConversationDraft,
    },
    ConversationNotificationsUpdated {
        conversation_id: ConversationId,
        settings: NotificationSettings,
    },
    FolderUpserted {
        folder: ConversationFolder,
    },
    FolderDeleted {
        folder_id: String,
    },
    PresenceTriggeredSendQueued {
        pending: PendingPresenceSend,
    },
    PresenceTriggeredSendRemoved {
        client_message_id: ClientMessageId,
    },
    MessageQueued {
        message: Message,
    },
    MessageAcknowledged {
        conversation_id: ConversationId,
        local_message_id: MessageId,
        server_message_id: MessageId,
        accepted_at_ms: i64,
    },
    DeliveryStateUpdated {
        conversation_id: ConversationId,
        message_id: MessageId,
        state: DeliveryState,
    },
    MessageEdited {
        conversation_id: ConversationId,
        message_id: MessageId,
        content: MessageContent,
        edited_at_ms: i64,
    },
    MessagesDeleted {
        conversation_id: ConversationId,
        message_ids: Vec<MessageId>,
    },
    ConversationRead {
        conversation_id: ConversationId,
        actor_id: ActorId,
        message_id: MessageId,
    },
    TopicReadChanged {
        conversation_id: ConversationId,
        topic_id: String,
        actor_id: ActorId,
        message_id: MessageId,
    },
    ConversationChildReadChanged {
        destination: ConversationDestination,
        actor_id: ActorId,
        message_id: MessageId,
    },
    TopicDraftChanged {
        draft: TopicDraft,
    },
    ConversationChildDraftChanged {
        destination: ConversationDestination,
        actor_id: ActorId,
        text: String,
        reply_to_message_id: Option<String>,
        updated_at_ms: i64,
    },
    ConversationChildWindowReplaced {
        destination: ConversationDestination,
        actor_id: ActorId,
        message_ids: Vec<String>,
        skipped_before: Option<u32>,
        skipped_after: Option<u32>,
        full_count: Option<u32>,
    },
    ConversationChildPinnedChanged {
        destination: ConversationDestination,
        actor_id: ActorId,
        pinned: bool,
    },
    ConversationChildActiveChanged {
        destination: ConversationDestination,
        actor_id: ActorId,
        active: bool,
    },
    ConversationChildMarkedUnreadChanged {
        destination: ConversationDestination,
        actor_id: ActorId,
        marked_unread: bool,
    },
    ConversationChildNoPaidMessagesChanged {
        destination: ConversationDestination,
        actor_id: ActorId,
        no_paid_messages: bool,
    },
    ConversationChildDestroyed {
        destination: ConversationDestination,
        actor_id: ActorId,
    },
    ReactionUpdated {
        conversation_id: ConversationId,
        message_id: MessageId,
        reaction: ReactionSummary,
    },
    MessagePinned {
        conversation_id: ConversationId,
        message_id: MessageId,
        pinned: bool,
    },
    PollVoteChanged {
        conversation_id: ConversationId,
        message_id: MessageId,
        actor_id: ActorId,
        option_ids: Vec<String>,
    },
    InvoiceCreated {
        invoice: Invoice,
    },
    OrderUpserted {
        order: PaymentOrder,
    },
    WalletChanged {
        wallet: WalletLedger,
        entry: LedgerEntry,
    },
    StoryChanged {
        story: Story,
    },
    StoryDeleted {
        story_id: StoryId,
    },
    CommunityChanged {
        community: CommunityState,
    },
    BotRegistryChanged {
        registry: BotRegistry,
        profile: Option<BotProfile>,
        execution: Option<BotExecution>,
    },
    MiniAppInstalled {
        manifest: MiniAppManifest,
    },
    MiniAppGrantUpdated {
        grant: MiniAppGrant,
    },
    MiniAppOpened {
        session: MiniAppSession,
    },
    MiniAppResponded {
        session_id: String,
        request_id: String,
        response: MiniAppResponse,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MessagingState {
    pub actors: BTreeMap<ActorId, Actor>,
    pub privacy: BTreeMap<ActorId, PrivacySettings>,
    pub conversations: BTreeMap<ConversationId, Conversation>,
    pub folders: BTreeMap<String, ConversationFolder>,
    pub messages: BTreeMap<ConversationId, BTreeMap<MessageId, Message>>,
    pub read_cursors: BTreeMap<ConversationId, BTreeMap<ActorId, MessageId>>,
    pub topic_read_cursors:
        BTreeMap<ConversationId, BTreeMap<ActorId, BTreeMap<String, MessageId>>>,
    pub poll_votes:
        BTreeMap<ConversationId, BTreeMap<MessageId, BTreeMap<ActorId, BTreeSet<String>>>>,
    pub marked_unread_by_actor: BTreeMap<ConversationId, BTreeSet<ActorId>>,
    pub drafts: BTreeMap<ConversationId, BTreeMap<ActorId, ConversationDraft>>,
    pub topic_drafts: BTreeMap<ConversationId, BTreeMap<ActorId, BTreeMap<String, TopicDraft>>>,
    /// Canonical source-neutral child lifecycle state. Topic-only legacy maps above
    /// remain protocol-compatibility projections until their load-time migration
    /// is completed; new saved-sublist/community child state belongs here.
    pub conversation_child_states: Vec<ConversationChildRuntimeState>,
    pub pending_presence_sends: BTreeMap<ClientMessageId, PendingPresenceSend>,
    pub invoices: BTreeMap<String, Invoice>,
    pub orders: BTreeMap<String, PaymentOrder>,
    pub wallet: WalletLedger,
    pub stories: BTreeMap<StoryId, Story>,
    pub communities: BTreeMap<ConversationId, CommunityState>,
    pub bots: BotRegistry,
    pub mini_apps: BTreeMap<String, MiniAppManifest>,
    pub mini_app_grants: BTreeMap<(String, ActorId), MiniAppGrant>,
    pub mini_app_sessions: BTreeMap<String, MiniAppSession>,
}

impl MessagingState {
    pub fn child_state_mut(
        &mut self,
        destination: ConversationDestination,
        actor_id: ActorId,
    ) -> Option<&mut ConversationChildRuntimeState> {
        if let Some(index) = self.conversation_child_states.iter().position(|state| {
            state.destination == destination && state.actor_id == actor_id
        }) {
            return self.conversation_child_states.get_mut(index);
        }
        let state = ConversationChildRuntimeState::new(destination, actor_id)?;
        self.conversation_child_states.push(state);
        self.conversation_child_states.last_mut()
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum EngineError {
    #[error("actor identifier is invalid")]
    InvalidActor,
    #[error("actor {0:?} does not exist")]
    ActorNotFound(ActorId),
    #[error("conversation identifier is invalid")]
    InvalidConversation,
    #[error("conversation {0:?} does not exist")]
    ConversationNotFound(ConversationId),
    #[error("conversation participant data is invalid")]
    InvalidConversationParticipant,
    #[error("message {message_id:?} does not exist in conversation {conversation_id:?}")]
    MessageNotFound {
        conversation_id: ConversationId,
        message_id: MessageId,
    },
    #[error("presence-triggered send is invalid for this target or conversation")]
    InvalidPresenceTriggeredSend,
    #[error("presence-trigger target is already online")]
    PresenceTriggerAlreadySatisfied,
    #[error("client message id {0:?} conflicts with an existing presence-triggered send")]
    DuplicatePresenceTriggeredSend(ClientMessageId),
    #[error("message {message_id:?} already exists in conversation {conversation_id:?}")]
    DuplicateMessage {
        conversation_id: ConversationId,
        message_id: MessageId,
    },
    #[error("client message identifier is invalid")]
    InvalidClientMessageId,
    #[error("message id list must not be empty")]
    EmptyMessageList,
    #[error("poll vote is invalid")]
    InvalidPollVote,
    #[error("message is protected from forwarding")]
    ProtectedContent,
    #[error("actor {actor_id:?} is not a participant of conversation {conversation_id:?}")]
    SenderNotParticipant {
        conversation_id: ConversationId,
        actor_id: ActorId,
    },
    #[error("conversation {0:?} does not allow sending messages")]
    MessageSendPermissionDenied(ConversationId),
    #[error("conversation {0:?} does not allow sending media")]
    MediaSendPermissionDenied(ConversationId),
    #[error("conversation {0:?} does not allow sending polls")]
    PollSendPermissionDenied(ConversationId),
    #[error("conversation {0:?} does not allow sending stickers, animations, or dice")]
    OtherSendPermissionDenied(ConversationId),
    #[error("secret conversations must contain exactly two participants")]
    InvalidSecretConversation,
    #[error("secret conversations only accept encrypted secret content or service messages")]
    SecretPlaintextRejected,
    #[error("secret content may only be sent inside a secret conversation")]
    SecretContentOutsideSecretConversation,
    #[error("secret message envelope identity does not match the conversation participants")]
    SecretEnvelopeMismatch,
    #[error("story is invalid")]
    InvalidStory,
    #[error("story {0:?} does not exist")]
    StoryNotFound(StoryId),
    #[error("only the story owner may modify story {0:?}")]
    StoryPermissionDenied(StoryId),
    #[error(transparent)]
    Story(#[from] StoryError),
    #[error("community {0:?} does not exist")]
    CommunityNotFound(ConversationId),
    #[error("community administration permission denied")]
    CommunityPermissionDenied,
    #[error("community {0:?} member is not allowed to send messages")]
    CommunitySendRestricted(ConversationId),
    #[error("community {0:?} member is not allowed to send media")]
    CommunityMediaRestricted(ConversationId),
    #[error("community {0:?} member is not allowed to send polls")]
    CommunityPollRestricted(ConversationId),
    #[error("community {0:?} member is not allowed to send stickers, animations, or dice")]
    CommunityOtherRestricted(ConversationId),
    #[error("actor {actor_id:?} does not have access to community {conversation_id:?}")]
    CommunityAccessDenied {
        conversation_id: ConversationId,
        actor_id: ActorId,
    },
    #[error("community {conversation_id:?} slow mode is active until {retry_at_ms}")]
    SlowModeActive {
        conversation_id: ConversationId,
        retry_at_ms: i64,
    },
    #[error("forum topic {topic_id:?} does not exist in community {conversation_id:?}")]
    ForumTopicNotFound {
        conversation_id: ConversationId,
        topic_id: String,
    },
    #[error("forum topic {topic_id:?} is closed in community {conversation_id:?}")]
    ForumTopicClosed {
        conversation_id: ConversationId,
        topic_id: String,
    },
    #[error("message does not belong to forum topic {topic_id:?}")]
    TopicMessageMismatch { topic_id: String },
    #[error("conversation child destination is invalid")]
    InvalidConversationChildDestination,
    #[error("message does not belong to the selected conversation child")]
    ConversationChildMessageMismatch,
    #[error(transparent)]
    Community(#[from] CommunityError),
    #[error("bot operation permission denied")]
    BotPermissionDenied,
    #[error("bot operation failed: {0}")]
    Bot(String),
    #[error("invoice is invalid")]
    InvalidInvoice,
    #[error("invoice {0} does not exist")]
    InvoiceNotFound(String),
    #[error("payment order {0} does not exist")]
    OrderNotFound(String),
    #[error("payment invoice {0} has expired")]
    InvoiceExpired(String),
    #[error("payment order {0} conflicts with an existing order")]
    OrderConflict(String),
    #[error("only the invoice seller may refund order {0}")]
    RefundForbidden(String),
    #[error("payment order {0} is not refundable in its current state")]
    OrderNotRefundable(String),
    #[error(transparent)]
    Wallet(#[from] WalletError),
    #[error("Mini App {0} is not installed")]
    MiniAppNotFound(String),
    #[error("Mini App session {0} does not exist")]
    MiniAppSessionNotFound(String),
    #[error("Mini App permission {0:?} was not granted")]
    MiniAppPermissionDenied(MiniAppPermission),
    #[error("Mini App request cannot be completed by the pure domain engine")]
    MiniAppHostActionRequired,
}

#[derive(Debug, Clone, Default)]
pub struct MessagingEngine {
    pub(crate) state: MessagingState,
}

pub(crate) fn topic_id_from_root(root: &MessageId) -> Option<&str> {
    root.0.strip_prefix("topic:").filter(|id| !id.is_empty())
}

fn wallet_account_id(actor_id: &ActorId) -> WalletAccountId {
    WalletAccountId(format!("wallet:{}", actor_id.0))
}

fn ensure_wallet_account(
    wallet: &mut WalletLedger,
    account_id: &WalletAccountId,
    owner_id: &ActorId,
    now_ms: i64,
) -> Result<(), WalletError> {
    if wallet.accounts.contains_key(account_id) {
        return Ok(());
    }
    wallet.create_account(account_id.clone(), owner_id.clone(), now_ms)
}

impl MessagingEngine {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn from_state(mut state: MessagingState) -> Self {
        state.migrate_legacy_topic_child_states();
        Self { state }
    }
    pub fn state(&self) -> &MessagingState {
        &self.state
    }
    pub fn into_state(self) -> MessagingState {
        self.state
    }

    pub fn execute(&mut self, command: Command) -> Result<Vec<Event>, EngineError> {
        let events = self.decide(command)?;
        for event in events.iter().cloned() {
            self.apply(event);
        }
        Ok(events)
    }

    pub fn decide(&self, command: Command) -> Result<Vec<Event>, EngineError> {
        match command {
            Command::UpsertActor { actor } => {
                if !actor.id.is_valid() || actor.display_name.trim().is_empty() {
                    return Err(EngineError::InvalidActor);
                }
                Ok(vec![Event::ActorUpserted { actor }])
            }
            Command::UpsertPrivacySettings { actor_id, settings } => {
                self.require_actor(&actor_id)?;
                Ok(vec![Event::PrivacySettingsUpdated { actor_id, settings }])
            }
            Command::SetPresence { actor_id, presence } => {
                self.require_actor(&actor_id)?;
                Ok(vec![Event::PresenceUpdated { actor_id, presence }])
            }
            Command::UpsertConversation { conversation } => {
                if !conversation.id.is_valid() || conversation.title.trim().is_empty() {
                    return Err(EngineError::InvalidConversation);
                }
                if let Some(existing) = self.state.conversations.get(&conversation.id) {
                    if matches!(
                        existing.kind,
                        ConversationKind::Group | ConversationKind::Channel
                    ) && (conversation.owner_id != existing.owner_id
                        || conversation.participants != existing.participants
                        || conversation.topics != existing.topics)
                    {
                        return Err(EngineError::InvalidConversationParticipant);
                    }
                }
                for participant in &conversation.participants {
                    self.require_actor(&participant.actor_id)?;
                }
                if matches!(
                    conversation.kind,
                    crate::conversation::ConversationKind::Secret
                ) && conversation.participants.len() != 2
                {
                    return Err(EngineError::InvalidSecretConversation);
                }
                Ok(vec![Event::ConversationUpserted { conversation }])
            }
            Command::UpdateConversationInfo {
                conversation_id,
                title,
                description,
            } => {
                self.require_conversation(&conversation_id)?;
                if title.trim().is_empty() || title.trim().len() > 200 {
                    return Err(EngineError::InvalidConversationParticipant);
                }
                Ok(vec![Event::ConversationInfoUpdated {
                    conversation_id,
                    title: title.trim().to_string(),
                    description: description
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty()),
                }])
            }
            Command::SetConversationParticipant {
                conversation_id,
                participant,
            } => {
                let conversation = self.require_conversation(&conversation_id)?;
                self.require_actor(&participant.actor_id)?;
                if conversation.owner_id.as_ref() == Some(&participant.actor_id)
                    && !matches!(participant.role, ParticipantRole::Owner)
                {
                    return Err(EngineError::InvalidConversationParticipant);
                }
                if matches!(
                    conversation.kind,
                    ConversationKind::Group | ConversationKind::Channel
                ) {
                    let mut community = self
                        .state
                        .communities
                        .get(&conversation_id)
                        .cloned()
                        .ok_or_else(|| EngineError::CommunityNotFound(conversation_id.clone()))?;
                    let status = match participant.role {
                        ParticipantRole::Owner => MemberStatus::Owner,
                        ParticipantRole::Admin => MemberStatus::Administrator,
                        ParticipantRole::Member => MemberStatus::Member,
                        ParticipantRole::Restricted => MemberStatus::Restricted,
                    };
                    let mut member = community
                        .members
                        .get(&participant.actor_id)
                        .cloned()
                        .unwrap_or_else(|| CommunityMember {
                            actor_id: participant.actor_id.clone(),
                            status: MemberStatus::Member,
                            admin_title: None,
                            admin_rights: AdminRights::default(),
                            restrictions: Default::default(),
                            joined_at_ms: participant.joined_at_ms,
                            invited_by: None,
                        });
                    member.status = status;
                    member.joined_at_ms = participant.joined_at_ms;
                    community
                        .members
                        .insert(participant.actor_id.clone(), member);
                    Ok(vec![
                        Event::CommunityChanged { community },
                        Event::ConversationParticipantUpserted {
                            conversation_id,
                            participant,
                        },
                    ])
                } else {
                    Ok(vec![Event::ConversationParticipantUpserted {
                        conversation_id,
                        participant,
                    }])
                }
            }
            Command::RemoveConversationParticipant {
                conversation_id,
                actor_id,
            } => {
                let conversation = self.require_conversation(&conversation_id)?;
                if conversation.owner_id.as_ref() == Some(&actor_id) {
                    return Err(EngineError::InvalidConversationParticipant);
                }
                if matches!(
                    conversation.kind,
                    ConversationKind::Group | ConversationKind::Channel
                ) {
                    let mut community = self
                        .state
                        .communities
                        .get(&conversation_id)
                        .cloned()
                        .ok_or_else(|| EngineError::CommunityNotFound(conversation_id.clone()))?;
                    if let Some(member) = community.members.get_mut(&actor_id) {
                        member.status = MemberStatus::Left;
                    }
                    community.subscribers.remove(&actor_id);
                    Ok(vec![
                        Event::CommunityChanged { community },
                        Event::ConversationParticipantRemoved {
                            conversation_id,
                            actor_id,
                        },
                    ])
                } else {
                    Ok(vec![Event::ConversationParticipantRemoved {
                        conversation_id,
                        actor_id,
                    }])
                }
            }
            Command::ArchiveConversation {
                conversation_id,
                archived,
            } => {
                self.require_conversation(&conversation_id)?;
                Ok(vec![Event::ConversationArchived {
                    conversation_id,
                    archived,
                }])
            }
            Command::PinConversation {
                conversation_id,
                pinned,
            } => {
                self.require_conversation(&conversation_id)?;
                Ok(vec![Event::ConversationPinned {
                    conversation_id,
                    pinned,
                }])
            }
            Command::SetMarkedUnread {
                conversation_id,
                actor_id,
                marked_unread,
            } => {
                self.require_conversation(&conversation_id)?;
                self.require_actor(&actor_id)?;
                Ok(vec![Event::ConversationMarkedUnread {
                    conversation_id,
                    actor_id,
                    marked_unread,
                }])
            }
            Command::SetDraft { draft } => {
                self.require_conversation(&draft.conversation_id)?;
                self.require_actor(&draft.actor_id)?;
                Ok(vec![Event::DraftChanged { draft }])
            }
            Command::SetConversationNotifications {
                conversation_id,
                settings,
            } => {
                self.require_conversation(&conversation_id)?;
                Ok(vec![Event::ConversationNotificationsUpdated {
                    conversation_id,
                    settings,
                }])
            }
            Command::UpsertFolder { folder } => Ok(vec![Event::FolderUpserted { folder }]),
            Command::DeleteFolder { folder_id } => Ok(vec![Event::FolderDeleted { folder_id }]),
            Command::QueuePresenceTriggeredSend { pending } => {
                let sender = self.require_actor(&pending.sender_id)?;
                let target_actor_id = match &pending.trigger {
                    PresenceSendTrigger::WhenParticipantOnline { actor_id } => actor_id,
                };
                let target = self.require_actor(target_actor_id)?;
                if target.kind != ActorKind::Human || &pending.sender_id == target_actor_id {
                    return Err(EngineError::InvalidPresenceTriggeredSend);
                }
                if target.id.0.starts_with("service:")
                    || target.capabilities.iter().any(|cap| {
                        matches!(
                            cap.as_str(),
                            "service_notifications"
                                | "notifications_service"
                                | "paid_messages_required"
                                | "stars_per_message_required"
                        )
                    })
                {
                    return Err(EngineError::InvalidPresenceTriggeredSend);
                }
                if let Some(target_privacy) = self.state.privacy.get(target_actor_id) {
                    let target_considers_sender_contact = target
                        .capabilities
                        .iter()
                        .any(|cap| cap == &format!("contact:{}", pending.sender_id.0));
                    let sender_considers_target_contact = sender
                        .capabilities
                        .iter()
                        .any(|cap| cap == &format!("contact:{}", target_actor_id.0));
                    if !target_privacy.can_receive_message_from(
                        &pending.sender_id,
                        target_considers_sender_contact,
                        target_considers_sender_contact && sender_considers_target_contact,
                    ) {
                        return Err(EngineError::InvalidPresenceTriggeredSend);
                    }
                    if let Some(last_seen_rule) = target_privacy.rules.get(&PrivacyKey::LastSeen) {
                        if !last_seen_rule.allows(&pending.sender_id, target_considers_sender_contact) {
                            return Err(EngineError::InvalidPresenceTriggeredSend);
                        }
                    }
                }
                if target.presence.status == PresenceStatus::Online {
                    return Err(EngineError::PresenceTriggerAlreadySatisfied);
                }
                let conversation = self.require_conversation(&pending.conversation_id)?;
                if !matches!(conversation.kind, ConversationKind::Direct) {
                    return Err(EngineError::InvalidPresenceTriggeredSend);
                }
                let target_is_participant = conversation
                    .participants
                    .iter()
                    .any(|participant| &participant.actor_id == target_actor_id)
                    || conversation.owner_id.as_ref() == Some(target_actor_id);
                if !target_is_participant {
                    return Err(EngineError::InvalidPresenceTriggeredSend);
                }
                if let Some(existing) = self
                    .state
                    .pending_presence_sends
                    .get(&pending.client_message_id)
                {
                    if existing == &pending {
                        return Ok(Vec::new());
                    }
                    return Err(EngineError::DuplicatePresenceTriggeredSend(
                        pending.client_message_id.clone(),
                    ));
                }
                // Reuse the canonical send decision for all current conversation,
                // media/poll, membership, channel/community, topic and slow-mode gates.
                let validation = self.decide(Command::QueueMessage {
                    conversation_id: pending.conversation_id.clone(),
                    local_message_id: pending.local_message_id.clone(),
                    client_message_id: pending.client_message_id.clone(),
                    sender_id: pending.sender_id.clone(),
                    content: pending.content.clone(),
                    reply_to_message_id: pending.reply_to_message_id.clone(),
                    thread_root_message_id: pending.thread_root_message_id.clone(),
                    created_at_ms: pending.created_at_ms,
                    scheduled_at_ms: None,
                    silent: pending.silent,
                    protected_content: pending.protected_content,
                })?;
                if !matches!(validation.as_slice(), [Event::MessageQueued { .. }]) {
                    return Err(EngineError::InvalidPresenceTriggeredSend);
                }
                Ok(vec![Event::PresenceTriggeredSendQueued { pending }])
            }
            Command::CancelPresenceTriggeredSend {
                actor_id,
                client_message_id,
            } => self.decide_cancel_presence_triggered_send(actor_id, client_message_id),
            Command::FailPresenceTriggeredSend {
                client_message_id,
                failed_at_ms,
            } => self.decide_fail_presence_triggered_send(client_message_id, failed_at_ms),
            Command::RemovePresenceTriggeredSend { client_message_id } => {
                self.decide_remove_presence_triggered_send(client_message_id)
            }
            Command::QueueMessage {
                conversation_id,
                local_message_id,
                client_message_id,
                sender_id,
                content,
                reply_to_message_id,
                thread_root_message_id,
                created_at_ms,
                scheduled_at_ms,
                silent,
                protected_content,
            } => {
                let conversation = self.require_conversation(&conversation_id)?;
                self.require_actor(&sender_id)?;
                let sender_is_participant = conversation
                    .participants
                    .iter()
                    .any(|participant| participant.actor_id == sender_id)
                    || conversation.owner_id.as_ref() == Some(&sender_id);
                if !sender_is_participant {
                    return Err(EngineError::SenderNotParticipant {
                        conversation_id: conversation_id.clone(),
                        actor_id: sender_id.clone(),
                    });
                }
                if !conversation.permissions.can_send_messages {
                    return Err(EngineError::MessageSendPermissionDenied(
                        conversation_id.clone(),
                    ));
                }
                if message_content_uses_media(&content) && !conversation.permissions.can_send_media
                {
                    return Err(EngineError::MediaSendPermissionDenied(
                        conversation_id.clone(),
                    ));
                }
                if matches!(&content, MessageContent::Poll { .. })
                    && !conversation.permissions.can_send_polls
                {
                    return Err(EngineError::PollSendPermissionDenied(
                        conversation_id.clone(),
                    ));
                }
                if message_content_uses_other(&content) && !conversation.permissions.can_send_other
                {
                    return Err(EngineError::OtherSendPermissionDenied(
                        conversation_id.clone(),
                    ));
                }
                if let Some(member) = self
                    .state
                    .communities
                    .get(&conversation_id)
                    .and_then(|community| community.members.get(&sender_id))
                {
                    if matches!(member.status, MemberStatus::Left | MemberStatus::Banned)
                        || (matches!(member.status, MemberStatus::Restricted)
                            && member.restrictions.send_messages)
                    {
                        return Err(EngineError::CommunitySendRestricted(
                            conversation_id.clone(),
                        ));
                    }
                    if message_content_uses_media(&content)
                        && matches!(member.status, MemberStatus::Restricted)
                        && member.restrictions.send_media
                    {
                        return Err(EngineError::CommunityMediaRestricted(
                            conversation_id.clone(),
                        ));
                    }
                    if matches!(&content, MessageContent::Poll { .. })
                        && matches!(member.status, MemberStatus::Restricted)
                        && member.restrictions.send_polls
                    {
                        return Err(EngineError::CommunityPollRestricted(
                            conversation_id.clone(),
                        ));
                    }
                    if message_content_uses_other(&content)
                        && matches!(member.status, MemberStatus::Restricted)
                        && member.restrictions.send_other
                    {
                        return Err(EngineError::CommunityOtherRestricted(
                            conversation_id.clone(),
                        ));
                    }
                }
                if matches!(conversation.kind, ConversationKind::Channel) {
                    let can_post =
                        conversation.owner_id.as_ref() == Some(&sender_id)
                            || conversation.participants.iter().any(|participant| {
                                participant.actor_id == sender_id
                                    && matches!(
                                        participant.role,
                                        ParticipantRole::Owner | ParticipantRole::Admin
                                    )
                            })
                            || self.state.communities.get(&conversation_id).is_some_and(
                                |community| {
                                    community.members.get(&sender_id).is_some_and(|member| {
                                        matches!(member.status, MemberStatus::Administrator)
                                            && member.admin_rights.post_messages
                                    })
                                },
                            );
                    if !can_post {
                        return Err(EngineError::CommunitySendRestricted(
                            conversation_id.clone(),
                        ));
                    }
                }
                if let Some(community) = self.state.communities.get(&conversation_id) {
                    if let Some(thread_root) = &thread_root_message_id {
                        if let Some(topic_id) = topic_id_from_root(thread_root) {
                            let topic = community.topics.get(topic_id).ok_or_else(|| {
                                EngineError::ForumTopicNotFound {
                                    conversation_id: conversation_id.clone(),
                                    topic_id: topic_id.to_string(),
                                }
                            })?;
                            if topic.closed || topic.hidden {
                                return Err(EngineError::ForumTopicClosed {
                                    conversation_id: conversation_id.clone(),
                                    topic_id: topic_id.to_string(),
                                });
                            }
                        }
                    }
                    if let Some(seconds) = community.slow_mode_seconds {
                        let bypass = community.can_moderate(&sender_id);
                        if !bypass {
                            let latest_sender_message = self
                                .state
                                .messages
                                .get(&conversation_id)
                                .into_iter()
                                .flat_map(|messages| messages.values())
                                .filter(|message| {
                                    message.sender_id == sender_id
                                        && !message.deleted
                                        && message
                                            .scheduled_at_ms
                                            .is_none_or(|scheduled| scheduled <= created_at_ms)
                                })
                                .max_by_key(|message| (message.created_at_ms, message.id.clone()));
                            if let Some(message) = latest_sender_message {
                                let retry_at = message
                                    .created_at_ms
                                    .saturating_add(i64::from(seconds).saturating_mul(1_000));
                                if created_at_ms < retry_at {
                                    return Err(EngineError::SlowModeActive {
                                        conversation_id: conversation_id.clone(),
                                        retry_at_ms: retry_at,
                                    });
                                }
                            }
                        }
                    }
                }
                let is_secret_conversation = matches!(
                    conversation.kind,
                    crate::conversation::ConversationKind::Secret
                );
                let is_secret_content = matches!(&content, MessageContent::Secret { .. });
                let is_service_content = matches!(&content, MessageContent::Service { .. });
                if is_secret_conversation && !is_secret_content && !is_service_content {
                    return Err(EngineError::SecretPlaintextRejected);
                }
                if !is_secret_conversation && is_secret_content {
                    return Err(EngineError::SecretContentOutsideSecretConversation);
                }
                if let MessageContent::Secret { envelope } = &content {
                    let participant_ids = conversation
                        .participants
                        .iter()
                        .map(|participant| &participant.actor_id)
                        .collect::<Vec<_>>();
                    if envelope.conversation_id != conversation_id
                        || envelope.sender_id != sender_id
                        || !participant_ids.contains(&&envelope.sender_id)
                        || !participant_ids.contains(&&envelope.recipient_id)
                        || envelope.sender_id == envelope.recipient_id
                    {
                        return Err(EngineError::SecretEnvelopeMismatch);
                    }
                }
                if client_message_id.0.trim().is_empty() || client_message_id.0.len() > 200 {
                    return Err(EngineError::InvalidClientMessageId);
                }
                if self
                    .state
                    .messages
                    .get(&conversation_id)
                    .is_some_and(|messages| messages.contains_key(&local_message_id))
                {
                    return Err(EngineError::DuplicateMessage {
                        conversation_id,
                        message_id: local_message_id,
                    });
                }
                let is_saved_messages = matches!(
                    conversation.kind,
                    crate::conversation::ConversationKind::SavedMessages
                );
                let message = Message {
                    id: local_message_id,
                    conversation_id,
                    sender_id,
                    content,
                    reply_to_message_id,
                    thread_root_message_id,
                    forward_origin: None,
                    reply_markup: None,
                    reactions: Vec::new(),
                    delivery_state: DeliveryState::Pending { client_message_id },
                    created_at_ms,
                    edited_at_ms: None,
                    scheduled_at_ms,
                    silent: if is_saved_messages { false } else { silent },
                    protected_content: protected_content || is_secret_content,
                    pinned: false,
                    deleted: false,
                };
                Ok(vec![Event::MessageQueued { message }])
            }
            Command::ForwardMessage {
                source_conversation_id,
                message_id,
                destination_conversation_id,
                local_message_id,
                client_message_id,
                sender_id,
                thread_root_message_id,
                created_at_ms,
                scheduled_at_ms,
                silent,
                privacy,
            } => {
                let conversation = self.require_conversation(&destination_conversation_id)?;
                self.require_actor(&sender_id)?;
                let original = self
                    .require_message(&source_conversation_id, &message_id)?
                    .clone();
                if original.deleted {
                    return Err(EngineError::MessageNotFound {
                        conversation_id: source_conversation_id,
                        message_id,
                    });
                }
                // Forwarding is still a send into the destination conversation. Reuse the
                // canonical destination policy instead of letting the forward path bypass
                // membership, media/poll, community, channel, slow-mode, or secret-chat gates.
                let sender_is_participant = conversation
                    .participants
                    .iter()
                    .any(|participant| participant.actor_id == sender_id)
                    || conversation.owner_id.as_ref() == Some(&sender_id);
                let member = self
                    .state
                    .communities
                    .get(&destination_conversation_id)
                    .and_then(|community| community.members.get(&sender_id));
                let community_forbidden = member.is_some_and(|m| {
                    matches!(m.status, MemberStatus::Left | MemberStatus::Banned)
                });
                let is_restricted =
                    member.is_some_and(|m| matches!(m.status, MemberStatus::Restricted));
                let channel_posting_allowed = !matches!(conversation.kind, ConversationKind::Channel)
                    || conversation.owner_id.as_ref() == Some(&sender_id)
                    || conversation.participants.iter().any(|participant| {
                        participant.actor_id == sender_id
                            && matches!(
                                participant.role,
                                ParticipantRole::Owner | ParticipantRole::Admin
                            )
                    })
                    || member.is_some_and(|m| {
                        matches!(m.status, MemberStatus::Administrator)
                            && m.admin_rights.post_messages
                    });
                let auth_input = RecipientAuthorizationInput {
                    conversation_kind: conversation.kind,
                    sender_is_participant,
                    can_send_messages: conversation.permissions.can_send_messages,
                    can_send_media: conversation.permissions.can_send_media,
                    can_send_polls: conversation.permissions.can_send_polls,
                    can_send_other: conversation.permissions.can_send_other,
                    can_send_inline: conversation.permissions.can_send_inline,
                    can_send_games: conversation.permissions.can_send_games,
                    community_forbidden,
                    community_restrict_messages: is_restricted
                        && member.is_some_and(|m| m.restrictions.send_messages),
                    community_restrict_media: is_restricted
                        && member.is_some_and(|m| m.restrictions.send_media),
                    community_restrict_polls: is_restricted
                        && member.is_some_and(|m| m.restrictions.send_polls),
                    community_restrict_other: is_restricted
                        && member.is_some_and(|m| m.restrictions.send_other),
                    community_restrict_inline: is_restricted
                        && member.is_some_and(|m| m.restrictions.send_inline),
                    community_restrict_games: is_restricted
                        && member.is_some_and(|m| m.restrictions.send_games),
                    channel_posting_allowed,
                };
                let requirements = RecipientSearchRequirements {
                    require_media: message_content_uses_media(&original.content),
                    require_polls: matches!(&original.content, MessageContent::Poll { .. }),
                    require_send_other: message_content_uses_other(&original.content),
                    require_inline: false,
                    require_game: false,
                    source_protected_content: original.protected_content,
                    content_kind: RecipientContentKind::Service,
                };
                if let Err(denial) = authorize_destination(auth_input, requirements) {
                    return Err(match denial {
                        DestinationAuthorizationDenial::SourceProtectedContent => {
                            EngineError::ProtectedContent
                        }
                        DestinationAuthorizationDenial::SenderNotParticipant => {
                            EngineError::SenderNotParticipant {
                                conversation_id: destination_conversation_id.clone(),
                                actor_id: sender_id.clone(),
                            }
                        }
                        DestinationAuthorizationDenial::MessageSendPermissionDenied => {
                            EngineError::MessageSendPermissionDenied(
                                destination_conversation_id.clone(),
                            )
                        }
                        DestinationAuthorizationDenial::MediaSendPermissionDenied => {
                            EngineError::MediaSendPermissionDenied(
                                destination_conversation_id.clone(),
                            )
                        }
                        DestinationAuthorizationDenial::PollSendPermissionDenied => {
                            EngineError::PollSendPermissionDenied(
                                destination_conversation_id.clone(),
                            )
                        }
                        DestinationAuthorizationDenial::OtherSendPermissionDenied
                        | DestinationAuthorizationDenial::InlineSendPermissionDenied
                        | DestinationAuthorizationDenial::GameSendPermissionDenied => {
                            EngineError::OtherSendPermissionDenied(
                                destination_conversation_id.clone(),
                            )
                        }
                        DestinationAuthorizationDenial::CommunitySendRestricted
                        | DestinationAuthorizationDenial::ChannelPostingRestricted => {
                            EngineError::CommunitySendRestricted(
                                destination_conversation_id.clone(),
                            )
                        }
                        DestinationAuthorizationDenial::CommunityMediaRestricted => {
                            EngineError::CommunityMediaRestricted(
                                destination_conversation_id.clone(),
                            )
                        }
                        DestinationAuthorizationDenial::CommunityPollRestricted => {
                            EngineError::CommunityPollRestricted(
                                destination_conversation_id.clone(),
                            )
                        }
                        DestinationAuthorizationDenial::CommunityOtherRestricted
                        | DestinationAuthorizationDenial::CommunityInlineRestricted
                        | DestinationAuthorizationDenial::CommunityGameRestricted => {
                            EngineError::CommunityOtherRestricted(
                                destination_conversation_id.clone(),
                            )
                        }
                        DestinationAuthorizationDenial::SecretPlaintextRejected => {
                            EngineError::SecretPlaintextRejected
                        }
                        DestinationAuthorizationDenial::SecretContentOutsideSecretConversation => {
                            EngineError::SecretContentOutsideSecretConversation
                        }
                        DestinationAuthorizationDenial::TopicNotFound => {
                            EngineError::ForumTopicNotFound {
                                conversation_id: destination_conversation_id.clone(),
                                topic_id: thread_root_message_id
                                    .as_ref()
                                    .map(|id| id.0.clone())
                                    .unwrap_or_default(),
                            }
                        }
                        DestinationAuthorizationDenial::TopicClosed => {
                            EngineError::ForumTopicClosed {
                                conversation_id: destination_conversation_id.clone(),
                                topic_id: thread_root_message_id
                                    .as_ref()
                                    .map(|id| id.0.clone())
                                    .unwrap_or_default(),
                            }
                        }
                    });
                }
                if let Some(community) = self.state.communities.get(&destination_conversation_id) {
                    if let Some(thread_root) = &thread_root_message_id {
                        if let Some(topic_id) = topic_id_from_root(thread_root) {
                            let topic = community.topics.get(topic_id).ok_or_else(|| {
                                EngineError::ForumTopicNotFound {
                                    conversation_id: destination_conversation_id.clone(),
                                    topic_id: topic_id.to_string(),
                                }
                            })?;
                            if topic.closed || topic.hidden {
                                return Err(EngineError::ForumTopicClosed {
                                    conversation_id: destination_conversation_id.clone(),
                                    topic_id: topic_id.to_string(),
                                });
                            }
                        }
                    }
                    if let Some(seconds) = community.slow_mode_seconds {
                        let bypass = community.can_moderate(&sender_id);
                        if !bypass {
                            let latest_sender_message = self
                                .state
                                .messages
                                .get(&destination_conversation_id)
                                .into_iter()
                                .flat_map(|messages| messages.values())
                                .filter(|message| {
                                    message.sender_id == sender_id
                                        && !message.deleted
                                        && message
                                            .scheduled_at_ms
                                            .is_none_or(|scheduled| scheduled <= created_at_ms)
                                })
                                .max_by_key(|message| (message.created_at_ms, message.id.clone()));
                            if let Some(message) = latest_sender_message {
                                let retry_at = message
                                    .created_at_ms
                                    .saturating_add(i64::from(seconds).saturating_mul(1_000));
                                if created_at_ms < retry_at {
                                    return Err(EngineError::SlowModeActive {
                                        conversation_id: destination_conversation_id.clone(),
                                        retry_at_ms: retry_at,
                                    });
                                }
                            }
                        }
                    }
                }
                let secret_content_kind = match &original.content {
                    MessageContent::Secret { .. } => RecipientContentKind::Secret,
                    MessageContent::Service { .. } => RecipientContentKind::Service,
                    _ => RecipientContentKind::Standard,
                };
                if let Err(denial) = authorize_destination(
                    auth_input,
                    RecipientSearchRequirements {
                        content_kind: secret_content_kind,
                        ..requirements
                    },
                ) {
                    return Err(match denial {
                        DestinationAuthorizationDenial::SecretPlaintextRejected => {
                            EngineError::SecretPlaintextRejected
                        }
                        DestinationAuthorizationDenial::SecretContentOutsideSecretConversation => {
                            EngineError::SecretContentOutsideSecretConversation
                        }
                        DestinationAuthorizationDenial::TopicNotFound => {
                            EngineError::ForumTopicNotFound {
                                conversation_id: destination_conversation_id.clone(),
                                topic_id: thread_root_message_id
                                    .as_ref()
                                    .map(|id| id.0.clone())
                                    .unwrap_or_default(),
                            }
                        }
                        DestinationAuthorizationDenial::TopicClosed => {
                            EngineError::ForumTopicClosed {
                                conversation_id: destination_conversation_id.clone(),
                                topic_id: thread_root_message_id
                                    .as_ref()
                                    .map(|id| id.0.clone())
                                    .unwrap_or_default(),
                            }
                        }
                        _ => EngineError::CommunitySendRestricted(
                            destination_conversation_id.clone(),
                        ),
                    });
                }
                if client_message_id.0.trim().is_empty() || client_message_id.0.len() > 200 {
                    return Err(EngineError::InvalidClientMessageId);
                }
                if self
                    .state
                    .messages
                    .get(&destination_conversation_id)
                    .is_some_and(|messages| messages.contains_key(&local_message_id))
                {
                    return Err(EngineError::DuplicateMessage {
                        conversation_id: destination_conversation_id,
                        message_id: local_message_id,
                    });
                }
                let privacy =
                    privacy.normalized_for_source(original.content.requires_preserving_forward_info());
                let forward_origin = if privacy.drop_sender_names {
                    None
                } else {
                    Some(
                        original
                            .forward_origin
                            .clone()
                            .unwrap_or_else(|| {
                                format!("{}:{}", original.conversation_id.0, original.id.0)
                            }),
                    )
                };
                let mut content = original.content;
                if privacy.drop_captions {
                    content.clear_caption();
                }
                if privacy.drop_sender_names {
                    let sender_can_keep_custom_emoji = self
                        .state
                        .actors
                        .get(&sender_id)
                        .is_some_and(|actor| {
                            actor.capabilities.iter().any(|cap| {
                                cap.eq_ignore_ascii_case("premium")
                                    || cap.eq_ignore_ascii_case("can_send_custom_emoji")
                            })
                        });
                    if !sender_can_keep_custom_emoji {
                        content.strip_custom_emoji();
                    }
                }
                let is_saved_messages = matches!(conversation.kind, ConversationKind::SavedMessages);
                let message = Message {
                    id: local_message_id,
                    conversation_id: destination_conversation_id,
                    sender_id,
                    content,
                    reply_to_message_id: None,
                    thread_root_message_id,
                    forward_origin,
                    reply_markup: original.reply_markup,
                    reactions: Vec::new(),
                    delivery_state: DeliveryState::Pending { client_message_id },
                    created_at_ms,
                    edited_at_ms: None,
                    scheduled_at_ms,
                    silent: if is_saved_messages { false } else { silent },
                    protected_content: original.protected_content,
                    pinned: false,
                    deleted: false,
                };
                Ok(vec![Event::MessageQueued { message }])
            }
            Command::AcknowledgeMessage {
                conversation_id,
                local_message_id,
                server_message_id,
                accepted_at_ms,
            } => {
                self.require_message(&conversation_id, &local_message_id)?;
                if local_message_id != server_message_id
                    && self
                        .state
                        .messages
                        .get(&conversation_id)
                        .is_some_and(|messages| messages.contains_key(&server_message_id))
                {
                    return Err(EngineError::DuplicateMessage {
                        conversation_id,
                        message_id: server_message_id,
                    });
                }
                Ok(vec![Event::MessageAcknowledged {
                    conversation_id,
                    local_message_id,
                    server_message_id,
                    accepted_at_ms,
                }])
            }
            Command::SetDeliveryState {
                conversation_id,
                message_id,
                state,
            } => {
                self.require_message(&conversation_id, &message_id)?;
                Ok(vec![Event::DeliveryStateUpdated {
                    conversation_id,
                    message_id,
                    state,
                }])
            }
            Command::EditMessage {
                conversation_id,
                message_id,
                content,
                edited_at_ms,
            } => {
                self.require_message(&conversation_id, &message_id)?;
                Ok(vec![Event::MessageEdited {
                    conversation_id,
                    message_id,
                    content,
                    edited_at_ms,
                }])
            }
            Command::DeleteMessages {
                conversation_id,
                message_ids,
            } => {
                let unique: BTreeSet<_> = message_ids.into_iter().collect();
                if unique.is_empty() {
                    return Err(EngineError::EmptyMessageList);
                }
                for id in &unique {
                    self.require_message(&conversation_id, id)?;
                }
                Ok(vec![Event::MessagesDeleted {
                    conversation_id,
                    message_ids: unique.into_iter().collect(),
                }])
            }
            Command::MarkRead {
                conversation_id,
                actor_id,
                message_id,
            } => {
                self.require_actor(&actor_id)?;
                let conversation = self.require_conversation(&conversation_id)?;
                let has_access = self
                    .state
                    .communities
                    .get(&conversation_id)
                    .map(|community| {
                        community_has_access(community, &actor_id)
                            || conversation.owner_id.as_ref() == Some(&actor_id)
                    })
                    .unwrap_or_else(|| {
                        conversation.owner_id.as_ref() == Some(&actor_id)
                            || conversation
                                .participants
                                .iter()
                                .any(|participant| participant.actor_id == actor_id)
                    });
                if !has_access {
                    return Err(EngineError::CommunityAccessDenied {
                        conversation_id,
                        actor_id,
                    });
                }
                self.require_message(&conversation_id, &message_id)?;
                Ok(vec![Event::ConversationRead {
                    conversation_id,
                    actor_id,
                    message_id,
                }])
            }
            Command::MarkTopicRead {
                conversation_id,
                topic_id,
                actor_id,
                message_id,
            } => {
                self.require_actor(&actor_id)?;
                let community = self
                    .state
                    .communities
                    .get(&conversation_id)
                    .ok_or_else(|| EngineError::CommunityNotFound(conversation_id.clone()))?;
                if !community_has_access(community, &actor_id)
                    && self
                        .state
                        .conversations
                        .get(&conversation_id)
                        .and_then(|conversation| conversation.owner_id.as_ref())
                        != Some(&actor_id)
                {
                    return Err(EngineError::CommunityAccessDenied {
                        conversation_id,
                        actor_id,
                    });
                }
                let topic = community.topics.get(&topic_id).ok_or_else(|| {
                    EngineError::ForumTopicNotFound {
                        conversation_id: conversation_id.clone(),
                        topic_id: topic_id.clone(),
                    }
                })?;
                let message = self.require_message(&conversation_id, &message_id)?;
                let belongs_to_topic = message
                    .thread_root_message_id
                    .as_ref()
                    .is_some_and(|root| topic_id_from_root(root) == Some(topic_id.as_str()))
                    || topic.last_message_id.as_deref() == Some(message_id.0.as_str());
                if !belongs_to_topic {
                    return Err(EngineError::TopicMessageMismatch { topic_id });
                }
                Ok(vec![Event::TopicReadChanged {
                    conversation_id,
                    topic_id,
                    actor_id,
                    message_id,
                }])
            }
            Command::MarkConversationChildRead {
                destination,
                actor_id,
                message_id,
            } => {
                if !destination.is_valid() {
                    return Err(EngineError::InvalidConversationChildDestination);
                }
                self.require_actor(&actor_id)?;
                let conversation_id = destination.conversation_id.clone();
                let conversation = self.require_conversation(&conversation_id)?;
                let community = self.state.communities.get(&conversation_id);
                let has_access = community
                    .map(|community| {
                        community_has_access(community, &actor_id)
                            || conversation.owner_id.as_ref() == Some(&actor_id)
                    })
                    .unwrap_or_else(|| {
                        conversation.owner_id.as_ref() == Some(&actor_id)
                            || conversation
                                .participants
                                .iter()
                                .any(|participant| participant.actor_id == actor_id)
                    });
                if !has_access {
                    return Err(EngineError::CommunityAccessDenied {
                        conversation_id: conversation_id.clone(),
                        actor_id: actor_id.clone(),
                    });
                }
                match destination.child.as_ref() {
                    Some(ConversationChildIdentity::Topic { root_message_id }) => {
                        let topic_exists = community.is_some_and(|community| {
                            community.topics.contains_key(root_message_id)
                                || topic_id_from_root(&MessageId(root_message_id.clone()))
                                    .is_some_and(|topic_id| community.topics.contains_key(topic_id))
                        }) || self
                            .state
                            .messages
                            .get(&conversation_id)
                            .is_some_and(|messages| messages.contains_key(&MessageId(root_message_id.clone())));
                        if !topic_exists {
                            return Err(EngineError::InvalidConversationChildDestination);
                        }
                    }
                    Some(ConversationChildIdentity::SavedSublist { participant_id }) => {
                        // Telegram SavedSublist is owned either by self Saved Messages or by
                        // an explicit monoforum parent chat. Fabushi does not yet model the
                        // monoforum parent relation, so accept only the canonical self
                        // SavedMessages parent and fail closed for all other parent kinds.
                        let self_saved_messages = is_self_saved_messages(conversation, &actor_id);
                        let participant_exists = self.state.actors.contains_key(participant_id);
                        if !self_saved_messages || !participant_exists {
                            return Err(EngineError::InvalidConversationChildDestination);
                        }
                    }
                    Some(ConversationChildIdentity::Conversation {
                        conversation_id: child_conversation_id,
                    }) => {
                        let child = self.require_conversation(child_conversation_id)?;
                        let child_access = child.owner_id.as_ref() == Some(&actor_id)
                            || child
                                .participants
                                .iter()
                                .any(|participant| participant.actor_id == actor_id)
                            || self
                                .state
                                .communities
                                .get(child_conversation_id)
                                .is_some_and(|community| community_has_access(community, &actor_id));
                        if !child_access {
                            return Err(EngineError::CommunityAccessDenied {
                                conversation_id: child_conversation_id.clone(),
                                actor_id: actor_id.clone(),
                            });
                        }
                    }
                    None => {}
                }
                let message_conversation_id = destination_message_conversation_id(&destination);
                let message = self.require_message(message_conversation_id, &message_id)?;
                match &destination.child {
                    Some(ConversationChildIdentity::Topic { root_message_id }) => {
                        let belongs = message
                            .thread_root_message_id
                            .as_ref()
                            .is_some_and(|root| root.0 == *root_message_id)
                            || message.id.0 == *root_message_id;
                        if !belongs {
                            return Err(EngineError::ConversationChildMessageMismatch);
                        }
                    }
                    Some(ConversationChildIdentity::SavedSublist { participant_id }) => {
                        if message.forward_origin.is_some()
                            && self
                                .saved_sublist_participant_for_message(
                                    &destination.conversation_id,
                                    &message_id,
                                )
                                .as_ref()
                                != Some(participant_id)
                        {
                            return Err(EngineError::ConversationChildMessageMismatch);
                        }
                    }
                    _ => {}
                }
                Ok(vec![Event::ConversationChildReadChanged {
                    destination,
                    actor_id,
                    message_id,
                }])
            }
            Command::SetTopicDraft { draft } => {
                self.require_actor(&draft.actor_id)?;
                let community = self
                    .state
                    .communities
                    .get(&draft.conversation_id)
                    .ok_or_else(|| EngineError::CommunityNotFound(draft.conversation_id.clone()))?;
                if !community_has_access(community, &draft.actor_id)
                    && self
                        .state
                        .conversations
                        .get(&draft.conversation_id)
                        .and_then(|conversation| conversation.owner_id.as_ref())
                        != Some(&draft.actor_id)
                {
                    return Err(EngineError::CommunityAccessDenied {
                        conversation_id: draft.conversation_id,
                        actor_id: draft.actor_id,
                    });
                }
                if draft.topic_id.trim().is_empty()
                    || !community.topics.contains_key(&draft.topic_id)
                {
                    return Err(EngineError::ForumTopicNotFound {
                        conversation_id: draft.conversation_id,
                        topic_id: draft.topic_id,
                    });
                }
                Ok(vec![Event::TopicDraftChanged { draft }])
            }
            Command::SetConversationChildDraft {
                destination,
                actor_id,
                text,
                reply_to_message_id,
                updated_at_ms,
            } => {
                if !destination.is_valid() {
                    return Err(EngineError::InvalidConversationChildDestination);
                }
                self.require_actor(&actor_id)?;
                let conversation_id = destination.conversation_id.clone();
                let conversation = self.require_conversation(&conversation_id)?;
                let community = self.state.communities.get(&conversation_id);
                let has_access = community
                    .map(|community| {
                        community_has_access(community, &actor_id)
                            || conversation.owner_id.as_ref() == Some(&actor_id)
                    })
                    .unwrap_or_else(|| {
                        conversation.owner_id.as_ref() == Some(&actor_id)
                            || conversation
                                .participants
                                .iter()
                                .any(|participant| participant.actor_id == actor_id)
                    });
                if !has_access {
                    return Err(EngineError::CommunityAccessDenied {
                        conversation_id: conversation_id.clone(),
                        actor_id: actor_id.clone(),
                    });
                }
                match destination.child.as_ref() {
                    Some(ConversationChildIdentity::Topic { root_message_id }) => {
                        let topic_exists = community.is_some_and(|community| {
                            community.topics.contains_key(root_message_id)
                                || topic_id_from_root(&MessageId(root_message_id.clone()))
                                    .is_some_and(|topic_id| community.topics.contains_key(topic_id))
                        }) || self
                            .state
                            .messages
                            .get(&conversation_id)
                            .is_some_and(|messages| messages.contains_key(&MessageId(root_message_id.clone())));
                        if !topic_exists {
                            return Err(EngineError::InvalidConversationChildDestination);
                        }
                    }
                    Some(ConversationChildIdentity::SavedSublist { participant_id }) => {
                        // Telegram SavedSublist is owned either by self Saved Messages or by
                        // an explicit monoforum parent chat. Fabushi does not yet model the
                        // monoforum parent relation, so accept only the canonical self
                        // SavedMessages parent and fail closed for all other parent kinds.
                        let self_saved_messages = is_self_saved_messages(conversation, &actor_id);
                        let participant_exists = self.state.actors.contains_key(participant_id);
                        if !self_saved_messages || !participant_exists {
                            return Err(EngineError::InvalidConversationChildDestination);
                        }
                    }
                    Some(ConversationChildIdentity::Conversation {
                        conversation_id: child_conversation_id,
                    }) => {
                        let child = self.require_conversation(child_conversation_id)?;
                        let child_access = child.owner_id.as_ref() == Some(&actor_id)
                            || child
                                .participants
                                .iter()
                                .any(|participant| participant.actor_id == actor_id)
                            || self
                                .state
                                .communities
                                .get(child_conversation_id)
                                .is_some_and(|community| community_has_access(community, &actor_id));
                        if !child_access {
                            return Err(EngineError::CommunityAccessDenied {
                                conversation_id: child_conversation_id.clone(),
                                actor_id: actor_id.clone(),
                            });
                        }
                    }
                    None => {}
                }
                if let Some(reply_to_message_id) = &reply_to_message_id {
                    self.require_message(
                        destination_message_conversation_id(&destination),
                        reply_to_message_id,
                    )?;
                }
                Ok(vec![Event::ConversationChildDraftChanged {
                    destination,
                    actor_id,
                    text,
                    reply_to_message_id: reply_to_message_id.map(|id| id.0),
                    updated_at_ms,
                }])
            }
            Command::ReplaceConversationChildWindow {
                destination,
                actor_id,
                message_ids,
                skipped_before,
                skipped_after,
                full_count,
            } => {
                self.decide(Command::SetConversationChildDraft {
                    destination: destination.clone(),
                    actor_id: actor_id.clone(),
                    text: String::new(),
                    reply_to_message_id: None,
                    updated_at_ms: 0,
                })?;
                if let Some(ConversationChildIdentity::SavedSublist { participant_id }) =
                    &destination.child
                {
                    for message_id in &message_ids {
                        if self
                            .saved_sublist_participant_for_message(
                                &destination.conversation_id,
                                message_id,
                            )
                            .as_ref()
                            != Some(participant_id)
                        {
                            return Err(EngineError::ConversationChildMessageMismatch);
                        }
                    }
                }
                for message_id in &message_ids {
                    self.decide(Command::MarkConversationChildRead {
                        destination: destination.clone(),
                        actor_id: actor_id.clone(),
                        message_id: message_id.clone(),
                    })?;
                }
                let message_ids = message_ids
                    .into_iter()
                    .map(|message_id| message_id.0)
                    .collect::<Vec<_>>();
                let mut pagination = crate::conversation::ConversationChildPaginationState::default();
                if !pagination.replace_window(
                    message_ids.clone(),
                    skipped_before,
                    skipped_after,
                    full_count,
                ) {
                    return Err(EngineError::InvalidConversationChildDestination);
                }
                Ok(vec![Event::ConversationChildWindowReplaced {
                    destination,
                    actor_id,
                    message_ids,
                    skipped_before,
                    skipped_after,
                    full_count,
                }])
            }
            Command::SetConversationChildPinned {
                destination,
                actor_id,
                pinned,
            } => {
                self.decide(Command::SetConversationChildDraft {
                    destination: destination.clone(),
                    actor_id: actor_id.clone(),
                    text: String::new(),
                    reply_to_message_id: None,
                    updated_at_ms: 0,
                })?;
                Ok(vec![Event::ConversationChildPinnedChanged {
                    destination,
                    actor_id,
                    pinned,
                }])
            }
            Command::SetConversationChildActive {
                destination,
                actor_id,
                active,
            } => {
                self.decide(Command::SetConversationChildDraft {
                    destination: destination.clone(),
                    actor_id: actor_id.clone(),
                    text: String::new(),
                    reply_to_message_id: None,
                    updated_at_ms: 0,
                })?;
                Ok(vec![Event::ConversationChildActiveChanged {
                    destination,
                    actor_id,
                    active,
                }])
            }
            Command::SetConversationChildMarkedUnread {
                destination,
                actor_id,
                marked_unread,
            } => {
                self.decide(Command::SetConversationChildDraft {
                    destination: destination.clone(),
                    actor_id: actor_id.clone(),
                    text: String::new(),
                    reply_to_message_id: None,
                    updated_at_ms: 0,
                })?;
                let conversation = self.require_conversation(&destination.conversation_id)?;
                let currently_unread = self
                    .state
                    .conversation_child_states
                    .iter()
                    .find(|state| state.destination == destination && state.actor_id == actor_id)
                    .is_some_and(|state| state.marked_unread || state.unread_count.unwrap_or(0) > 0);
                let context = crate::conversation::ConversationChildUnreadContext {
                    parent_is_self: is_self_saved_messages(conversation, &actor_id),
                    parent_is_community: self.state.communities.contains_key(&destination.conversation_id),
                    actor_is_monoforum_admin: false,
                };
                if !destination.can_toggle_unread(currently_unread, context) {
                    return Err(EngineError::InvalidConversationChildDestination);
                }
                Ok(vec![Event::ConversationChildMarkedUnreadChanged {
                    destination,
                    actor_id,
                    marked_unread,
                }])
            }
            Command::SetConversationChildNoPaidMessages {
                destination,
                actor_id,
                no_paid_messages,
            } => {
                self.decide(Command::SetConversationChildDraft {
                    destination: destination.clone(),
                    actor_id: actor_id.clone(),
                    text: String::new(),
                    reply_to_message_id: None,
                    updated_at_ms: 0,
                })?;
                if !matches!(
                    &destination.child,
                    Some(ConversationChildIdentity::SavedSublist { .. })
                ) {
                    return Err(EngineError::InvalidConversationChildDestination);
                }
                Ok(vec![Event::ConversationChildNoPaidMessagesChanged {
                    destination,
                    actor_id,
                    no_paid_messages,
                }])
            }
            Command::DestroyConversationChild {
                destination,
                actor_id,
            } => {
                self.decide(Command::SetConversationChildDraft {
                    destination: destination.clone(),
                    actor_id: actor_id.clone(),
                    text: String::new(),
                    reply_to_message_id: None,
                    updated_at_ms: 0,
                })?;
                Ok(vec![Event::ConversationChildDestroyed {
                    destination,
                    actor_id,
                }])
            }
            Command::SetReaction {
                conversation_id,
                message_id,
                reaction,
            } => {
                self.require_message(&conversation_id, &message_id)?;
                if reaction.reaction.trim().is_empty() {
                    return Err(EngineError::InvalidClientMessageId);
                }
                Ok(vec![Event::ReactionUpdated {
                    conversation_id,
                    message_id,
                    reaction,
                }])
            }
            Command::PinMessage {
                conversation_id,
                message_id,
                pinned,
            } => {
                self.require_message(&conversation_id, &message_id)?;
                Ok(vec![Event::MessagePinned {
                    conversation_id,
                    message_id,
                    pinned,
                }])
            }
            Command::VotePoll {
                conversation_id,
                message_id,
                actor_id,
                option_ids,
            } => {
                self.require_actor(&actor_id)?;
                let message = self.require_message(&conversation_id, &message_id)?;
                let (valid_ids, multiple_answers) = match &message.content {
                    MessageContent::Poll {
                        options,
                        multiple_answers,
                        ..
                    } => (
                        options
                            .iter()
                            .map(|option| option.id.clone())
                            .collect::<BTreeSet<_>>(),
                        *multiple_answers,
                    ),
                    _ => return Err(EngineError::InvalidPollVote),
                };
                let selected = option_ids.into_iter().collect::<BTreeSet<_>>();
                if (!multiple_answers && selected.len() > 1)
                    || selected.iter().any(|id| !valid_ids.contains(id))
                {
                    return Err(EngineError::InvalidPollVote);
                }
                Ok(vec![Event::PollVoteChanged {
                    conversation_id,
                    message_id,
                    actor_id,
                    option_ids: selected.into_iter().collect(),
                }])
            }
            Command::CreateInvoice { invoice } => {
                if !invoice.is_valid() {
                    return Err(EngineError::InvalidInvoice);
                }
                self.require_actor(&invoice.seller_id)?;
                self.require_conversation(&invoice.conversation_id)?;
                Ok(vec![Event::InvoiceCreated { invoice }])
            }
            Command::UpsertOrder { order } => {
                if !self.state.invoices.contains_key(&order.invoice_id) {
                    return Err(EngineError::InvoiceNotFound(order.invoice_id));
                }
                self.require_actor(&order.buyer_id)?;
                Ok(vec![Event::OrderUpserted { order }])
            }
            Command::CheckoutInvoice {
                invoice_id,
                order_id,
                buyer_id,
                customer,
                created_at_ms,
            } => {
                let invoice = self
                    .state
                    .invoices
                    .get(&invoice_id)
                    .cloned()
                    .ok_or_else(|| EngineError::InvoiceNotFound(invoice_id.clone()))?;
                self.require_actor(&buyer_id)?;
                if invoice
                    .expires_at_ms
                    .is_some_and(|expires_at_ms| expires_at_ms <= created_at_ms)
                {
                    return Err(EngineError::InvoiceExpired(invoice_id));
                }
                if let Some(existing) = self.state.orders.get(&order_id) {
                    if existing.invoice_id == invoice.id && existing.buyer_id == buyer_id {
                        return Ok(vec![Event::OrderUpserted {
                            order: existing.clone(),
                        }]);
                    }
                    return Err(EngineError::OrderConflict(order_id));
                }
                let amount_minor = invoice
                    .checked_total_minor()
                    .ok_or(EngineError::InvalidInvoice)?;
                if amount_minor <= 0 {
                    return Err(EngineError::InvalidInvoice);
                }
                let buyer_account = wallet_account_id(&buyer_id);
                let seller_account = wallet_account_id(&invoice.seller_id);
                let mut wallet = self.state.wallet.clone();
                ensure_wallet_account(&mut wallet, &buyer_account, &buyer_id, created_at_ms)?;
                ensure_wallet_account(
                    &mut wallet,
                    &seller_account,
                    &invoice.seller_id,
                    created_at_ms,
                )?;
                let entry = wallet.transfer(
                    format!("checkout:{order_id}"),
                    &buyer_account,
                    &seller_account,
                    Money::new(&invoice.currency, amount_minor),
                    Some(invoice.id.clone()),
                    created_at_ms,
                )?;
                let order = PaymentOrder {
                    id: order_id,
                    invoice_id: invoice.id,
                    buyer_id,
                    status: PaymentStatus::Paid,
                    amount: Money::new(&invoice.currency, amount_minor),
                    customer,
                    provider_payment_id: Some(entry.id.clone()),
                    provider_receipt_url: Some(format!("fabushi://payments/receipts/{}", entry.id)),
                    created_at_ms,
                    updated_at_ms: created_at_ms,
                };
                Ok(vec![
                    Event::WalletChanged { wallet, entry },
                    Event::OrderUpserted { order },
                ])
            }
            Command::RefundOrder {
                order_id,
                seller_id,
                request_id,
                refunded_at_ms,
            } => {
                let mut order = self
                    .state
                    .orders
                    .get(&order_id)
                    .cloned()
                    .ok_or_else(|| EngineError::OrderNotFound(order_id.clone()))?;
                let invoice = self
                    .state
                    .invoices
                    .get(&order.invoice_id)
                    .cloned()
                    .ok_or_else(|| EngineError::InvoiceNotFound(order.invoice_id.clone()))?;
                if invoice.seller_id != seller_id {
                    return Err(EngineError::RefundForbidden(order_id));
                }
                if order.status == PaymentStatus::Refunded {
                    return Ok(vec![Event::OrderUpserted { order }]);
                }
                if order.status != PaymentStatus::Paid {
                    return Err(EngineError::OrderNotRefundable(order_id));
                }
                let original_entry_id = order
                    .provider_payment_id
                    .clone()
                    .ok_or_else(|| EngineError::OrderNotRefundable(order.id.clone()))?;
                let mut wallet = self.state.wallet.clone();
                let entry =
                    wallet.refund_transfer(request_id, &original_entry_id, refunded_at_ms)?;
                order.status = PaymentStatus::Refunded;
                order.updated_at_ms = refunded_at_ms;
                Ok(vec![
                    Event::WalletChanged { wallet, entry },
                    Event::OrderUpserted { order },
                ])
            }
            Command::CreditWalletSettlement {
                request_id,
                owner_id,
                amount,
                reference,
                settled_at_ms,
            } => {
                self.require_actor(&owner_id)?;
                let account_id = wallet_account_id(&owner_id);
                let mut wallet = self.state.wallet.clone();
                ensure_wallet_account(&mut wallet, &account_id, &owner_id, settled_at_ms)?;
                let entry =
                    wallet.credit(request_id, &account_id, amount, reference, settled_at_ms)?;
                Ok(vec![Event::WalletChanged { wallet, entry }])
            }
            Command::PublishStory { actor_id, story } => self.decide_publish_story(actor_id, story),
            Command::DeleteStory { actor_id, story_id } => self.decide_delete_story(actor_id, story_id),
            Command::ViewStory {
                actor_id,
                story_id,
                viewed_at_ms,
            } => self.decide_view_story(actor_id, story_id, viewed_at_ms),
            Command::ReactStory {
                actor_id,
                story_id,
                reaction,
            } => self.decide_react_story(actor_id, story_id, reaction),
            Command::UpdateCommunity {
                actor_id,
                community,
            } => self.decide_update_community(actor_id, community),
            Command::SubscribeChannel {
                actor_id,
                conversation_id,
                subscribed_at_ms,
            } => self.decide_subscribe_channel(actor_id, conversation_id, subscribed_at_ms),
            Command::UnsubscribeChannel {
                actor_id,
                conversation_id,
                unsubscribed_at_ms,
            } => self.decide_unsubscribe_channel(actor_id, conversation_id, unsubscribed_at_ms),
            Command::SetCommunitySlowMode {
                actor_id,
                conversation_id,
                seconds,
                changed_at_ms,
            } => self.decide_set_community_slow_mode(actor_id, conversation_id, seconds, changed_at_ms),
            Command::ModerateCommunityMember {
                actor_id,
                conversation_id,
                member,
                reason,
                decided_at_ms,
            } => self.decide_moderate_community_member(
                actor_id,
                conversation_id,
                member,
                reason,
                decided_at_ms,
            ),
            Command::SetCommunityMember {
                actor_id,
                conversation_id,
                member,
            } => self.decide_set_community_member(actor_id, conversation_id, member),
            Command::CreateInviteLink { actor_id, invite } => {
                self.decide_create_invite_link(actor_id, invite)
            }
            Command::RevokeInviteLink {
                actor_id,
                conversation_id,
                invite_id,
                revoked_at_ms,
            } => self.decide_revoke_invite_link(
                actor_id,
                conversation_id,
                invite_id,
                revoked_at_ms,
            ),
            Command::RequestCommunityJoin { actor_id, request } => {
                self.decide_request_community_join(actor_id, request)
            }
            Command::RespondCommunityJoin {
                actor_id,
                conversation_id,
                requester_id,
                approved,
                decided_at_ms,
            } => self.decide_respond_community_join(
                actor_id,
                conversation_id,
                requester_id,
                approved,
                decided_at_ms,
            ),
            Command::UpsertForumTopic { actor_id, topic } => {
                self.decide_upsert_forum_topic(actor_id, topic)
            }
            Command::DeleteForumTopic {
                actor_id,
                conversation_id,
                topic_id,
            } => self.decide_delete_forum_topic(actor_id, conversation_id, topic_id),
            Command::RegisterBot { actor_id, profile } => {
                let actor = self.require_actor(&actor_id)?;
                if profile.actor_id != actor_id
                    || !matches!(actor.kind, ActorKind::Bot | ActorKind::Assistant)
                {
                    return Err(EngineError::BotPermissionDenied);
                }
                let mut registry = self.state.bots.clone();
                registry
                    .register(profile.clone())
                    .map_err(|error| EngineError::Bot(error.to_string()))?;
                Ok(vec![Event::BotRegistryChanged {
                    registry,
                    profile: Some(profile),
                    execution: None,
                }])
            }
            Command::BeginBotInvocation {
                actor_id,
                invocation,
                created_at_ms,
            } => {
                if invocation.sender_id != actor_id {
                    return Err(EngineError::BotPermissionDenied);
                }
                self.require_actor(&actor_id)?;
                self.require_conversation(&invocation.conversation_id)?;
                let mut registry = self.state.bots.clone();
                let execution = registry
                    .begin_execution(&invocation, created_at_ms)
                    .map_err(|error| EngineError::Bot(error.to_string()))?;
                let profile = registry.bots.get(&execution.bot_id).cloned();
                Ok(vec![Event::BotRegistryChanged {
                    registry,
                    profile,
                    execution: Some(execution),
                }])
            }
            Command::FinishBotExecution {
                actor_id,
                execution_id,
                success,
                finished_at_ms,
                error,
            } => {
                let mut registry = self.state.bots.clone();
                let execution =
                    registry
                        .executions
                        .get(&execution_id)
                        .cloned()
                        .ok_or_else(|| {
                            EngineError::Bot(format!("execution not found: {execution_id}"))
                        })?;
                if execution.bot_id != actor_id {
                    return Err(EngineError::BotPermissionDenied);
                }
                registry
                    .finish_execution(&execution_id, success, finished_at_ms, error)
                    .map_err(|error| EngineError::Bot(error.to_string()))?;
                let execution = registry.executions.get(&execution_id).cloned();
                let profile = execution
                    .as_ref()
                    .and_then(|execution| registry.bots.get(&execution.bot_id))
                    .cloned();
                Ok(vec![Event::BotRegistryChanged {
                    registry,
                    profile,
                    execution,
                }])
            }
            Command::InstallMiniApp { manifest } => Ok(vec![Event::MiniAppInstalled { manifest }]),
            Command::GrantMiniApp { grant } => {
                if !self.state.mini_apps.contains_key(&grant.mini_app_id) {
                    return Err(EngineError::MiniAppNotFound(grant.mini_app_id));
                }
                self.require_actor(&grant.actor_id)?;
                Ok(vec![Event::MiniAppGrantUpdated { grant }])
            }
            Command::OpenMiniApp { session } => {
                if !self.state.mini_apps.contains_key(&session.mini_app_id) {
                    return Err(EngineError::MiniAppNotFound(session.mini_app_id));
                }
                let grant = self
                    .state
                    .mini_app_grants
                    .get(&(session.mini_app_id.clone(), session.actor_id.clone()));
                if session.granted_permissions.iter().any(|permission| {
                    !grant.is_some_and(|grant| grant.permissions.contains(permission))
                }) {
                    return Err(EngineError::MiniAppPermissionDenied(
                        *session
                            .granted_permissions
                            .iter()
                            .find(|permission| {
                                !grant.is_some_and(|grant| grant.permissions.contains(permission))
                            })
                            .expect("permission exists"),
                    ));
                }
                Ok(vec![Event::MiniAppOpened { session }])
            }
            Command::MiniAppCall {
                session_id,
                request_id,
                request,
            } => {
                let session = self
                    .state
                    .mini_app_sessions
                    .get(&session_id)
                    .ok_or_else(|| EngineError::MiniAppSessionNotFound(session_id.clone()))?;
                let grant = self
                    .state
                    .mini_app_grants
                    .get(&(session.mini_app_id.clone(), session.actor_id.clone()))
                    .cloned()
                    .unwrap_or_else(|| MiniAppGrant {
                        mini_app_id: session.mini_app_id.clone(),
                        actor_id: session.actor_id.clone(),
                        permissions: session.granted_permissions.clone(),
                        granted_at_ms: session.opened_at_ms,
                        expires_at_ms: None,
                    });
                let action = match authorize_miniapp_request(session, &grant, &request, session.opened_at_ms) {
                    Ok(action) => action,
                    Err(MiniAppDenial::PermissionDenied { permission }) => {
                        return Err(EngineError::MiniAppPermissionDenied(permission));
                    }
                    Err(_) => return Err(EngineError::MiniAppHostActionRequired),
                };
                let response = match action {
                    MiniAppAction::DirectResponse { response } => response,
                    _ => return Err(EngineError::MiniAppHostActionRequired),
                };
                Ok(vec![Event::MiniAppResponded {
                    session_id,
                    request_id,
                    response,
                }])
            }
        }
    }

    pub fn apply(&mut self, event: Event) {
        match event {
            Event::ActorUpserted { actor } => {
                self.state.actors.insert(actor.id.clone(), actor);
            }
            Event::PrivacySettingsUpdated { actor_id, settings } => {
                self.state.privacy.insert(actor_id, settings);
            }
            Event::PresenceUpdated { actor_id, presence } => {
                if let Some(actor) = self.state.actors.get_mut(&actor_id) {
                    actor.presence = presence;
                }
            }
            Event::ConversationUpserted { conversation } => {
                self.state
                    .conversations
                    .insert(conversation.id.clone(), conversation);
            }
            Event::ConversationInfoUpdated {
                conversation_id,
                title,
                description,
            } => {
                if let Some(conversation) = self.state.conversations.get_mut(&conversation_id) {
                    conversation.title = title;
                    conversation.description = description;
                }
            }
            Event::ConversationParticipantUpserted {
                conversation_id,
                participant,
            } => {
                if let Some(conversation) = self.state.conversations.get_mut(&conversation_id) {
                    if let Some(existing) = conversation
                        .participants
                        .iter_mut()
                        .find(|item| item.actor_id == participant.actor_id)
                    {
                        *existing = participant;
                    } else {
                        conversation.participants.push(participant);
                    }
                }
            }
            Event::ConversationParticipantRemoved {
                conversation_id,
                actor_id,
            } => {
                if let Some(conversation) = self.state.conversations.get_mut(&conversation_id) {
                    conversation
                        .participants
                        .retain(|participant| participant.actor_id != actor_id);
                }
                if let Some(cursors) = self.state.read_cursors.get_mut(&conversation_id) {
                    cursors.remove(&actor_id);
                }
                if let Some(drafts) = self.state.drafts.get_mut(&conversation_id) {
                    drafts.remove(&actor_id);
                }
                if let Some(topic_cursors) = self.state.topic_read_cursors.get_mut(&conversation_id)
                {
                    topic_cursors.remove(&actor_id);
                }
                if let Some(topic_drafts) = self.state.topic_drafts.get_mut(&conversation_id) {
                    topic_drafts.remove(&actor_id);
                }
                if let Some(marked) = self.state.marked_unread_by_actor.get_mut(&conversation_id) {
                    marked.remove(&actor_id);
                }
            }
            Event::ConversationArchived {
                conversation_id,
                archived,
            } => {
                if let Some(conversation) = self.state.conversations.get_mut(&conversation_id) {
                    conversation.archived = archived;
                }
            }
            Event::ConversationPinned {
                conversation_id,
                pinned,
            } => {
                if let Some(conversation) = self.state.conversations.get_mut(&conversation_id) {
                    conversation.pinned = pinned;
                }
            }
            Event::ConversationMarkedUnread {
                conversation_id,
                actor_id,
                marked_unread,
            } => {
                let actors = self
                    .state
                    .marked_unread_by_actor
                    .entry(conversation_id.clone())
                    .or_default();
                if marked_unread {
                    actors.insert(actor_id.clone());
                } else {
                    actors.remove(&actor_id);
                }
                if actors.is_empty() {
                    self.state.marked_unread_by_actor.remove(&conversation_id);
                }
                if let Some(conversation) = self.state.conversations.get_mut(&conversation_id) {
                    conversation.marked_unread = marked_unread;
                }
            }
            Event::DraftChanged { draft } => {
                if draft.text.trim().is_empty() && draft.reply_to_message_id.is_none() {
                    if let Some(by_actor) = self.state.drafts.get_mut(&draft.conversation_id) {
                        by_actor.remove(&draft.actor_id);
                        if by_actor.is_empty() {
                            self.state.drafts.remove(&draft.conversation_id);
                        }
                    }
                } else {
                    self.state
                        .drafts
                        .entry(draft.conversation_id.clone())
                        .or_default()
                        .insert(draft.actor_id.clone(), draft);
                }
            }
            Event::ConversationNotificationsUpdated {
                conversation_id,
                settings,
            } => {
                if let Some(conversation) = self.state.conversations.get_mut(&conversation_id) {
                    conversation.notification_settings = settings;
                }
            }
            Event::FolderUpserted { folder } => {
                self.state.folders.insert(folder.id.clone(), folder);
            }
            Event::FolderDeleted { folder_id } => {
                self.state.folders.remove(&folder_id);
                for conversation in self.state.conversations.values_mut() {
                    conversation.folder_ids.retain(|id| id != &folder_id);
                }
            }
            Event::PresenceTriggeredSendQueued { pending } => {
                self.state
                    .pending_presence_sends
                    .insert(pending.client_message_id.clone(), pending);
            }
            Event::PresenceTriggeredSendRemoved { client_message_id } => {
                self.state.pending_presence_sends.remove(&client_message_id);
            }
            Event::MessageQueued { message } => {
                if !matches!(message.delivery_state, DeliveryState::Failed { .. }) {
                    if let Some(conversation) =
                        self.state.conversations.get_mut(&message.conversation_id)
                    {
                        conversation.last_message_id = Some(message.id.0.clone());
                        conversation.updated_at_ms = message.created_at_ms;
                    }
                }
                self.state
                    .messages
                    .entry(message.conversation_id.clone())
                    .or_default()
                    .insert(message.id.clone(), message.clone());
                if !matches!(message.delivery_state, DeliveryState::Failed { .. }) {
                    if let Some(thread_root) = &message.thread_root_message_id {
                        if let Some(topic_id) = topic_id_from_root(thread_root) {
                            if let Some(topic) = self
                                .state
                                .communities
                                .get_mut(&message.conversation_id)
                                .and_then(|community| community.topics.get_mut(topic_id))
                            {
                                topic.last_message_id = Some(message.id.0.clone());
                            }
                        }
                    }
                    let saved_sublist_participant = self.saved_sublist_participant_for_message(
                        &message.conversation_id,
                        &message.id,
                    );
                    for child in &mut self.state.conversation_child_states {
                        let matches_child = match &child.destination.child {
                            Some(ConversationChildIdentity::Topic { root_message_id }) => {
                                child.destination.conversation_id == message.conversation_id
                                    && (message
                                        .thread_root_message_id
                                        .as_ref()
                                        .is_some_and(|root| root.0 == *root_message_id)
                                        || message.id.0 == *root_message_id)
                            }
                            Some(ConversationChildIdentity::SavedSublist { participant_id }) => {
                                child.destination.conversation_id == message.conversation_id
                                    && saved_sublist_participant.as_ref() == Some(participant_id)
                            }
                            Some(ConversationChildIdentity::Conversation { conversation_id }) => {
                                *conversation_id == message.conversation_id
                            }
                            None => false,
                        };
                        if matches_child {
                            child.note_non_empty();
                        }
                    }
                }
            }
            Event::MessageAcknowledged {
                conversation_id,
                local_message_id,
                server_message_id,
                accepted_at_ms,
            } => {
                if let Some(messages) = self.state.messages.get_mut(&conversation_id) {
                    if let Some(mut message) = messages.remove(&local_message_id) {
                        message.id = server_message_id.clone();
                        message.delivery_state = DeliveryState::Sent;
                        message.created_at_ms = accepted_at_ms;
                        messages.insert(server_message_id.clone(), message);
                    }
                    if local_message_id != server_message_id {
                        for message in messages.values_mut() {
                            if message.reply_to_message_id.as_ref() == Some(&local_message_id) {
                                message.reply_to_message_id = Some(server_message_id.clone());
                            }
                            if message.thread_root_message_id.as_ref() == Some(&local_message_id) {
                                message.thread_root_message_id = Some(server_message_id.clone());
                            }
                        }
                    }
                }
                if local_message_id != server_message_id {
                    if let Some(by_actor) = self.state.drafts.get_mut(&conversation_id) {
                        for draft in by_actor.values_mut() {
                            if draft.reply_to_message_id.as_deref()
                                == Some(local_message_id.0.as_str())
                            {
                                draft.reply_to_message_id = Some(server_message_id.0.clone());
                            }
                        }
                    }
                    if let Some(by_actor) = self.state.topic_drafts.get_mut(&conversation_id) {
                        for by_topic in by_actor.values_mut() {
                            for draft in by_topic.values_mut() {
                                if draft.reply_to_message_id.as_deref()
                                    == Some(local_message_id.0.as_str())
                                {
                                    draft.reply_to_message_id = Some(server_message_id.0.clone());
                                }
                            }
                        }
                    }
                    if let Some(by_actor) = self.state.read_cursors.get_mut(&conversation_id) {
                        for cursor in by_actor.values_mut() {
                            if *cursor == local_message_id {
                                *cursor = server_message_id.clone();
                            }
                        }
                    }
                    if let Some(by_actor) = self.state.topic_read_cursors.get_mut(&conversation_id)
                    {
                        for by_topic in by_actor.values_mut() {
                            for cursor in by_topic.values_mut() {
                                if *cursor == local_message_id {
                                    *cursor = server_message_id.clone();
                                }
                            }
                        }
                    }
                    for child in &mut self.state.conversation_child_states {
                        if destination_message_conversation_id(&child.destination)
                            == &conversation_id
                        {
                            child.rekey_message_id(
                                &local_message_id.0,
                                &server_message_id.0,
                                Some(accepted_at_ms),
                            );
                        }
                    }
                }
                if let Some(conversation) = self.state.conversations.get_mut(&conversation_id) {
                    if conversation.last_message_id.as_deref() == Some(local_message_id.0.as_str())
                    {
                        conversation.last_message_id = Some(server_message_id.0);
                    }
                }
            }
            Event::DeliveryStateUpdated {
                conversation_id,
                message_id,
                state,
            } => {
                if let Some(message) = self
                    .state
                    .messages
                    .get_mut(&conversation_id)
                    .and_then(|messages| messages.get_mut(&message_id))
                {
                    message.delivery_state = state;
                }
            }
            Event::MessageEdited {
                conversation_id,
                message_id,
                content,
                edited_at_ms,
            } => {
                if let Some(message) = self
                    .state
                    .messages
                    .get_mut(&conversation_id)
                    .and_then(|messages| messages.get_mut(&message_id))
                {
                    message.content = content;
                    message.edited_at_ms = Some(edited_at_ms);
                }
            }
            Event::MessagesDeleted {
                conversation_id,
                message_ids,
            } => {
                let deleted_ids = message_ids
                    .iter()
                    .map(|id| id.0.clone())
                    .collect::<BTreeSet<_>>();
                if let Some(messages) = self.state.messages.get_mut(&conversation_id) {
                    for id in message_ids {
                        if let Some(message) = messages.get_mut(&id) {
                            message.deleted = true;
                        }
                    }
                }
                if let Some(by_actor) = self.state.drafts.get_mut(&conversation_id) {
                    for draft in by_actor.values_mut() {
                        if draft
                            .reply_to_message_id
                            .as_ref()
                            .is_some_and(|reply_id| deleted_ids.contains(reply_id))
                        {
                            draft.reply_to_message_id = None;
                        }
                    }
                    by_actor.retain(|_, draft| {
                        !draft.text.trim().is_empty() || draft.reply_to_message_id.is_some()
                    });
                    if by_actor.is_empty() {
                        self.state.drafts.remove(&conversation_id);
                    }
                }
                if let Some(by_actor) = self.state.topic_drafts.get_mut(&conversation_id) {
                    for by_topic in by_actor.values_mut() {
                        for draft in by_topic.values_mut() {
                            if draft
                                .reply_to_message_id
                                .as_ref()
                                .is_some_and(|reply_id| deleted_ids.contains(reply_id))
                            {
                                draft.reply_to_message_id = None;
                            }
                        }
                        by_topic.retain(|_, draft| {
                            !draft.text.trim().is_empty() || draft.reply_to_message_id.is_some()
                        });
                    }
                    by_actor.retain(|_, by_topic| !by_topic.is_empty());
                    if by_actor.is_empty() {
                        self.state.topic_drafts.remove(&conversation_id);
                    }
                }
                for child in &mut self.state.conversation_child_states {
                    if destination_message_conversation_id(&child.destination) == &conversation_id {
                        child.remove_deleted_messages(&deleted_ids);
                    }
                }
            }
            Event::ConversationRead {
                conversation_id,
                actor_id,
                message_id,
            } => {
                self.state
                    .read_cursors
                    .entry(conversation_id.clone())
                    .or_default()
                    .insert(actor_id.clone(), message_id.clone());
                if let Some(actors) = self.state.marked_unread_by_actor.get_mut(&conversation_id) {
                    actors.remove(&actor_id);
                    if actors.is_empty() {
                        self.state.marked_unread_by_actor.remove(&conversation_id);
                    }
                }
                // Keep legacy snapshot fields coherent for older readers. Actor-specific
                // clients receive the authoritative read state from the projected sync view.
                if let Some(conversation) = self.state.conversations.get_mut(&conversation_id) {
                    conversation.last_read_message_id = Some(message_id.0);
                    conversation.unread_count = 0;
                    conversation.marked_unread = false;
                }
            }
            Event::TopicReadChanged {
                conversation_id,
                topic_id,
                actor_id,
                message_id,
            } => {
                let position = self
                    .state
                    .messages
                    .get(&conversation_id)
                    .and_then(|messages| messages.get(&message_id))
                    .map(|message| {
                        ConversationMessagePosition::new(
                            message.created_at_ms,
                            message.id.0.clone(),
                        )
                    });
                if let Some(position) = position {
                    let destination = ConversationDestination::topic(
                        conversation_id.clone(),
                        format!("topic:{topic_id}"),
                    );
                    if let Some(child) =
                        self.state.child_state_mut(destination, actor_id.clone())
                    {
                        let _ = child.advance_inbox_read_till(position, None);
                    }
                }
                // Protocol compatibility projection while topic callers migrate to
                // the source-neutral child runtime state above.
                self.state
                    .topic_read_cursors
                    .entry(conversation_id)
                    .or_default()
                    .entry(actor_id)
                    .or_default()
                    .insert(topic_id, message_id);
            }
            Event::ConversationChildReadChanged {
                destination,
                actor_id,
                message_id,
            } => {
                let position = self
                    .state
                    .messages
                    .get(destination_message_conversation_id(&destination))
                    .and_then(|messages| messages.get(&message_id))
                    .map(|message| {
                        ConversationMessagePosition::new(
                            message.created_at_ms,
                            message.id.0.clone(),
                        )
                    });
                if let Some(position) = position {
                    if let Some(child) = self.state.child_state_mut(destination, actor_id) {
                        let _ = child.advance_inbox_read_till(position, None);
                    }
                }
            }
            Event::TopicDraftChanged { draft } => {
                let destination = ConversationDestination::topic(
                    draft.conversation_id.clone(),
                    format!("topic:{}", draft.topic_id),
                );
                if let Some(child) = self
                    .state
                    .child_state_mut(destination, draft.actor_id.clone())
                {
                    if draft.text.trim().is_empty() && draft.reply_to_message_id.is_none() {
                        child.clear_draft();
                    } else {
                        child.set_draft(
                            draft.text.clone(),
                            draft.reply_to_message_id.clone(),
                            draft.updated_at_ms,
                        );
                    }
                }
                if draft.text.trim().is_empty() && draft.reply_to_message_id.is_none() {
                    if let Some(by_actor) = self.state.topic_drafts.get_mut(&draft.conversation_id)
                    {
                        if let Some(by_topic) = by_actor.get_mut(&draft.actor_id) {
                            by_topic.remove(&draft.topic_id);
                            if by_topic.is_empty() {
                                by_actor.remove(&draft.actor_id);
                            }
                        }
                        if by_actor.is_empty() {
                            self.state.topic_drafts.remove(&draft.conversation_id);
                        }
                    }
                } else {
                    self.state
                        .topic_drafts
                        .entry(draft.conversation_id.clone())
                        .or_default()
                        .entry(draft.actor_id.clone())
                        .or_default()
                        .insert(draft.topic_id.clone(), draft);
                }
            }
            Event::ConversationChildDraftChanged {
                destination,
                actor_id,
                text,
                reply_to_message_id,
                updated_at_ms,
            } => {
                if let Some(child) = self.state.child_state_mut(destination, actor_id) {
                    if text.trim().is_empty() && reply_to_message_id.is_none() {
                        child.clear_draft();
                    } else {
                        child.set_draft(text, reply_to_message_id, updated_at_ms);
                    }
                }
            }
            Event::ConversationChildWindowReplaced {
                destination,
                actor_id,
                message_ids,
                skipped_before,
                skipped_after,
                full_count,
            } => {
                if let Some(child) = self.state.child_state_mut(destination, actor_id) {
                    let empty = message_ids.is_empty();
                    if child.pagination.replace_window(
                        message_ids,
                        skipped_before,
                        skipped_after,
                        full_count,
                    ) {
                        if empty {
                            child.note_locally_empty();
                        } else {
                            child.note_non_empty();
                        }
                    }
                }
            }
            Event::ConversationChildPinnedChanged {
                destination,
                actor_id,
                pinned,
            } => {
                if let Some(child) = self.state.child_state_mut(destination, actor_id) {
                    child.pinned = pinned;
                    if pinned {
                        child.restore_pinned_when_non_empty = false;
                    }
                }
            }
            Event::ConversationChildActiveChanged {
                destination,
                actor_id,
                active,
            } => {
                if active {
                    for child in &mut self.state.conversation_child_states {
                        if child.actor_id == actor_id
                            && child.destination.conversation_id == destination.conversation_id
                        {
                            child.active = false;
                        }
                    }
                }
                if let Some(child) = self.state.child_state_mut(destination, actor_id) {
                    child.set_active(active);
                }
            }
            Event::ConversationChildMarkedUnreadChanged {
                destination,
                actor_id,
                marked_unread,
            } => {
                if let Some(child) = self.state.child_state_mut(destination, actor_id) {
                    child.marked_unread = marked_unread;
                }
            }
            Event::ConversationChildNoPaidMessagesChanged {
                destination,
                actor_id,
                no_paid_messages,
            } => {
                if let Some(child) = self.state.child_state_mut(destination, actor_id) {
                    child.no_paid_messages = no_paid_messages;
                }
            }
            Event::ConversationChildDestroyed {
                destination,
                actor_id,
            } => {
                self.state.conversation_child_states.retain(|child| {
                    child.destination != destination || child.actor_id != actor_id
                });
            }
            Event::ReactionUpdated {
                conversation_id,
                message_id,
                reaction,
            } => {
                if let Some(message) = self
                    .state
                    .messages
                    .get_mut(&conversation_id)
                    .and_then(|messages| messages.get_mut(&message_id))
                {
                    message
                        .reactions
                        .retain(|item| item.reaction != reaction.reaction);
                    if reaction.count > 0 {
                        message.reactions.push(reaction);
                    }
                }
            }
            Event::MessagePinned {
                conversation_id,
                message_id,
                pinned,
            } => {
                if let Some(message) = self
                    .state
                    .messages
                    .get_mut(&conversation_id)
                    .and_then(|messages| messages.get_mut(&message_id))
                {
                    message.pinned = pinned;
                }
                if let Some(conversation) = self.state.conversations.get_mut(&conversation_id) {
                    conversation
                        .pinned_message_ids
                        .retain(|id| id != &message_id.0);
                    if pinned {
                        conversation.pinned_message_ids.push(message_id.0);
                    }
                }
            }
            Event::PollVoteChanged {
                conversation_id,
                message_id,
                actor_id,
                option_ids,
            } => {
                let selected = option_ids.into_iter().collect::<BTreeSet<_>>();
                let by_message = self
                    .state
                    .poll_votes
                    .entry(conversation_id.clone())
                    .or_default();
                let by_actor = by_message.entry(message_id.clone()).or_default();
                if selected.is_empty() {
                    by_actor.remove(&actor_id);
                } else {
                    by_actor.insert(actor_id, selected);
                }
                if let Some(message) = self
                    .state
                    .messages
                    .get_mut(&conversation_id)
                    .and_then(|messages| messages.get_mut(&message_id))
                {
                    if let MessageContent::Poll { options, .. } = &mut message.content {
                        for option in options {
                            option.voter_count = u32::try_from(
                                by_actor
                                    .values()
                                    .filter(|votes| votes.contains(&option.id))
                                    .count(),
                            )
                            .unwrap_or(u32::MAX);
                            option.chosen = false;
                        }
                    }
                }
            }
            Event::InvoiceCreated { invoice } => {
                self.state.invoices.insert(invoice.id.clone(), invoice);
            }
            Event::OrderUpserted { order } => {
                self.state.orders.insert(order.id.clone(), order);
            }
            Event::WalletChanged { wallet, .. } => {
                self.state.wallet = wallet;
            }
            Event::StoryChanged { story } => {
                self.state.stories.insert(story.id.clone(), story);
            }
            Event::StoryDeleted { story_id } => {
                self.state.stories.remove(&story_id);
            }
            Event::CommunityChanged { community } => {
                self.state
                    .communities
                    .insert(community.conversation_id.clone(), community);
            }
            Event::BotRegistryChanged { registry, .. } => {
                self.state.bots = registry;
            }
            Event::MiniAppInstalled { manifest } => {
                self.state.mini_apps.insert(manifest.id.clone(), manifest);
            }
            Event::MiniAppGrantUpdated { grant } => {
                self.state
                    .mini_app_grants
                    .insert((grant.mini_app_id.clone(), grant.actor_id.clone()), grant);
            }
            Event::MiniAppOpened { session } => {
                self.state
                    .mini_app_sessions
                    .insert(session.id.clone(), session);
            }
            Event::MiniAppResponded { .. } => {}
        }
    }

    pub fn complete_paid_order(
        &mut self,
        order_id: &str,
        updated_at_ms: i64,
    ) -> Result<Vec<Event>, EngineError> {
        let mut order = self
            .state
            .orders
            .get(order_id)
            .cloned()
            .ok_or_else(|| EngineError::OrderNotFound(order_id.to_string()))?;
        order.status = PaymentStatus::Paid;
        order.updated_at_ms = updated_at_ms;
        self.execute(Command::UpsertOrder { order })
    }

    pub(crate) fn require_actor(&self, id: &ActorId) -> Result<&Actor, EngineError> {
        self.state
            .actors
            .get(id)
            .ok_or_else(|| EngineError::ActorNotFound(id.clone()))
    }
    pub(crate) fn require_conversation(&self, id: &ConversationId) -> Result<&Conversation, EngineError> {
        self.state
            .conversations
            .get(id)
            .ok_or_else(|| EngineError::ConversationNotFound(id.clone()))
    }
    pub(crate) fn require_message(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
    ) -> Result<&Message, EngineError> {
        self.state
            .messages
            .get(conversation_id)
            .and_then(|messages| messages.get(message_id))
            .ok_or_else(|| EngineError::MessageNotFound {
                conversation_id: conversation_id.clone(),
                message_id: message_id.clone(),
            })
    }
}
