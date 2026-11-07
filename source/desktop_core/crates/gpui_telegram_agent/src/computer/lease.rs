use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

pub use mahayana_host_runtime::runner::computer_use::ComputerUseCoordination;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseState {
    Unassigned,
    Acquiring,
    ActiveControl,
    Preempted,
    Released,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerControlLease {
    pub lease_id: String,
    pub target_device_id: String,
    pub holder_user_id: u64,
    pub granted_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub is_preempted: bool,
    pub generation: u64,
    pub state: LeaseState,
    pub window_index: u32,
}

impl ComputerControlLease {
    pub fn is_valid(&self) -> bool {
        !self.is_preempted && self.state == LeaseState::ActiveControl && Utc::now() < self.expires_at
    }

    pub fn preempt(&mut self) {
        self.is_preempted = true;
        self.state = LeaseState::Preempted;
    }

    pub fn release(&mut self) {
        self.state = LeaseState::Released;
    }
}

pub struct ComputerLeaseBroker {
    coordination: ComputerUseCoordination,
    current_generation: u64,
    current_agent_id: Option<String>,
    active_lease: Option<ComputerControlLease>,
}

impl Default for ComputerLeaseBroker {
    fn default() -> Self {
        Self::new()
    }
}

impl ComputerLeaseBroker {
    pub fn new() -> Self {
        Self {
            coordination: ComputerUseCoordination::new(true),
            current_generation: 0,
            current_agent_id: None,
            active_lease: None,
        }
    }

    pub fn acquire_lease(&mut self, agent_id: &str, device_id: &str) -> Option<ComputerControlLease> {
        let inherited_lease = self.coordination.acquire_control_lease(agent_id)?;
        self.current_generation = self.current_generation.max(inherited_lease.generation());
        self.current_agent_id = Some(agent_id.to_string());
        let lease = ComputerControlLease {
            lease_id: uuid::Uuid::new_v4().to_string(),
            target_device_id: device_id.to_string(),
            holder_user_id: 1,
            granted_at: Utc::now(),
            expires_at: Utc::now() + Duration::minutes(15),
            is_preempted: false,
            generation: self.current_generation,
            state: LeaseState::ActiveControl,
            window_index: inherited_lease.window_index(),
        };
        self.active_lease = Some(lease.clone());
        Some(lease)
    }

    /// Sub-10ms human emergency takeover via Esc / Kill-Switch
    /// Preempts active lease and monotonically bumps generation fence
    pub fn emergency_takeover(&mut self) -> u64 {
        if let Some(lease) = self.active_lease.as_mut() {
            lease.preempt();
        }
        if let Some(agent_id) = self.current_agent_id.take() {
            self.coordination.free_window(&agent_id);
        }
        self.current_generation = self.current_generation.saturating_add(1);
        self.active_lease = None;
        self.current_generation
    }

    pub fn release_lease(&mut self, lease_id: &str) -> bool {
        if let Some(lease) = self.active_lease.as_mut() {
            if lease.lease_id == lease_id {
                lease.release();
                if let Some(agent_id) = self.current_agent_id.take() {
                    self.coordination.free_window(&agent_id);
                }
                self.active_lease = None;
                return true;
            }
        }
        false
    }

    pub fn current_generation(&self) -> u64 {
        self.current_generation
    }

    pub fn active_lease(&self) -> Option<&ComputerControlLease> {
        self.active_lease.as_ref()
    }

    pub fn is_lease_active(&self) -> bool {
        self.active_lease.as_ref().is_some_and(|l| l.is_valid())
    }
}
