//! Target Symbol: DataSession
//! Telegram Desktop 1:1 Submodule: `DataSession`
//! Subsystem: `data`, Feature ID: `TCG-N02`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::{PeerId, UserId, ChatId, ChannelId};
use crate::data::data_peer::{PeerData, DataPeer};
use crate::data::data_user::{UserData, DataUser};
use crate::data::data_chat::{ChatData, DataChat};
use crate::data::data_channel::{ChannelData, DataChannel, ChannelDataFlags};
use crate::data::data_histories::{History, DataHistories};
use crate::data::data_folder::{Folder, DataFolder};
use crate::data::data_chat_filters::{ChatFilter, DataChatFilters};

/// Central domain repository and manager for all Telegram peer entities,
/// chat histories, folders, and filters within an authenticated session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataSession {
    pub peers: DataPeer,
    pub users: DataUser,
    pub chats: DataChat,
    pub channels: DataChannel,
    pub histories: DataHistories,
    pub folders: DataFolder,
    pub filters: DataChatFilters,
}

impl DataSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn user(&mut self, user_id: UserId) -> &mut UserData {
        self.users.get_or_insert(user_id)
    }

    pub fn user_loaded(&self, user_id: UserId) -> Option<&UserData> {
        self.users.find(user_id)
    }

    pub fn chat(&mut self, chat_id: ChatId) -> &mut ChatData {
        self.chats.get_or_insert(chat_id)
    }

    pub fn chat_loaded(&self, chat_id: ChatId) -> Option<&ChatData> {
        self.chats.find(chat_id)
    }

    pub fn channel(&mut self, channel_id: ChannelId) -> &mut ChannelData {
        self.channels.get_or_insert(channel_id)
    }

    pub fn channel_loaded(&self, channel_id: ChannelId) -> Option<&ChannelData> {
        self.channels.find(channel_id)
    }

    pub fn peer(&mut self, peer_id: PeerId) -> &mut PeerData {
        self.peers.get_or_insert(peer_id)
    }

    pub fn history(&mut self, peer_id: PeerId) -> &mut History {
        self.histories.get_or_create(peer_id)
    }

    pub fn process_user(
        &mut self,
        user_id: UserId,
        first_name: &str,
        last_name: &str,
        phone: &str,
        username: Option<&str>,
    ) -> &UserData {
        let u = self.user(user_id);
        u.set_name(first_name, last_name);
        u.phone = phone.to_string();
        if let Some(uname) = username {
            u.usernames.set_single(uname);
        }
        self.user_loaded(user_id).unwrap()
    }

    pub fn process_chat(
        &mut self,
        chat_id: ChatId,
        title: &str,
        creator: UserId,
        participants: Vec<UserId>,
    ) -> &ChatData {
        let c = self.chat(chat_id);
        c.title = title.to_string();
        c.creator = creator;
        c.count = participants.len();
        c.participants = participants;
        self.chat_loaded(chat_id).unwrap()
    }

    pub fn process_channel(
        &mut self,
        channel_id: ChannelId,
        title: &str,
        username: Option<&str>,
        is_megagroup: bool,
    ) -> &ChannelData {
        let ch = self.channel(channel_id);
        ch.title = title.to_string();
        ch.username = username.map(|s| s.to_string());
        if is_megagroup {
            ch.flags.add(ChannelDataFlags::MEGAGROUP);
        } else {
            ch.flags.add(ChannelDataFlags::BROADCAST);
        }
        self.channel_loaded(channel_id).unwrap()
    }

    pub fn add_folder(&mut self, folder: Folder) {
        self.folders.folders.push(folder);
    }

    pub fn add_filter(&mut self, filter: ChatFilter) {
        self.filters.add(filter);
    }

    pub fn clear(&mut self) {
        self.peers.peers.clear();
        self.users.users.clear();
        self.chats.chats.clear();
        self.channels.channels.clear();
        self.histories.histories.clear();
        self.folders.folders.clear();
        self.filters.filters.clear();
    }
}
