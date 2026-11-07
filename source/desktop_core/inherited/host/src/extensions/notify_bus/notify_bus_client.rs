use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use reqwest::Client;
use tokio::runtime::Builder;
use tokio::sync::watch;
use url::Url;

use crate::extensions::auth::credential_renewer::{
    SAND_CLIENT_TYPE, sand_box_namespace, sand_client_version,
};

pub const HEALTHY_CONNECTION_MIN_LIFETIME_MS:u64=30_000;
pub const RECONNECT_INITIAL_DELAY_MS:u64=1_000;
pub const RECONNECT_MAX_DELAY_MS:u64=60_000;
pub const NOTIFY_STREAM_STALL_MS:u64=35_000;

#[derive(Debug,Clone,Copy,PartialEq,Eq,Hash,Ord,PartialOrd)]
pub enum SandNotifyTopic{AutomationFires,ListenerEvents,XuserEvents}
impl SandNotifyTopic{
 pub const ALL:[Self;3]=[Self::AutomationFires,Self::ListenerEvents,Self::XuserEvents];
 pub const fn as_str(self)->&'static str{match self{Self::AutomationFires=>"automation-fires",Self::ListenerEvents=>"listener-events",Self::XuserEvents=>"xuser-events"}}
 pub fn parse(value:&str)->Option<Self>{match value{"automation-fires"=>Some(Self::AutomationFires),"listener-events"=>Some(Self::ListenerEvents),"xuser-events"=>Some(Self::XuserEvents),_=>None}}
}
#[derive(Debug,Clone,PartialEq,Eq)]
pub enum SandNotifyFrame{Connected,Notify(SandNotifyTopic),Ignored}
pub fn parse_notify_frame(frame:&str)->SandNotifyFrame{
 let mut data=None;
 for line in frame.split('\n'){if let Some(raw)=line.strip_prefix("data: "){data=Some(raw)}}
 let Some(raw)=data else{return SandNotifyFrame::Ignored};
 let Ok(value)=serde_json::from_str::<serde_json::Value>(raw) else{return SandNotifyFrame::Ignored};
 let Some(record)=value.as_object() else{return SandNotifyFrame::Ignored};
 if record.get("kind").and_then(|v|v.as_str())==Some("connected"){return SandNotifyFrame::Connected}
 if record.get("kind").and_then(|v|v.as_str())==Some("notify"){
  if let Some(topic)=record.get("topic").and_then(|v|v.as_str()).and_then(SandNotifyTopic::parse){return SandNotifyFrame::Notify(topic)}
 }
 SandNotifyFrame::Ignored
}

#[derive(Debug,Clone,PartialEq,Eq,thiserror::Error)]
pub enum SandNotifyStreamError{
 #[error("sand notify stream failed: {0}")] Status(u16),
 #[error("sand notify stream stalled")] Stalled,
 #[error("sand notify transport failed: {0}")] Transport(String),
 #[error("sand notify backend invalid: {0}")] Backend(String),
 #[error("sand notify auth failed: {0}")] Auth(String),
}

#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub struct NotifyBusTiming{
 pub healthy_connection_min_lifetime_ms:u64,
 pub reconnect_initial_delay_ms:u64,
 pub reconnect_max_delay_ms:u64,
 pub stall_ms:u64,
}
impl Default for NotifyBusTiming{
 fn default()->Self{Self{healthy_connection_min_lifetime_ms:HEALTHY_CONNECTION_MIN_LIFETIME_MS,reconnect_initial_delay_ms:RECONNECT_INITIAL_DELAY_MS,reconnect_max_delay_ms:RECONNECT_MAX_DELAY_MS,stall_ms:NOTIFY_STREAM_STALL_MS}}
}
pub fn reconnect_delay_ms(attempt:u32,timing:NotifyBusTiming)->u64{
 if attempt==0{return 0}
 let exponent=attempt.saturating_sub(1).min(62);
 timing.reconnect_initial_delay_ms.saturating_mul(1u64<<exponent).min(timing.reconnect_max_delay_ms)
}
pub fn next_reconnect_attempt(connected_at_ms:Option<u64>,now_ms:u64,attempt:u32,timing:NotifyBusTiming)->u32{
 let healthy=connected_at_ms.is_some_and(|at|now_ms.saturating_sub(at)>=timing.healthy_connection_min_lifetime_ms);
 if healthy{0}else{attempt.saturating_add(1)}
}

#[derive(Default)]
struct SseFrameDecoder{buffer:Vec<u8>}
impl SseFrameDecoder{
 fn push(&mut self,chunk:&[u8])->Vec<String>{
  self.buffer.extend_from_slice(chunk);let mut out=Vec::new();
  loop{
   let boundary=self.buffer.windows(2).position(|w|w==b"\n\n");
   let Some(at)=boundary else{break};
   let frame=self.buffer.drain(..at).collect::<Vec<_>>();
   self.buffer.drain(..2);
   out.push(String::from_utf8_lossy(&frame).into_owned());
  }
  out
 }
}

pub struct NotifyBusClientDependencies{
 pub get_backend_url:Arc<dyn Fn()->Result<String,String>+Send+Sync>,
 pub get_access_token:Arc<dyn Fn(&str)->Result<String,String>+Send+Sync>,
 pub on_connected:Arc<dyn Fn()+Send+Sync>,
 pub on_notify:Arc<dyn Fn(SandNotifyTopic)+Send+Sync>,
 pub on_stream_error:Arc<dyn Fn(&SandNotifyStreamError)+Send+Sync>,
 pub now_ms:Arc<dyn Fn()->u64+Send+Sync>,
 pub timing:NotifyBusTiming,
}
impl NotifyBusClientDependencies{
 pub fn with_clock(
  get_backend_url:Arc<dyn Fn()->Result<String,String>+Send+Sync>,
  get_access_token:Arc<dyn Fn(&str)->Result<String,String>+Send+Sync>,
  on_connected:Arc<dyn Fn()+Send+Sync>,
  on_notify:Arc<dyn Fn(SandNotifyTopic)+Send+Sync>,
  on_stream_error:Arc<dyn Fn(&SandNotifyStreamError)+Send+Sync>,
 )->Self{
  Self{get_backend_url,get_access_token,on_connected,on_notify,on_stream_error,now_ms:Arc::new(||SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().try_into().unwrap_or(u64::MAX)),timing:NotifyBusTiming::default()}
 }
}
struct RuntimeState{connected_at_ms:Option<u64>}

pub struct SandNotifyBusClient{
 deps:Arc<NotifyBusClientDependencies>,
 http:Client,
 state:Arc<Mutex<RuntimeState>>,
 stop_tx:Mutex<Option<watch::Sender<bool>>>,
 worker:Mutex<Option<JoinHandle<()>>>,
}
impl SandNotifyBusClient{
 pub fn new(deps:NotifyBusClientDependencies)->Result<Self,SandNotifyStreamError>{
  let http=Client::builder().connect_timeout(Duration::from_secs(15)).build().map_err(|e|SandNotifyStreamError::Transport(e.to_string()))?;
  Ok(Self{deps:Arc::new(deps),http,state:Arc::new(Mutex::new(RuntimeState{connected_at_ms:None})),stop_tx:Mutex::new(None),worker:Mutex::new(None)})
 }
 pub fn is_connected(&self)->bool{self.state.lock().unwrap_or_else(|p|p.into_inner()).connected_at_ms.is_some()}
 pub fn start(&self){
  let mut slot=self.stop_tx.lock().unwrap_or_else(|p|p.into_inner());if slot.is_some(){return}
  let (tx,rx)=watch::channel(false);*slot=Some(tx);drop(slot);
  let deps=Arc::clone(&self.deps);let state=Arc::clone(&self.state);let http=self.http.clone();
  let worker=thread::Builder::new().name("sand-notify-bus".into()).spawn(move||{
   let runtime=Builder::new_current_thread().enable_all().build();
   match runtime{Ok(runtime)=>runtime.block_on(run_loop(deps,http,state,rx)),Err(error)=>(deps.on_stream_error)(&SandNotifyStreamError::Transport(error.to_string()))}
  });
  if let Ok(worker)=worker{*self.worker.lock().unwrap_or_else(|p|p.into_inner())=Some(worker)}
 }
 pub fn stop(&self){
  if let Some(tx)=self.stop_tx.lock().unwrap_or_else(|p|p.into_inner()).take(){let _=tx.send(true);}
  if let Some(worker)=self.worker.lock().unwrap_or_else(|p|p.into_inner()).take(){let _=worker.join();}
  self.state.lock().unwrap_or_else(|p|p.into_inner()).connected_at_ms=None;
 }
}
impl Drop for SandNotifyBusClient{fn drop(&mut self){self.stop()}}

async fn run_loop(deps:Arc<NotifyBusClientDependencies>,http:Client,state:Arc<Mutex<RuntimeState>>,mut cancel:watch::Receiver<bool>){
 let mut attempt=0u32;
 while !*cancel.borrow(){
  let result=stream_once(&deps,&http,&state,&mut cancel).await;
  if !*cancel.borrow(){if let Err(error)=result{(deps.on_stream_error)(&error)}}
  if *cancel.borrow(){break}
  let now=(deps.now_ms)();
  let connected_at={let mut guard=state.lock().unwrap_or_else(|p|p.into_inner());let at=guard.connected_at_ms;guard.connected_at_ms=None;at};
  attempt=next_reconnect_attempt(connected_at,now,attempt,deps.timing);
  if attempt>0{
   let delay=Duration::from_millis(reconnect_delay_ms(attempt,deps.timing));
   tokio::select!{_ = cancel.changed()=>{},_ = tokio::time::sleep(delay)=>{}}
  }
 }
}
async fn stream_once(deps:&NotifyBusClientDependencies,http:&Client,state:&Mutex<RuntimeState>,cancel:&mut watch::Receiver<bool>)->Result<(),SandNotifyStreamError>{
 let backend=(deps.get_backend_url)().map_err(SandNotifyStreamError::Backend)?;
 let token=(deps.get_access_token)(&backend).map_err(SandNotifyStreamError::Auth)?;
 if *cancel.borrow(){return Ok(())}
 let base=Url::parse(&backend).map_err(|e|SandNotifyStreamError::Backend(e.to_string()))?;
 let url=base.join("/sand/notify").map_err(|e|SandNotifyStreamError::Backend(e.to_string()))?;
 let send=http.get(url).bearer_auth(token).header("accept","text/event-stream")
  .header("x-cursor-client-type",SAND_CLIENT_TYPE).header("x-cursor-client-version",sand_client_version()).header("x-sand-box-namespace",sand_box_namespace()).send();
 let response=tokio::select!{
  _=cancel.changed()=>return Ok(()),
  result=tokio::time::timeout(Duration::from_millis(deps.timing.stall_ms),send)=>result.map_err(|_|SandNotifyStreamError::Stalled)?.map_err(|e|SandNotifyStreamError::Transport(e.to_string()))?
 };
 if !response.status().is_success(){return Err(SandNotifyStreamError::Status(response.status().as_u16()))}
 let mut stream=response.bytes_stream();let mut decoder=SseFrameDecoder::default();
 loop{
  let next=tokio::select!{
   _=cancel.changed()=>return Ok(()),
   result=tokio::time::timeout(Duration::from_millis(deps.timing.stall_ms),stream.next())=>result.map_err(|_|SandNotifyStreamError::Stalled)?
  };
  let Some(chunk)=next else{break};let chunk=chunk.map_err(|e|SandNotifyStreamError::Transport(e.to_string()))?;
  for frame in decoder.push(&chunk){
   match parse_notify_frame(&frame){
    SandNotifyFrame::Connected=>{state.lock().unwrap_or_else(|p|p.into_inner()).connected_at_ms=Some((deps.now_ms)());(deps.on_connected)()}
    SandNotifyFrame::Notify(topic)=>(deps.on_notify)(topic),
    SandNotifyFrame::Ignored=>{}
   }
  }
 }
 Ok(())
}
