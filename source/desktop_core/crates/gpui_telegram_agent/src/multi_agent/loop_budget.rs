use serde::{Deserialize, Serialize};

pub const DEFAULT_MAX_LOOP_HOPS: u32 = 5;

#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
#[error("Loop budget exceeded: reached {current} hops (max {max}) with trace {trace:?}")]
pub struct LoopBudgetExceededError {
    pub current: u32,
    pub max: u32,
    pub trace: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopBudget {
    max_hops: u32,
    current_hop: u32,
    causal_trace: Vec<String>,
}

impl Default for LoopBudget {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_LOOP_HOPS)
    }
}

impl LoopBudget {
    pub fn new(max_hops: u32) -> Self {
        Self {
            max_hops,
            current_hop: 0,
            causal_trace: Vec::new(),
        }
    }

    pub fn record_hop(&mut self, agent_id: &str) -> Result<u32, LoopBudgetExceededError> {
        if self.current_hop >= self.max_hops {
            return Err(LoopBudgetExceededError {
                current: self.current_hop + 1,
                max: self.max_hops,
                trace: self.causal_trace.clone(),
            });
        }
        self.current_hop += 1;
        self.causal_trace.push(agent_id.to_string());
        Ok(self.current_hop)
    }

    pub fn is_exhausted(&self) -> bool {
        self.current_hop >= self.max_hops
    }

    pub fn current_hop(&self) -> u32 {
        self.current_hop
    }

    pub fn max_hops(&self) -> u32 {
        self.max_hops
    }

    pub fn root_agent_id(&self) -> Option<&str> {
        self.causal_trace.first().map(String::as_str)
    }

    pub fn trace(&self) -> &[String] {
        &self.causal_trace
    }

    pub fn reset(&mut self) {
        self.current_hop = 0;
        self.causal_trace.clear();
    }
}
