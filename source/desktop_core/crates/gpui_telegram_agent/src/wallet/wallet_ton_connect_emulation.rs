//! TON Connect Transaction Emulation & Balance Impact Simulator
//! Upstream: `Telegram/SourceFiles/wallet/wallet_ton_connect_emulation.cpp`, `wallet_ton_connect_emulation.h`
//! Subsystem: `wallet`, Feature ID: `CAP-06`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmulationAction {
    pub destination_address: String,
    pub amount_nanos: u64,
    pub gas_fee_nanos: u64,
    pub payload_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmulationSummary {
    pub total_amount_nanos: u64,
    pub total_fee_nanos: u64,
    pub net_balance_change_nanos: i64,
    pub is_success: bool,
    pub actions: Vec<EmulationAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletTonConnectEmulation {
    pub latest_emulation: Option<EmulationSummary>,
    pub default_gas_estimate_nanos: u64,
}

impl Default for WalletTonConnectEmulation {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletTonConnectEmulation {
    pub fn new() -> Self {
        Self {
            latest_emulation: None,
            default_gas_estimate_nanos: 15_000_000, // 0.015 TON
        }
    }

    pub fn simulate_actions(&mut self, actions: Vec<EmulationAction>, current_balance_nanos: u64) -> EmulationSummary {
        let mut total_amount = 0u64;
        let mut total_fees = 0u64;

        for action in &actions {
            total_amount = total_amount.saturating_add(action.amount_nanos);
            total_fees = total_fees.saturating_add(action.gas_fee_nanos);
        }

        let total_cost = total_amount.saturating_add(total_fees);
        let is_success = total_cost <= current_balance_nanos;
        let net_balance_change = -(total_cost as i64);

        let summary = EmulationSummary {
            total_amount_nanos: total_amount,
            total_fee_nanos: total_fees,
            net_balance_change_nanos: net_balance_change,
            is_success,
            actions,
        };

        self.latest_emulation = Some(summary.clone());
        summary
    }

    pub fn can_afford(&self, current_balance_nanos: u64) -> bool {
        if let Some(ref emu) = self.latest_emulation {
            let total_cost = emu.total_amount_nanos.saturating_add(emu.total_fee_nanos);
            total_cost <= current_balance_nanos
        } else {
            false
        }
    }
}
