use std::collections::BTreeMap;

pub const DEFAULT_WATCH_TIMEOUT_MS: u64 = 15 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingWatch {
    pub agent_id: String,
    pub expires_at_ms: u64,
    pub is_armed: bool,
}

#[derive(Debug, Clone)]
pub struct ListenerConnectWatcher {
    pending: BTreeMap<String, PendingWatch>,
    watch_timeout_ms: u64,
    suspended: bool,
    disposed: bool,
}

impl Default for ListenerConnectWatcher {
    fn default() -> Self {
        Self::new(DEFAULT_WATCH_TIMEOUT_MS)
    }
}

impl ListenerConnectWatcher {
    pub fn new(watch_timeout_ms: u64) -> Self {
        Self {
            pending: BTreeMap::new(),
            watch_timeout_ms,
            suspended: false,
            disposed: false,
        }
    }

    pub fn watch(
        &mut self,
        agent_id: impl Into<String>,
        platform: impl Into<String>,
        now_ms: u64,
    ) {
        if self.disposed {
            return;
        }
        self.pending.insert(
            platform.into(),
            PendingWatch {
                agent_id: agent_id.into(),
                expires_at_ms: now_ms.saturating_add(self.watch_timeout_ms),
                is_armed: false,
            },
        );
    }

    pub fn suspend(&mut self) {
        if !self.disposed {
            self.suspended = true;
        }
    }

    pub fn resume(&mut self) {
        if !self.disposed {
            self.suspended = false;
        }
    }

    pub fn dispose(&mut self) {
        self.disposed = true;
        self.suspended = true;
        self.pending.clear();
    }

    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    pub fn pending(&self) -> &BTreeMap<String, PendingWatch> {
        &self.pending
    }

    pub fn tick(
        &mut self,
        now_ms: u64,
        mut is_connected: impl FnMut(&str) -> Result<bool, String>,
    ) -> Vec<(String, String)> {
        if self.suspended || self.disposed {
            return Vec::new();
        }

        let platforms = self.pending.keys().cloned().collect::<Vec<_>>();
        let mut connected = Vec::new();
        for platform in platforms {
            let Some(snapshot) = self.pending.get(&platform).cloned() else {
                continue;
            };
            if snapshot.expires_at_ms <= now_ms {
                self.pending.remove(&platform);
                continue;
            }

            let Ok(is_connected_now) = is_connected(&platform) else {
                continue;
            };
            let Some(current) = self.pending.get_mut(&platform) else {
                continue;
            };

            if !is_connected_now {
                current.is_armed = true;
                continue;
            }

            let armed = current.is_armed;
            let agent_id = current.agent_id.clone();
            self.pending.remove(&platform);
            if armed {
                connected.push((agent_id, platform));
            }
        }
        connected
    }
}
