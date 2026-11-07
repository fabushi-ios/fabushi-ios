//! TON Connect Persistent Claims & Authorized Origins Store
//! Upstream: `Telegram/SourceFiles/wallet/wallet_ton_connect_claims.cpp`, `wallet_ton_connect_claims.h`
//! Subsystem: `wallet`, Feature ID: `CAP-06`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const CLAIMS_STORAGE_KEY: &str = "presentation/ton-connect-claims";
pub const CLAIMS_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TonConnectClaim {
    pub origin: String,
    pub permissions_mask: u32,
    pub granted_at_ms: u64,
    pub expires_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletTonConnectClaims {
    pub claims: HashMap<String, TonConnectClaim>,
    pub version: u32,
}

impl Default for WalletTonConnectClaims {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletTonConnectClaims {
    pub fn new() -> Self {
        Self {
            claims: HashMap::new(),
            version: CLAIMS_FORMAT_VERSION,
        }
    }

    pub fn grant_claim(&mut self, origin: impl Into<String>, permissions_mask: u32, now_ms: u64, duration_ms: Option<u64>) {
        let orig = origin.into();
        let expires_at_ms = duration_ms.map(|d| now_ms.saturating_add(d));
        let claim = TonConnectClaim {
            origin: orig.clone(),
            permissions_mask,
            granted_at_ms: now_ms,
            expires_at_ms,
        };
        self.claims.insert(orig, claim);
    }

    pub fn has_permission(&self, origin: &str, required_mask: u32, now_ms: u64) -> bool {
        if let Some(claim) = self.claims.get(origin) {
            if let Some(exp) = claim.expires_at_ms {
                if now_ms >= exp {
                    return false;
                }
            }
            (claim.permissions_mask & required_mask) == required_mask
        } else {
            false
        }
    }

    pub fn revoke_claim(&mut self, origin: &str) -> bool {
        self.claims.remove(origin).is_some()
    }

    pub fn count(&self) -> usize {
        self.claims.len()
    }
}
