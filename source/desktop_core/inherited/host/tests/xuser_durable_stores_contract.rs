use std::fs;
use std::sync::{Arc,atomic::{AtomicU64,Ordering}};
use std::time::{SystemTime,UNIX_EPOCH};
use mahayana_host_runtime::extensions::cross_user_sharing::xuser_turn_dedupe_store::*;
use mahayana_host_runtime::extensions::cross_user_sharing::xuser_room_tombstone_store::*;
use mahayana_host_runtime::extensions::cross_user_sharing::xuser_pending_departure_store::*;

fn dir(label:&str)->std::path::PathBuf{let n=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();std::env::temp_dir().join(format!("fabushi-xuser-{label}-{}-{n}",std::process::id()))}

#[test]
fn turn_dedupe_persists_prunes_and_reloads(){
 let root=dir("dedupe");let now=Arc::new(AtomicU64::new(10));let n2=Arc::clone(&now);
 let s=SandXuserTurnDedupeStore::new(&root,5,Box::new(move||n2.load(Ordering::SeqCst)));
 assert!(s.mark_seen_if_new("n"));assert!(!s.mark_seen_if_new("n"));now.store(16,Ordering::SeqCst);assert!(s.mark_seen_if_new("n"));
 let raw=fs::read_to_string(root.join(SAND_XUSER_TURN_DEDUPE_FILE_NAME)).unwrap();assert_eq!(parse_dedupe_file(Some(&raw)).len(),1);let _=fs::remove_dir_all(root);
}
#[test]
fn tombstones_are_owner_scoped_and_idempotent(){
 let root=dir("tomb");let s=SandXuserRoomTombstoneStore::new(&root);
 let a=XuserRoomTombstone{room_id:"r".into(),owner_auth_id:Some("a".into()),torn_down_at_ms:1};let b=XuserRoomTombstone{room_id:"r".into(),owner_auth_id:Some("b".into()),torn_down_at_ms:2};
 s.record(a.clone());s.record(a.clone());s.record(b.clone());assert_eq!(s.list().len(),2);s.clear("r",Some("a"));assert_eq!(s.list(),vec![b]);let _=fs::remove_dir_all(root);
}
#[test]
fn pending_departures_use_frozen_identity_keys(){
 let root=dir("pending");let s=SandXuserPendingDepartureStore::new(&root);
 let leave=XuserDeparture::LeaveRoom{owner_auth_id:Some("o".into()),room_id:"r".into(),agent_id:"a".into()};
 let remove=XuserDeparture::RemoveAgent{owner_auth_id:Some("o".into()),agent_id:"a".into()};
 s.record(leave.clone());s.record(remove.clone());s.record(remove.clone());assert_eq!(s.list().len(),2);s.clear(&leave);assert_eq!(s.list(),vec![remove]);let _=fs::remove_dir_all(root);
}
