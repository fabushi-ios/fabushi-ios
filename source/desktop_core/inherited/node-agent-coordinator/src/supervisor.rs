use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::{
    Failure, COORDINATOR_DISCONNECTED, COORDINATOR_HOST_RESTARTED,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostGeneration(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GatewayState {
    Unknown,
    Down,
    Up,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingRequest {
    pub request_id: String,
    pub method: String,
    pub host_generation: HostGeneration,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestSettlement {
    pub request_id: String,
    pub failure: Failure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinatorCrash {
    pub generation: u64,
    pub component: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResyncSnapshot {
    pub coordinator_generation: u64,
    pub host_generation: HostGeneration,
    pub gateway: GatewayState,
    pub pending_request_ids: Vec<String>,
    pub last_crash: Option<CoordinatorCrash>,
}

#[derive(Debug)]
pub struct CoordinatorSupervisor {
    connected: bool,
    coordinator_generation: u64,
    host_generation: HostGeneration,
    gateway: GatewayState,
    pending: HashMap<String, PendingRequest>,
    last_crash: Option<CoordinatorCrash>,
}

impl Default for CoordinatorSupervisor {
    fn default() -> Self {
        Self {
            connected: false,
            coordinator_generation: 0,
            host_generation: HostGeneration(0),
            gateway: GatewayState::Unknown,
            pending: HashMap::new(),
            last_crash: None,
        }
    }
}

impl CoordinatorSupervisor {
    pub fn connect(&mut self) -> ResyncSnapshot {
        self.connected = true;
        self.coordinator_generation = self.coordinator_generation.saturating_add(1);
        self.snapshot()
    }

    pub fn register_request(
        &mut self,
        request_id: impl Into<String>,
        method: impl Into<String>,
        metadata: Value,
    ) -> Result<(), Failure> {
        if !self.connected {
            return Err(Failure::new(COORDINATOR_DISCONNECTED, "coordinator is disconnected"));
        }
        let request_id = request_id.into();
        if request_id.is_empty() || self.pending.contains_key(&request_id) {
            return Err(Failure::new("COORDINATOR_DUPLICATE_REQUEST", format!("request id {request_id:?} is invalid or already pending")));
        }
        self.pending.insert(request_id.clone(), PendingRequest {
            request_id,
            method: method.into(),
            host_generation: self.host_generation,
            metadata,
        });
        Ok(())
    }

    pub fn complete_request(&mut self, request_id: &str) -> Option<PendingRequest> {
        self.pending.remove(request_id)
    }

    pub fn disconnect(&mut self, detail: impl Into<String>) -> Vec<RequestSettlement> {
        self.connected = false;
        let detail = detail.into();
        self.pending.drain().map(|(_, pending)| RequestSettlement {
            request_id: pending.request_id,
            failure: Failure::new(COORDINATOR_DISCONNECTED, detail.clone()),
        }).collect()
    }

    pub fn reconnect(&mut self, observed_host_generation: HostGeneration) -> ResyncSnapshot {
        self.connected = true;
        self.coordinator_generation = self.coordinator_generation.saturating_add(1);
        if observed_host_generation != self.host_generation {
            self.host_generation = observed_host_generation;
            self.pending.clear();
        }
        self.snapshot()
    }

    pub fn host_restarted(&mut self, new_generation: HostGeneration) -> Vec<RequestSettlement> {
        if new_generation == self.host_generation {
            return Vec::new();
        }
        self.host_generation = new_generation;
        self.pending.drain().map(|(_, pending)| RequestSettlement {
            request_id: pending.request_id,
            failure: Failure::new(
                COORDINATOR_HOST_RESTARTED,
                format!("host restarted at generation {}", new_generation.0),
            ),
        }).collect()
    }

    pub fn set_gateway_state(&mut self, state: GatewayState) {
        self.gateway = state;
    }

    pub fn record_crash(&mut self, component: impl Into<String>, detail: impl Into<String>) -> CoordinatorCrash {
        let crash = CoordinatorCrash {
            generation: self.coordinator_generation,
            component: component.into(),
            detail: detail.into(),
        };
        self.last_crash = Some(crash.clone());
        crash
    }

    pub fn snapshot(&self) -> ResyncSnapshot {
        let mut pending_request_ids = self.pending.keys().cloned().collect::<Vec<_>>();
        pending_request_ids.sort();
        ResyncSnapshot {
            coordinator_generation: self.coordinator_generation,
            host_generation: self.host_generation,
            gateway: self.gateway,
            pending_request_ids,
            last_crash: self.last_crash.clone(),
        }
    }
}
