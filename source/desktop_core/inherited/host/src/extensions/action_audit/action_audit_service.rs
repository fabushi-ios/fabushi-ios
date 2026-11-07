use std::collections::{BTreeMap,HashSet};
use std::fs::{self,OpenOptions};
use std::io::Write;
use std::path::{Path,PathBuf};
use std::sync::{Arc,Condvar,Mutex};
use std::thread::{self,JoinHandle};
use std::time::{Duration,SystemTime,UNIX_EPOCH};
use serde::{Serialize,Deserialize};
use uuid::Uuid;

pub const ACTION_AUDIT_FLUSH_INTERVAL_MS:u64=5_000;
pub const MAX_AUDIT_BATCH_SIZE:usize=50;
pub const MAX_PENDING_AUDIT_EVENTS:usize=2_000;
pub const AUDIT_FLUSH_FAILURE_BACKOFF_MS:u64=30_000;

#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(tag="kind",rename_all="camelCase")]
pub enum AuditAction{
 McpToolCall{#[serde(rename="toolCallId")]tool_call_id:String,#[serde(rename="serverIdentifier")]server_identifier:String,#[serde(rename="serverName",skip_serializing_if="Option::is_none")]server_name:Option<String>,#[serde(rename="toolName")]tool_name:String,transport:String,status:String,#[serde(rename="durationMs")]duration_ms:f64},
 ShellCommand{command:String,#[serde(rename="shellKind")]shell_kind:String,target:String,allowed:Option<bool>,#[serde(rename="blockedReason",skip_serializing_if="Option::is_none")]blocked_reason:Option<String>,#[serde(rename="classificationReasons",default)]classification_reasons:Vec<String>},
 BrowserNavigation{url:String,#[serde(rename="pageTitle")]page_title:String},
 ComputerUseSession{#[serde(rename="toolCallId",skip_serializing_if="Option::is_none")]tool_call_id:Option<String>,#[serde(rename="actionCount")]action_count:u64,#[serde(rename="actionCounts")]action_counts:BTreeMap<String,u64>,#[serde(rename="durationMs")]duration_ms:f64,#[serde(rename="screenshotCount")]screenshot_count:u64},
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct AuditRecord{pub occurred_at_ms:u64,pub agent_id:String,pub turn_id:Option<String>,pub box_id:Option<String>,pub action:AuditAction}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct AuditEvent{pub event_id:String,pub occurred_at_ms:u64,pub agent_id:String,pub turn_id:String,pub box_id:String,pub action:AuditAction}

pub fn is_backend_forwardable(record:&AuditRecord)->bool{
 match &record.action{AuditAction::McpToolCall{transport,..}=>transport=="stdio",_=>true}
}
fn iso_ms(ms:u64)->String{
 let millis=i64::try_from(ms).unwrap_or(i64::MAX);
 chrono::DateTime::<chrono::Utc>::from_timestamp_millis(millis)
  .map(|value|value.to_rfc3339_opts(chrono::SecondsFormat::Millis,true))
  .unwrap_or_else(||"1970-01-01T00:00:00.000Z".into())
}
pub fn local_audit_jsonl_line(record:&AuditRecord,event_id:&str)->String{
 let mut base=serde_json::Map::new();
 base.insert("ts".into(),serde_json::Value::String(iso_ms(record.occurred_at_ms)));
 base.insert("agentId".into(),serde_json::Value::String(record.agent_id.clone()));
 base.insert("eventId".into(),serde_json::Value::String(event_id.into()));
 if let Some(turn)=record.turn_id.as_ref().filter(|v|!v.is_empty()){base.insert("turnId".into(),serde_json::Value::String(turn.clone()));}
 match &record.action{
  AuditAction::McpToolCall{tool_call_id,server_identifier,tool_name,transport,status,duration_ms,..}=>{
   base.insert("type".into(),"mcp_tool_call".into());base.insert("serverIdentifier".into(),server_identifier.clone().into());base.insert("toolName".into(),tool_name.clone().into());base.insert("toolCallId".into(),tool_call_id.clone().into());base.insert("transport".into(),transport.clone().into());base.insert("status".into(),status.clone().into());base.insert("durationMs".into(),serde_json::json!(duration_ms));
  }
  AuditAction::BrowserNavigation{url,page_title}=>{base.insert("type".into(),"browser_navigation".into());base.insert("url".into(),url.clone().into());base.insert("pageTitle".into(),page_title.clone().into());}
  AuditAction::ComputerUseSession{tool_call_id,action_count,action_counts,duration_ms,screenshot_count}=>{base.insert("type".into(),"computer_use_session".into());base.insert("toolCallId".into(),tool_call_id.clone().unwrap_or_default().into());base.insert("actionCount".into(),(*action_count).into());base.insert("actionCounts".into(),serde_json::to_value(action_counts).unwrap_or_default());base.insert("durationMs".into(),serde_json::json!(duration_ms));base.insert("screenshotCount".into(),(*screenshot_count).into());}
  AuditAction::ShellCommand{command,shell_kind,target,..}=>{base.insert("type".into(),"shell_command".into());base.insert("command".into(),command.clone().into());base.insert("shellKind".into(),shell_kind.clone().into());base.insert("target".into(),target.clone().into());}
 }
 format!("{}\n",serde_json::Value::Object(base))
}

type Enabled=Arc<dyn Fn()->bool+Send+Sync>;
#[derive(Debug,Clone,PartialEq,Eq)]
pub struct AuditSendError{pub message:String,pub retry_after_ms:Option<u64>}
impl AuditSendError{pub fn new(message:impl Into<String>)->Self{Self{message:message.into(),retry_after_ms:None}}}
pub fn flush_failure_backoff_ms(error:&AuditSendError)->u64{error.retry_after_ms.unwrap_or(0).max(AUDIT_FLUSH_FAILURE_BACKOFF_MS)}
pub type SendBatch=Arc<dyn Fn(&[AuditEvent])->Result<(),AuditSendError>+Send+Sync>;
type Now=Arc<dyn Fn()->u64+Send+Sync>;
type Id=Arc<dyn Fn()->String+Send+Sync>;
type AuditPath=Arc<dyn Fn(&str)->PathBuf+Send+Sync>;
struct State{pending:Vec<AuditEvent>,loaded:bool,backoff_until_ms:u64,accepting:bool,stopped:bool}
pub struct SandActionAuditor{
 state:Arc<(Mutex<State>,Condvar)>,outbox_path:PathBuf,audit_path:AuditPath,enabled:Enabled,send_batch:SendBatch,now:Now,random_id:Id,worker:Mutex<Option<JoinHandle<()>>>
}
impl SandActionAuditor{
 pub fn new(outbox_path:PathBuf,audit_path:AuditPath,enabled:Enabled,send_batch:SendBatch,flush_interval:Duration)->Self{
  let state=Arc::new((Mutex::new(State{pending:Vec::new(),loaded:false,backoff_until_ms:0,accepting:true,stopped:false}),Condvar::new()));
  let now:Now=Arc::new(||SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().try_into().unwrap_or(u64::MAX));
  let random_id:Id=Arc::new(||Uuid::new_v4().to_string());
  let s2=Arc::clone(&state);let o2=outbox_path.clone();let e2=Arc::clone(&enabled);let b2=Arc::clone(&send_batch);let n2=Arc::clone(&now);
  let worker=thread::spawn(move||{
   let (lock,cv)=&*s2;let mut first=true;
   loop{
    let st=lock.lock().unwrap_or_else(|p|p.into_inner());let (mut st,_)=cv.wait_timeout(st,flush_interval).unwrap_or_else(|p|p.into_inner());
    if st.stopped{break}
    if first{first=false;continue}
    flush_locked(&mut st,&o2,&*e2,&*b2,&*n2);
   }
  });
  Self{state,outbox_path,audit_path,enabled,send_batch,now,random_id,worker:Mutex::new(Some(worker))}
 }
 pub fn with_clock_and_id(mut self,now:Now,random_id:Id)->Self{self.now=now;self.random_id=random_id;self}
 pub fn record(&self,record:AuditRecord){
  let event_id=(self.random_id)();
  let path=(self.audit_path)(&record.agent_id);if let Some(parent)=path.parent(){let _=fs::create_dir_all(parent);};if let Ok(mut f)=OpenOptions::new().create(true).append(true).open(path){let _=f.write_all(local_audit_jsonl_line(&record,&event_id).as_bytes());}
  if !is_backend_forwardable(&record){return}
  let (lock,_)=&*self.state;let mut st=lock.lock().unwrap_or_else(|p|p.into_inner());if !st.accepting{return}
  st.pending.push(AuditEvent{event_id,occurred_at_ms:record.occurred_at_ms,agent_id:record.agent_id,turn_id:record.turn_id.unwrap_or_default(),box_id:record.box_id.unwrap_or_default(),action:record.action});
  if st.pending.len()>MAX_PENDING_AUDIT_EVENTS{let drop_count=st.pending.len()-MAX_PENDING_AUDIT_EVENTS;st.pending.drain(0..drop_count);}
 }
 pub fn flush(&self){let(lock,_)=&*self.state;let mut st=lock.lock().unwrap_or_else(|p|p.into_inner());flush_locked(&mut st,&self.outbox_path,&*self.enabled,&*self.send_batch,&*self.now);}
 pub fn dispose(&self){
  {let(lock,cv)=&*self.state;let mut st=lock.lock().unwrap_or_else(|p|p.into_inner());st.accepting=false;st.stopped=true;cv.notify_all();}
  if let Some(h)=self.worker.lock().unwrap_or_else(|p|p.into_inner()).take(){let _=h.join();}
  self.flush();let(lock,_)=&*self.state;let st=lock.lock().unwrap_or_else(|p|p.into_inner());persist(&self.outbox_path,&st.pending);
 }
 pub fn pending_len(&self)->usize{let(lock,_)=&*self.state;lock.lock().unwrap_or_else(|p|p.into_inner()).pending.len()}
}
impl Drop for SandActionAuditor{fn drop(&mut self){self.dispose();}}
fn load(path:&Path)->Vec<AuditEvent>{fs::read_to_string(path).ok().and_then(|s|serde_json::from_str::<Vec<AuditEvent>>(&s).ok()).unwrap_or_default().into_iter().take(MAX_PENDING_AUDIT_EVENTS).collect()}
fn persist(path:&Path,pending:&[AuditEvent]){if pending.is_empty(){let _=fs::remove_file(path);return}if let Some(parent)=path.parent(){let _=fs::create_dir_all(parent);}let temp=PathBuf::from(format!("{}.{}.tmp",path.display(),std::process::id()));if fs::write(&temp,serde_json::to_vec(pending).unwrap_or_default()).is_ok(){let _=fs::rename(temp,path);}}
fn flush_locked(st:&mut State,outbox:&Path,enabled:&dyn Fn()->bool,send:&dyn Fn(&[AuditEvent])->Result<(),AuditSendError>,now:&dyn Fn()->u64){
 if !st.loaded{let prior=load(outbox);let room=MAX_PENDING_AUDIT_EVENTS.saturating_sub(prior.len());let tail=st.pending.iter().rev().take(room).cloned().collect::<Vec<_>>();st.pending=prior.into_iter().chain(tail.into_iter().rev()).collect();st.loaded=true}
 let t=now();if t<st.backoff_until_ms{return}if st.pending.is_empty()||!enabled(){persist(outbox,&st.pending);return}
 while !st.pending.is_empty(){let batch=st.pending.iter().take(MAX_AUDIT_BATCH_SIZE).cloned().collect::<Vec<_>>();match send(&batch){Ok(())=>{let ids=batch.iter().map(|e|e.event_id.clone()).collect::<HashSet<_>>();st.pending.retain(|e|!ids.contains(&e.event_id));},Err(error)=>{st.backoff_until_ms=t.saturating_add(flush_failure_backoff_ms(&error));persist(outbox,&st.pending);return}}}
 persist(outbox,&st.pending);
}
