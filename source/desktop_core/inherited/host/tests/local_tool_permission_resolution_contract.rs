use std::collections::{HashMap,HashSet};
use std::sync::{Arc,Mutex,atomic::{AtomicUsize,Ordering}};
use mahayana_host_runtime::extensions::local_tool_permission::local_tool_permission_resolution::*;

#[derive(Default)]
struct Asks{
 pending:Mutex<HashMap<String,PendingLocalToolPermissionRequest>>,
 settled:Mutex<HashSet<String>>,
 resolved:Mutex<Vec<(String,SandLocalToolResolution)>>,
}
impl LocalToolPermissionAskStore for Asks{
 fn get_pending_request_by_id(&self,id:&str)->Option<PendingLocalToolPermissionRequest>{self.pending.lock().unwrap().get(id).cloned()}
 fn was_settled(&self,id:&str)->bool{self.settled.lock().unwrap().contains(id)}
 fn resolve_request(&self,id:&str,r:SandLocalToolResolution)->bool{
  if self.pending.lock().unwrap().remove(id).is_none(){return false}
  self.resolved.lock().unwrap().push((id.into(),r)); true
 }
}
struct Widgets(StaleLocalToolPermissionCardSettlement);
impl LocalToolPermissionWidgetResponses for Widgets{
 fn settle_stale_local_tool_permission_card(&self,_:&str,_:&str,_:&str)->Result<StaleLocalToolPermissionCardSettlement,String>{Ok(self.0)}
}
fn args(value:&str)->LocalToolPermissionResolutionArgs{
 LocalToolPermissionResolutionArgs{agent_id:"agent".into(),entry_id:"entry".into(),request_id:"req".into(),resolution:value.into()}
}
#[test]
fn resolves_pending_and_is_idempotent_after_settlement(){
 let asks=Asks::default();
 asks.pending.lock().unwrap().insert("req".into(),PendingLocalToolPermissionRequest{agent_id:"agent".into()});
 resolve_local_tool_permission_ask(&asks,&Widgets(StaleLocalToolPermissionCardSettlement::NotSettled),&args("always"),None).unwrap();
 assert_eq!(asks.resolved.lock().unwrap().as_slice(),&[("req".into(),SandLocalToolResolution::Always)]);
 asks.settled.lock().unwrap().insert("req".into());
 resolve_local_tool_permission_ask(&asks,&Widgets(StaleLocalToolPermissionCardSettlement::NotSettled),&args("always"),None).unwrap();
}
#[test]
fn stale_retired_and_invalid_paths_fail_closed(){
 let asks=Asks::default();
 let retired=Arc::new(AtomicUsize::new(0)); let r2=Arc::clone(&retired);
 resolve_local_tool_permission_ask(&asks,&Widgets(StaleLocalToolPermissionCardSettlement::Retired),&args("deny"),Some(&move||{r2.fetch_add(1,Ordering::SeqCst);})).unwrap();
 assert_eq!(retired.load(Ordering::SeqCst),1);
 assert_eq!(
  resolve_local_tool_permission_ask(&asks,&Widgets(StaleLocalToolPermissionCardSettlement::NotSettled),&args("deny"),None).unwrap_err(),
  SandLocalToolPermissionResolutionError::Stale
 );
 assert_eq!(
  resolve_local_tool_permission_ask(&asks,&Widgets(StaleLocalToolPermissionCardSettlement::Settled),&args("bogus"),None).unwrap_err(),
  SandLocalToolPermissionResolutionError::UnknownResolution
 );
}
