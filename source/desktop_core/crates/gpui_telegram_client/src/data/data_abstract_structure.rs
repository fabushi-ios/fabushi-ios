//! Target Symbol: DataAbstractStructure
//! Telegram Desktop 1:1 Submodule: `DataAbstractStructure`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

/// Global abstract structure lifecycle tracking container.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataAbstractStructure {
    pub initialized: bool,
    pub name: String,
}

impl DataAbstractStructure {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_name(name: impl Into<String>) -> Self {
        Self {
            initialized: true,
            name: name.into(),
        }
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}
