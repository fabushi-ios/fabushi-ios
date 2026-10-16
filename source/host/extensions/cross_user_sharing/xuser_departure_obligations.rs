use std::collections::BTreeSet;
use std::sync::Arc;

use super::xuser_pending_departure_store::{SandXuserPendingDepartureStore, XuserDeparture};
use super::xuser_room_tombstone_store::{SandXuserRoomTombstoneStore, XuserRoomTombstone};

pub const ROOM_TOMBSTONE_MIN_RETENTION_MS: u64 = 48 * 60 * 60_000;

pub trait XuserDepartureRelay: Send + Sync {
    fn leave_room(&self, room_id: &str) -> Result<(), String>;
    fn remove_deleted_agent(&self, agent_id: &str) -> Result<(), String>;
}

#[derive(Clone)]
pub struct SandXuserDepartureObligations {
    pub pending: Arc<SandXuserPendingDepartureStore>,
    pub tombstones: Arc<SandXuserRoomTombstoneStore>,
}

impl SandXuserDepartureObligations {
    pub fn new(
        pending: Arc<SandXuserPendingDepartureStore>,
        tombstones: Arc<SandXuserRoomTombstoneStore>,
    ) -> Self {
        Self { pending, tombstones }
    }

    pub fn record_leave(
        &self,
        owner_auth_id: Option<String>,
        room_id: String,
        now_ms: u64,
    ) -> Result<XuserDeparture, String> {
        let departure = XuserDeparture::LeaveRoom {
            owner_auth_id: owner_auth_id.clone(),
            room_id: room_id.clone(),
        };
        self.pending.record(departure.clone())?;
        if let Err(error) = self.tombstones.record(XuserRoomTombstone {
            room_id,
            owner_auth_id,
            torn_down_at_ms: now_ms,
        }) {
            let _ = self.pending.clear(&departure);
            return Err(error);
        }
        Ok(departure)
    }

    pub fn record_agent_removal(
        &self,
        owner_auth_id: Option<String>,
        agent_id: String,
    ) -> Result<XuserDeparture, String> {
        let departure = XuserDeparture::RemoveAgent { owner_auth_id, agent_id };
        self.pending.record(departure.clone())?;
        Ok(departure)
    }

    pub fn drain(&self, relay: &dyn XuserDepartureRelay) -> Result<usize, String> {
        let mut completed = 0;
        for departure in self.pending.list()? {
            let result = match &departure {
                XuserDeparture::LeaveRoom { room_id, .. } => relay.leave_room(room_id),
                XuserDeparture::RemoveAgent { agent_id, .. } => relay.remove_deleted_agent(agent_id),
            };
            if result.is_ok() {
                self.pending.clear(&departure)?;
                completed += 1;
            }
        }
        Ok(completed)
    }

    pub fn is_room_abandoned(
        &self,
        room_id: &str,
        self_auth_id: Option<&str>,
    ) -> Result<bool, String> {
        if self.tombstones.list()?.iter().any(|value| {
            value.room_id == room_id
                && (value.owner_auth_id.is_none()
                    || value.owner_auth_id.as_deref() == self_auth_id)
        }) {
            return Ok(true);
        }
        Ok(self.pending.list()?.iter().any(|value| {
            matches!(
                value,
                XuserDeparture::LeaveRoom { owner_auth_id, room_id: pending_room }
                    if pending_room == room_id
                        && (owner_auth_id.is_none() || owner_auth_id.as_deref() == self_auth_id)
            )
        }))
    }

    pub fn prune_tombstones_for_registry(
        &self,
        now_ms: u64,
        registry_room_ids: &BTreeSet<String>,
        self_auth_id: Option<&str>,
    ) -> Result<(), String> {
        for value in self.tombstones.list()? {
            if (value.owner_auth_id.is_none() || value.owner_auth_id.as_deref() == self_auth_id)
                && !registry_room_ids.contains(&value.room_id)
                && now_ms.saturating_sub(value.torn_down_at_ms) >= ROOM_TOMBSTONE_MIN_RETENTION_MS
            {
                self.tombstones.clear(&value.room_id, value.owner_auth_id.as_deref())?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    struct Relay { leaves: Mutex<Vec<String>>, fail: bool }
    impl XuserDepartureRelay for Relay {
        fn leave_room(&self, room_id: &str) -> Result<(), String> {
            self.leaves.lock().unwrap().push(room_id.to_string());
            if self.fail { Err("offline".into()) } else { Ok(()) }
        }
        fn remove_deleted_agent(&self, _: &str) -> Result<(), String> {
            if self.fail { Err("offline".into()) } else { Ok(()) }
        }
    }

    #[test]
    fn failed_departure_remains_durable_until_retry_succeeds() {
        let root = std::env::temp_dir().join(format!(
            "fabushi-xuser-obligations-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let obligations = SandXuserDepartureObligations::new(
            Arc::new(SandXuserPendingDepartureStore::new(&root)),
            Arc::new(SandXuserRoomTombstoneStore::new(&root)),
        );
        obligations.record_leave(Some("a".into()), "room".into(), 1).unwrap();
        let offline = Relay { leaves: Mutex::new(Vec::new()), fail: true };
        assert_eq!(obligations.drain(&offline).unwrap(), 0);
        assert_eq!(obligations.pending.list().unwrap().len(), 1);
        let online = Relay { leaves: Mutex::new(Vec::new()), fail: false };
        assert_eq!(obligations.drain(&online).unwrap(), 1);
        assert!(obligations.pending.list().unwrap().is_empty());
        assert!(obligations.is_room_abandoned("room", Some("a")).unwrap());
        let _ = fs::remove_dir_all(root);
    }
}
