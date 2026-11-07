use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomMember {
    pub auth_id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XuserRoom {
    pub room_id: String,
    pub name: String,
    pub host_auth_id: String,
    #[serde(default)]
    pub members: Vec<RoomMember>,
    #[serde(default)]
    pub avatar_data_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconcileResult {
    pub rooms: Vec<XuserRoom>,
    pub revoked_room_ids: Vec<String>,
    pub added_room_ids: Vec<String>,
}

pub fn reconcile_rooms(
    previous: &[XuserRoom],
    remote: Vec<XuserRoom>,
    self_auth_id: Option<&str>,
) -> ReconcileResult {
    let old: BTreeMap<_, _> = previous
        .iter()
        .map(|r| (r.room_id.clone(), r.clone()))
        .collect();
    let new: BTreeMap<_, _> = remote
        .into_iter()
        .filter(|r| !r.room_id.is_empty())
        .map(|r| (r.room_id.clone(), r))
        .collect();
    let old_ids: BTreeSet<_> = old.keys().cloned().collect();
    let new_ids: BTreeSet<_> = new.keys().cloned().collect();
    let mut revoked = old_ids.difference(&new_ids).cloned().collect::<Vec<_>>();
    let mut added = new_ids.difference(&old_ids).cloned().collect::<Vec<_>>();
    revoked.sort();
    added.sort();
    let mut rooms = new
        .into_values()
        .filter(|r| {
            self_auth_id.is_none_or(|id| {
                r.members.is_empty()
                    || r.members.iter().any(|m| m.auth_id == id)
                    || r.host_auth_id == id
            })
        })
        .collect::<Vec<_>>();
    rooms.sort_by(|a, b| a.room_id.cmp(&b.room_id));
    ReconcileResult {
        rooms,
        revoked_room_ids: revoked,
        added_room_ids: added,
    }
}
