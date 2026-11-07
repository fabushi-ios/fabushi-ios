//! Target Symbol: DataPeerId
//! Telegram Desktop 1:1 Submodule: `DataPeerId`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

/// 64-bit integer holding raw numerical ID without type tags.
pub type BareId = u64;

/// UserId with 0 shift discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct UserId(pub BareId);

impl UserId {
    pub const fn new(id: BareId) -> Self {
        Self(id)
    }

    pub const fn bare(self) -> BareId {
        self.0
    }

    pub const fn is_valid(self) -> bool {
        self.0 != 0
    }
}

/// ChatId for legacy small group chats (shift discriminator = 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct ChatId(pub BareId);

impl ChatId {
    pub const fn new(id: BareId) -> Self {
        Self(id)
    }

    pub const fn bare(self) -> BareId {
        self.0
    }

    pub const fn is_valid(self) -> bool {
        self.0 != 0
    }
}

/// ChannelId for broadcast channels and supergroups (shift discriminator = 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct ChannelId(pub BareId);

impl ChannelId {
    pub const fn new(id: BareId) -> Self {
        Self(id)
    }

    pub const fn bare(self) -> BareId {
        self.0
    }

    pub const fn is_valid(self) -> bool {
        self.0 != 0
    }
}

/// FakeChatId for synthetic or secret chat dialog entries (shift discriminator = 0x7F).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct FakeChatId(pub BareId);

impl FakeChatId {
    pub const fn new(id: BareId) -> Self {
        Self(id)
    }

    pub const fn bare(self) -> BareId {
        self.0
    }
}

/// Peer type discriminator tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PeerType {
    User,
    Chat,
    Channel,
    FakeChat,
    Unknown(u8),
}

/// 64-bit PeerId packed with 8-bit discriminator in high bits [55:48]
/// and 48-bit bare identifier in low bits [47:0].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct PeerId {
    pub value: u64,
}

impl PeerId {
    pub const CHAT_TYPE_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
    pub const SHIFT_USER: u8 = 0;
    pub const SHIFT_CHAT: u8 = 1;
    pub const SHIFT_CHANNEL: u8 = 2;
    pub const SHIFT_FAKE: u8 = 0x7F;
    pub const RESERVED_BIT: u64 = 0x80 << 48;

    pub const ZERO: Self = Self { value: 0 };

    pub const fn new(value: u64) -> Self {
        Self { value }
    }

    pub const fn from_user(user_id: UserId) -> Self {
        Self {
            value: (user_id.0 & Self::CHAT_TYPE_MASK) | ((Self::SHIFT_USER as u64) << 48),
        }
    }

    pub const fn from_chat(chat_id: ChatId) -> Self {
        Self {
            value: (chat_id.0 & Self::CHAT_TYPE_MASK) | ((Self::SHIFT_CHAT as u64) << 48),
        }
    }

    pub const fn from_channel(channel_id: ChannelId) -> Self {
        Self {
            value: (channel_id.0 & Self::CHAT_TYPE_MASK) | ((Self::SHIFT_CHANNEL as u64) << 48),
        }
    }

    pub const fn from_fake(fake_id: FakeChatId) -> Self {
        Self {
            value: (fake_id.0 & Self::CHAT_TYPE_MASK) | ((Self::SHIFT_FAKE as u64) << 48),
        }
    }

    pub const fn is_valid(self) -> bool {
        self.value != 0
    }

    pub const fn shift(self) -> u8 {
        ((self.value >> 48) & 0xFF) as u8
    }

    pub const fn bare(self) -> BareId {
        self.value & Self::CHAT_TYPE_MASK
    }

    pub const fn peer_type(self) -> PeerType {
        match self.shift() {
            Self::SHIFT_USER => PeerType::User,
            Self::SHIFT_CHAT => PeerType::Chat,
            Self::SHIFT_CHANNEL => PeerType::Channel,
            Self::SHIFT_FAKE => PeerType::FakeChat,
            other => PeerType::Unknown(other),
        }
    }

    pub const fn is_user(self) -> bool {
        self.shift() == Self::SHIFT_USER && self.value != 0
    }

    pub const fn is_chat(self) -> bool {
        self.shift() == Self::SHIFT_CHAT
    }

    pub const fn is_channel(self) -> bool {
        self.shift() == Self::SHIFT_CHANNEL
    }

    pub const fn is_fake(self) -> bool {
        self.shift() == Self::SHIFT_FAKE
    }

    pub const fn to_user(self) -> Option<UserId> {
        if self.is_user() {
            Some(UserId(self.bare()))
        } else {
            None
        }
    }

    pub const fn to_chat(self) -> Option<ChatId> {
        if self.is_chat() {
            Some(ChatId(self.bare()))
        } else {
            None
        }
    }

    pub const fn to_channel(self) -> Option<ChannelId> {
        if self.is_channel() {
            Some(ChannelId(self.bare()))
        } else {
            None
        }
    }

    pub const fn to_fake(self) -> Option<FakeChatId> {
        if self.is_fake() {
            Some(FakeChatId(self.bare()))
        } else {
            None
        }
    }

    /// Serializes PeerId into storage format with reserved modern flag bit.
    pub const fn serialize(self) -> u64 {
        self.value | Self::RESERVED_BIT
    }

    /// Deserializes PeerId supporting both modern format and legacy 32-bit shift formats.
    pub fn deserialize(serialized: u64) -> Self {
        let is_modern = (serialized & Self::RESERVED_BIT) != 0;
        if is_modern {
            Self::new(serialized & !Self::RESERVED_BIT)
        } else {
            // Legacy format from earlier tdesktop database schemas:
            // High 32 bits held legacy shift: 0=User, 0x100000000=Chat, 0x200000000=Channel, 0xF00000000=Fake
            let peer_id_mask = 0x0000_0000_FFFF_FFFFu64;
            let peer_type_mask = 0x0000_000F_0000_0000u64;
            let shift = serialized & peer_type_mask;
            let bare = serialized & peer_id_mask;
            match shift {
                0x0000_0000_0000_0000 => Self::from_user(UserId(bare)),
                0x0000_0001_0000_0000 => Self::from_chat(ChatId(bare)),
                0x0000_0002_0000_0000 => Self::from_channel(ChannelId(bare)),
                0x0000_000F_0000_0000 => Self::from_fake(FakeChatId(bare)),
                _ => Self::ZERO,
            }
        }
    }
}

impl std::fmt::Display for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.peer_type() {
            PeerType::User => write!(f, "user#{}", self.bare()),
            PeerType::Chat => write!(f, "chat#{}", self.bare()),
            PeerType::Channel => write!(f, "channel#{}", self.bare()),
            PeerType::FakeChat => write!(f, "fake#{}", self.bare()),
            PeerType::Unknown(s) => write!(f, "peer[0x{:02x}]#{}", s, self.bare()),
        }
    }
}

/// Target Symbol for Module 03 Ledger: DataPeerId manager & utility engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataPeerId {
    pub cached_peers: Vec<PeerId>,
}

impl DataPeerId {
    pub fn new() -> Self {
        Self {
            cached_peers: Vec::new(),
        }
    }

    pub fn register(&mut self, peer_id: PeerId) {
        if !self.cached_peers.contains(&peer_id) {
            self.cached_peers.push(peer_id);
        }
    }

    pub fn parse_user_id(id: u64) -> PeerId {
        PeerId::from_user(UserId(id))
    }

    pub fn parse_chat_id(id: u64) -> PeerId {
        PeerId::from_chat(ChatId(id))
    }

    pub fn parse_channel_id(id: u64) -> PeerId {
        PeerId::from_channel(ChannelId(id))
    }

    pub fn serialize_peer_id(peer_id: PeerId) -> u64 {
        peer_id.serialize()
    }

    pub fn deserialize_peer_id(serialized: u64) -> PeerId {
        PeerId::deserialize(serialized)
    }

    pub fn count(&self) -> usize {
        self.cached_peers.len()
    }
}
