//! Authentic TON Staking Screen ViewModel
//! Part of Module 13 Wallet Part 4 Client Architecture

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StakingPool {
    pub name: String,
    pub address: String,
    pub apy_percentage: f64,
    pub min_deposit_nanotons: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonStakingScreen {
    pub active_pool: Option<StakingPool>,
    pub available_pools: Vec<StakingPool>,
    pub staked_balance_nanotons: u64,
    pub pending_reward_nanotons: u64,
    pub cycle_end_timestamp: u64,
    pub is_withdrawing: bool,
}

impl TonStakingScreen {
    pub fn new() -> Self {
        let default_pools = vec![
            StakingPool {
                name: "TON Whales Pool".into(),
                address: "EQCkR1cGMNsE45N4K0oh853fZC7Kni9mRgK7yU8Y7L1pool".into(),
                apy_percentage: 4.85,
                min_deposit_nanotons: 50_000_000_000, // 50 TON
            },
            StakingPool {
                name: "Tonstakers Liquid Staking".into(),
                address: "EQD2_4d989f_tonstakers_liquid_pool_contract_addr".into(),
                apy_percentage: 5.12,
                min_deposit_nanotons: 1_000_000_000, // 1 TON
            },
        ];

        let active = default_pools.first().cloned();
        Self {
            active_pool: active,
            available_pools: default_pools,
            staked_balance_nanotons: 0,
            pending_reward_nanotons: 0,
            cycle_end_timestamp: 0,
            is_withdrawing: false,
        }
    }

    pub fn select_pool(&mut self, pool_name: &str) {
        if let Some(pool) = self.available_pools.iter().find(|p| p.name == pool_name) {
            self.active_pool = Some(pool.clone());
        }
    }

    pub fn update_stake(&mut self, staked: u64, reward: u64, cycle_end: u64) {
        self.staked_balance_nanotons = staked;
        self.pending_reward_nanotons = reward;
        self.cycle_end_timestamp = cycle_end;
    }
}

impl Default for TonStakingScreen {
    fn default() -> Self {
        Self::new()
    }
}
