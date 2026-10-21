//! Portable accounting and cancellation result values adapted from dandelion-core.

use serde::{Deserialize, Serialize};

pub const MAX_CONTROL_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    #[default]
    Completed,
    Cancelled,
    BudgetExhausted,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub schema_version: u32,
    pub status: RunStatus,
    pub control_bytes_reserved: u64,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            schema_version: 1,
            status: RunStatus::Completed,
            control_bytes_reserved: 0,
        }
    }
}

/// Single-owner accounting. Reservations are never rolled back after they are
/// admitted because an ambiguous platform settlement must remain charged.
#[derive(Debug)]
pub struct TrafficBudget {
    limit: u64,
    reserved: u64,
}

impl TrafficBudget {
    pub fn new(limit: u64) -> Result<Self, String> {
        if limit == 0 {
            return Err("Dandelion traffic limit must be positive".into());
        }
        Ok(Self { limit, reserved: 0 })
    }

    pub fn reserve(&mut self, bytes: u64) -> Result<(), String> {
        let next = self
            .reserved
            .checked_add(bytes)
            .ok_or_else(|| "Dandelion traffic budget overflow".to_string())?;
        if next > self.limit {
            return Err("Dandelion traffic budget exhausted".into());
        }
        self.reserved = next;
        Ok(())
    }

    pub fn reserved(&self) -> u64 {
        self.reserved
    }
}

pub fn lowercase_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_budget_and_overflow_fail_closed() {
        let mut budget = TrafficBudget::new(100).unwrap();
        budget.reserve(100).unwrap();
        assert!(budget.reserve(1).is_err());
        assert_eq!(budget.reserved(), 100);

        let mut budget = TrafficBudget::new(u64::MAX).unwrap();
        budget.reserve(u64::MAX).unwrap();
        assert!(budget.reserve(1).is_err());
        assert!(TrafficBudget::new(0).is_err());
    }

    #[test]
    fn package_digest_is_lowercase_and_fixed_width() {
        assert!(lowercase_hex_digest(&"a".repeat(64)));
        assert!(!lowercase_hex_digest(&"A".repeat(64)));
        assert!(!lowercase_hex_digest(&"a".repeat(63)));
    }
}
