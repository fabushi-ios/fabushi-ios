use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomMember {
    pub auth_id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default, alias = "displayName")]
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
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileResult {
    pub rooms: Vec<XuserRoom>,
    pub revoked_room_ids: Vec<String>,
    pub added_room_ids: Vec<String>,
}

pub fn reconcile_rooms(previous: &[XuserRoom], remote: Vec<XuserRoom>, self_auth_id: Option<&str>) -> ReconcileResult {
    let old = previous.iter().cloned().map(|room| (room.room_id.clone(), room)).collect::<BTreeMap<_, _>>();
    let new = remote.into_iter().filter(|room| !room.room_id.is_empty()).map(|room| (room.room_id.clone(), room)).collect::<BTreeMap<_, _>>();
    let old_ids = old.keys().cloned().collect::<BTreeSet<_>>();
    let new_ids = new.keys().cloned().collect::<BTreeSet<_>>();
    let mut rooms = new.into_values().filter(|room| {
        self_auth_id.is_none_or(|id| {
            room.host_auth_id == id || room.members.is_empty() || room.members.iter().any(|member| member.auth_id == id)
        })
    }).collect::<Vec<_>>();
    rooms.sort_by(|left, right| left.room_id.cmp(&right.room_id));
    ReconcileResult {
        rooms,
        revoked_room_ids: old_ids.difference(&new_ids).cloned().collect(),
        added_room_ids: new_ids.difference(&old_ids).cloned().collect(),
    }
}
