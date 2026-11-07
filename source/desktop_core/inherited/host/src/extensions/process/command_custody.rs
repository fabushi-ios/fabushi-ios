use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandCapacityGroupConfig {
    pub name: String,
    pub max_concurrency: usize,
}

#[derive(Clone, Default)]
pub struct CommandProcessCustody {
    groups: Arc<Mutex<HashMap<String, usize>>>, // group -> current active
    capacities: Arc<Mutex<HashMap<String, usize>>>, // group -> limit
    cancellations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

impl CommandProcessCustody {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_group_capacity(&self, group: &str, limit: usize) {
        let mut caps = self.capacities.lock().unwrap();
        caps.insert(group.to_string(), limit);
    }

    pub fn acquire_slot(&self, group: &str) -> Result<SlotGuard, String> {
        let mut groups = self.groups.lock().unwrap();
        let caps = self.capacities.lock().unwrap();

        let limit = caps.get(group).copied().unwrap_or(16);
        let current = groups.entry(group.to_string()).or_insert(0);

        if *current >= limit {
            return Err(format!(
                "Command capacity group '{group}' exhausted: {current}/{limit} active"
            ));
        }

        *current += 1;
        Ok(SlotGuard {
            group: group.to_string(),
            groups: Arc::clone(&self.groups),
        })
    }

    pub fn create_cancellation_token(&self, command_id: &str) -> Arc<AtomicBool> {
        let token = Arc::new(AtomicBool::new(false));
        let mut cancellations = self.cancellations.lock().unwrap();
        cancellations.insert(command_id.to_string(), Arc::clone(&token));
        token
    }

    pub fn cancel_command(&self, command_id: &str) -> bool {
        let cancellations = self.cancellations.lock().unwrap();
        if let Some(token) = cancellations.get(command_id) {
            token.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub fn is_cancelled(&self, command_id: &str) -> bool {
        let cancellations = self.cancellations.lock().unwrap();
        cancellations
            .get(command_id)
            .map(|t| t.load(Ordering::SeqCst))
            .unwrap_or(false)
    }

    pub fn enforce_deadline(&self, start: Instant, timeout: Duration) -> Result<(), String> {
        if start.elapsed() > timeout {
            Err(format!("Command execution timed out after {}ms", timeout.as_millis()))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug)]
pub struct SlotGuard {
    group: String,
    groups: Arc<Mutex<HashMap<String, usize>>>,
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        let mut groups = self.groups.lock().unwrap();
        if let Some(current) = groups.get_mut(&self.group) {
            if *current > 0 {
                *current -= 1;
            }
        }
    }
}
