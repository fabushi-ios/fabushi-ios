//! Precompiled header definitions and core exports for the Gram subsystem.
//! Upstream: Telegram/SourceFiles/gram/gram_pch.h

#![allow(dead_code, unused_imports)]

pub use std::collections::{BTreeMap, BTreeSet};
pub use serde::{Deserialize, Serialize};

/// Precompiled header container for the Gram TON subsystem.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GramPch {
    pub subsystem_initialized: bool,
    pub api_version: u32,
}

impl GramPch {
    pub const API_VERSION: u32 = 2;

    pub fn new() -> Self {
        Self {
            subsystem_initialized: true,
            api_version: Self::API_VERSION,
        }
    }

    pub fn is_ready(&self) -> bool {
        self.subsystem_initialized && self.api_version >= Self::API_VERSION
    }
}
