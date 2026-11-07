//! Target Symbol: DataPeerBotCommand
//! Telegram Desktop 1:1 Submodule: `DataPeerBotCommand`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

/// Telegram bot slash command specification.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct BotCommand {
    pub command: String,
    pub description: String,
    pub ephemeral: bool,
}

impl BotCommand {
    pub fn new(command: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            description: description.into(),
            ephemeral: false,
        }
    }

    pub fn with_ephemeral(mut self, ephemeral: bool) -> Self {
        self.ephemeral = ephemeral;
        self
    }
}

/// Target Symbol for Module 03 Ledger: DataPeerBotCommand coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataPeerBotCommand {
    pub command: BotCommand,
}

impl DataPeerBotCommand {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_command(command: BotCommand) -> Self {
        Self { command }
    }
}
