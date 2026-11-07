use std::collections::{BTreeMap,HashSet};
use std::sync::{Arc,Mutex};

use mahayana_host_runtime::cloud_agents::cloud_agent_transcript_dump::*;
use mahayana_host_runtime::extensions::host_upgrade::host_upgrade_marker::*;
use mahayana_host_runtime::host_roster_bookkeeping::*;

#[derive(Default)]
struct Attachments(Mutex<Vec<Option<String>>>);
impl AttachmentRosterPort for Attachments {
 fn set_fallback_agent_id(&self,id:Option<&str>){self.0.lock().unwrap().push(id.map(str::to_string));}
}
#[derive(Default)]
struct Transcript(Mutex<Vec<String>>);
impl TranscriptRosterPort for Transcript {
 fn live_running_agent_ids(&self)->Vec<String>{self.0.lock().unwrap().clone()}
}
#[derive(Default)]
struct Forever{enrolled:Mutex<Vec<HashSet<String>>>,busy:Mutex<Vec<bool>>}
impl ForeverBoxRosterPort for Forever{
 fn enroll_disk_pressure_reminder(&self,ids:&HashSet<String>){self.enrolled.lock().unwrap().push(ids.clone());}
 fn set_busy(&self,b:bool){self.busy.lock().unwrap().push(b);}
}
struct Snap{enabled:bool,calls:Mutex<Vec<String>>}
impl SnapshotBackstopPort for Snap{
 fn is_enabled(&self)->bool{self.enabled}
 fn schedule_snapshot(&self,id:&str){self.calls.lock().unwrap().push(id.into());}
}
struct BoxSnap{enabled:bool,calls:Mutex<Vec<String>>}
impl BoxStoreSnapshotPort for BoxSnap{
 fn is_enabled(&self)->bool{self.enabled}
 fn schedule_store_db_snapshot(&self,id:&str){self.calls.lock().unwrap().push(id.into());}
}
#[derive(Default)]
struct SourceMap(Mutex<Vec<String>>);
impl SourceMapRosterPort for SourceMap{fn get_or_create(&self,id:&str){self.0.lock().unwrap().push(id.into());}}

#[test]
fn roster_bookkeeping_projects_active_busy_started_and_stopped_agents() {
 let attachments=Arc::new(Attachments::default());
 let transcript=Arc::new(Transcript::default());
 let forever=Arc::new(Forever::default());
 let state=Arc::new(Snap{enabled:true,calls:Mutex::new(Vec::new())});
 let box_store=Arc::new(BoxSnap{enabled:false,calls:Mutex::new(Vec::new())});
 let source=Arc::new(SourceMap::default());
 let mut roster=HostRosterBookkeeping::new(HostRosterPorts{
  attachments:attachments.clone(),transcript:transcript.clone(),forever_box:forever.clone(),
  state_backstop:state.clone(),box_store_sync:box_store.clone(),source_map:source.clone()
 });
 *transcript.0.lock().unwrap()=vec!["a".into(),"b".into()];
 roster.apply(Some("a"));
 assert_eq!(roster.latest_active_agent_id(),Some("a"));
 assert!(roster.is_busy());
 assert_eq!(roster.running_agent_ids(),HashSet::from(["a".into(),"b".into()]));
 assert_eq!(forever.enrolled.lock().unwrap()[0],HashSet::from(["a".into(),"b".into()]));
 assert_eq!(source.0.lock().unwrap().as_slice(),&["a"]);
 *transcript.0.lock().unwrap()=vec!["b".into(),"c".into()];
 roster.apply(Some(""));
 assert_eq!(roster.latest_active_agent_id(),None);
 assert_eq!(forever.enrolled.lock().unwrap()[1],HashSet::from(["c".into()]));
 assert_eq!(state.calls.lock().unwrap().as_slice(),&["a"]);
 assert_eq!(box_store.calls.lock().unwrap().as_slice(),&["a"]);
}

#[test]
fn upgrade_marker_parse_metadata_and_forward_lifecycle_match_frozen_rules() {
 let parsed=parse_host_upgrade_marker(r#"{"outcome":"applied","fromVersion":"1","toVersion":"2","issuedAtMs":100,"appliedAtMs":160,"swapMs":9,"junk":true}"#).unwrap();
 let metadata=compute_host_upgrade_metadata(&parsed,220.0);
 assert_eq!(metadata["outcome"],"applied");
 assert_eq!(metadata["deliver_to_apply_ms"],"60");
 assert_eq!(metadata["swap_ms"],"9");
 assert_eq!(metadata["total_ms"],"120");
 let failed=parse_host_upgrade_marker(r#"{"outcome":"failed","swapError":"boom"}"#).unwrap();
 let metadata=compute_host_upgrade_metadata(&failed,0.0);
 assert_eq!(metadata["phase"],"swap");
 assert_eq!(metadata["error_class"],"boom");
 assert!(parse_host_upgrade_marker("[]").is_none());
}

struct ForwardDeps{
 raw:Mutex<Option<String>>,forwarded:Mutex<HashSet<String>>,emits:Mutex<Vec<BTreeMap<String,String>>>,
 allow_emit:bool,warnings:Mutex<Vec<String>>,deleted:Mutex<usize>,
}
impl HostUpgradeMarkerForwardDeps for ForwardDeps{
 fn read_raw(&self)->Result<Option<String>,String>{Ok(self.raw.lock().unwrap().clone())}
 fn delete_marker(&self)->Result<(),String>{*self.deleted.lock().unwrap()+=1;*self.raw.lock().unwrap()=None;Ok(())}
 fn emit(&self,m:&BTreeMap<String,String>)->Result<bool,String>{self.emits.lock().unwrap().push(m.clone());Ok(self.allow_emit)}
 fn warn(&self,m:&str){self.warnings.lock().unwrap().push(m.into())}
 fn now(&self,_:&str)->f64{200.0}
 fn was_forwarded(&self,r:&str)->bool{self.forwarded.lock().unwrap().contains(r)}
 fn mark_forwarded(&self,r:&str){self.forwarded.lock().unwrap().insert(r.into());}
 fn on_forwarded(&self,_:&HostUpgradeMarker){}
}
#[test]
fn upgrade_marker_deferred_does_not_retire_but_parse_error_does() {
 let deferred=ForwardDeps{raw:Mutex::new(Some(r#"{"outcome":"applied"}"#.into())),forwarded:Mutex::new(HashSet::new()),emits:Mutex::new(Vec::new()),allow_emit:false,warnings:Mutex::new(Vec::new()),deleted:Mutex::new(0)};
 assert_eq!(forward_host_upgrade_marker_with(&deferred).unwrap(),HostUpgradeMarkerForwardOutcome::Deferred);
 assert_eq!(*deferred.deleted.lock().unwrap(),0);
 let bad=ForwardDeps{raw:Mutex::new(Some("{".into())),forwarded:Mutex::new(HashSet::new()),emits:Mutex::new(Vec::new()),allow_emit:true,warnings:Mutex::new(Vec::new()),deleted:Mutex::new(0)};
 assert_eq!(forward_host_upgrade_marker_with(&bad).unwrap(),HostUpgradeMarkerForwardOutcome::ParseError);
 assert_eq!(*bad.deleted.lock().unwrap(),1);
 assert_eq!(bad.warnings.lock().unwrap().len(),1);
}

struct DumpApi(Option<CloudAgentTranscriptDump>);
impl CloudAgentDumpApi for DumpApi{
 fn get_transcript_dump(&self,_:&str)->Result<Option<CloudAgentTranscriptDump>,String>{Ok(self.0.clone())}
}
#[derive(Default)]
struct Writer(Mutex<Vec<(String,Vec<u8>)>>);
impl CloudAgentBoxFileWriter for Writer{
 fn write_box_file(&self,p:&str,d:&[u8])->Result<(),String>{self.0.lock().unwrap().push((p.into(),d.to_vec()));Ok(())}
}
#[derive(Clone,PartialEq,Eq,Debug)]
struct Watch{ text:String, code:u32 }
impl CloudAgentWatchResult for Watch{
 fn text(&self)->&str{&self.text}
 fn with_text(&self,text:String)->Self{Self{text,code:self.code}}
}
#[test]
fn cloud_transcript_dump_writes_bytes_and_augments_without_losing_result_fields() {
 let api=DumpApi(Some(CloudAgentTranscriptDump{status:"done".into(),line_count:2,jsonl:"a\nb\n".into()}));
 let writer=Writer::default();
 let outcome=dump_cloud_agent_transcript(&api,&writer,"bc-1").unwrap().unwrap();
 assert_eq!(outcome.file.as_ref().unwrap().path,"cloud-agent-transcripts/bc-1.jsonl");
 assert_eq!(outcome.file.as_ref().unwrap().size_bytes,4);
 assert_eq!(writer.0.lock().unwrap()[0].1,b"a\nb\n");
 let base=Watch{text:"finished".into(),code:7};
 let augmented=augment_watch_result_with_transcript_dump(&api,&writer,"bc-1",&base);
 assert_eq!(augmented.code,7);
 assert!(augmented.text.contains("Full transcript dumped to cloud-agent-transcripts/bc-1.jsonl (4 bytes, 2 message lines)."));
 let empty=DumpApi(Some(CloudAgentTranscriptDump{status:"done".into(),line_count:0,jsonl:String::new()}));
 assert!(dump_cloud_agent_transcript(&empty,&writer,"bc-2").unwrap().unwrap().file.is_none());
}
