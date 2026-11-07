//! Target Symbol: DataPeerBotCommands
//! Telegram Desktop 1:1 Submodule: `DataPeerBotCommands`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::data::data_peer_id::UserId;
use crate::data::data_peer_bot_command::BotCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum BotStatus {
    NoBots = -1,
    #[default]
    Unknown = 0,
    HasBots = 2,
}

/// Commands exposed by a specific bot user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BotCommands {
    pub user_id: UserId,
    pub commands: Vec<BotCommand>,
}

/// Target Symbol for Module 03 Ledger: DataPeerBotCommands container.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataPeerBotCommands {
    pub bot_commands_map: HashMap<UserId, Vec<BotCommand>>,
}

impl DataPeerBotCommands {
    pub fn new() -> Self {
        Self {
            bot_commands_map: HashMap::new(),
        }
    }

    pub fn update(&mut self, list: Vec<BotCommands>) -> bool {
        let mut changed = false;
        for entry in list {
            if self.bot_commands_map.insert(entry.user_id, entry.commands).is_none() {
                changed = true;
            }
        }
        changed
    }

    pub fn get_commands(&self, user_id: UserId) -> Option<&[BotCommand]> {
        self.bot_commands_map.get(&user_id).map(|v| v.as_slice())
    }

    pub fn all_commands(&self) -> Vec<&BotCommand> {
        self.bot_commands_map.values().flat_map(|cmds| cmds.iter()).collect()
    }
}
