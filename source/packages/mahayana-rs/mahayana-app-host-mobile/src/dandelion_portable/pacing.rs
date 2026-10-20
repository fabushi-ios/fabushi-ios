//! Bounded control-plane pacing derived from Dandelion's shared wire-budget rule.

use super::core::{MAX_CONTROL_BYTES, TrafficBudget};

#[derive(Debug)]
pub struct ControlPacer {
    budget: TrafficBudget,
    max_frame_bytes: u64,
}

impl ControlPacer {
    pub fn new(total_bytes: u64, max_frame_bytes: u64) -> Result<Self, String> {
        if max_frame_bytes == 0 || max_frame_bytes > MAX_CONTROL_BYTES as u64 {
            return Err("Dandelion control frame bound is invalid".into());
        }
        Ok(Self {
            budget: TrafficBudget::new(total_bytes)?,
            max_frame_bytes,
        })
    }

    pub fn reserve_frame(&mut self, bytes: usize) -> Result<(), String> {
        let bytes = u64::try_from(bytes)
            .map_err(|_| "Dandelion control frame length is not representable".to_string())?;
        if bytes == 0 || bytes > self.max_frame_bytes {
            return Err("Dandelion control frame exceeds the per-frame bound".into());
        }
        self.budget.reserve(bytes)
    }

    pub fn reserved(&self) -> u64 {
        self.budget.reserved()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacing_reserves_before_platform_handoff() {
        let mut pacer = ControlPacer::new(12, 8).unwrap();
        pacer.reserve_frame(8).unwrap();
        assert!(pacer.reserve_frame(5).is_err());
        assert_eq!(pacer.reserved(), 8);
        assert!(ControlPacer::new(10, 0).is_err());
    }
}
