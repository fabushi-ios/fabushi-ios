use std::sync::{Arc,Mutex};
use mahayana_host_runtime::mcp_auth::host_mcp_auth_completion::*;
use mahayana_host_runtime::mcp_auth::mcp_auth_wait_registry::McpAuthWaitEvent;

#[derive(Default)]
struct Mcp{watch:Mutex<Option<String>>,notes:Mutex<Vec<(String,String)>>,restarts:Mutex<usize>}
impl HostMcpAuthMcp for Mcp{
 fn note_auth_completed_elsewhere(&self,s:&str,a:&str)->Option<String>{
  self.notes.lock().unwrap().push((s.into(),a.into()));
  self.watch.lock().unwrap().clone()
 }
 fn restart(&self)->Result<(),String>{*self.restarts.lock().unwrap()+=1;Ok(())}
}
#[derive(Default)]
struct Transcript{resumes:Mutex<Vec<(String,String,String)>>}
impl HostMcpAuthTranscript for Transcript{
 fn resume_after_mcp_auth(&self,a:&str,s:&str,k:&str)->Result<(),String>{
  self.resumes.lock().unwrap().push((a.into(),s.into(),k.into()));Ok(())
 }
}
fn event(outcome:&str,requesting:Option<&str>)->HostMcpAuthCompletionEvent{
 HostMcpAuthCompletionEvent{server_id:"srv".into(),server_name:"GitHub".into(),account_key:"acct".into(),outcome:outcome.into(),requesting_agent_id:requesting.map(str::to_string)}
}
#[test]
fn requesting_then_watch_then_waiting_agent_precedence_matches_frozen_host(){
 let mcp=Arc::new(Mcp::default()); *mcp.watch.lock().unwrap()=Some("watch".into());
 let transcript=Arc::new(Transcript::default());
 let completion=HostMcpAuthCompletion::new(mcp.clone(),transcript.clone(),None);
 completion.register_connect_card(McpAuthWaitEvent{agent_id:"waiting".into(),connector:"GitHub".into(),server_id:Some("srv".into())});
 completion.resolve(&event("ok",Some("requesting")));
 assert_eq!(transcript.resumes.lock().unwrap()[0].0,"requesting");
 assert_eq!(mcp.notes.lock().unwrap().len(),1);
}
#[test]
fn cancelled_consumes_wait_and_notes_without_resume_and_desktop_restarts_first(){
 let mcp=Arc::new(Mcp::default()); let transcript=Arc::new(Transcript::default());
 let completion=HostMcpAuthCompletion::new(mcp.clone(),transcript.clone(),None);
 completion.register_connect_card(McpAuthWaitEvent{agent_id:"waiting".into(),connector:"GitHub".into(),server_id:Some("srv".into())});
 completion.resolve(&event("cancelled",None));
 assert!(transcript.resumes.lock().unwrap().is_empty());
 assert_eq!(mcp.notes.lock().unwrap().len(),1);
 completion.resolve_desktop(&event("ok",None));
 assert_eq!(*mcp.restarts.lock().unwrap(),1);
}
