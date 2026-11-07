use std::collections::BTreeMap;
use std::fs;
use std::io::{Read,Write};
use std::net::{TcpListener,TcpStream};
use std::path::PathBuf;
use std::sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}};
use std::thread;
use std::time::Duration;
use serde_json::{Value,json};
use mahayana_host_runtime::extensions::auth::auth_service::{HostAuthEnvironment,HostAuthService,HostAuthServiceOptions,FABUSHI_HOST_ACCESS_CREDENTIAL_FILE_ENV};
use mahayana_host_runtime::extensions::automations::{automations_proto::*,native_cloud_sync_client::NativeCloudSyncClient,sand_automation_cloud_sync::CloudSyncClient};

struct Account {root:PathBuf,path:PathBuf,auth:Arc<HostAuthService>}
impl Account {
    fn new(base:&str)->Self {
        let root=std::env::temp_dir().join(format!("native-definition-{}",uuid::Uuid::new_v4()));fs::create_dir_all(&root).unwrap();
        let path=root.join("account.json");write_account(&path,base,"a");
        let auth=Arc::new(HostAuthService::new(HostAuthServiceOptions{environment:HostAuthEnvironment::from_pairs(BTreeMap::from([(FABUSHI_HOST_ACCESS_CREDENTIAL_FILE_ENV.into(),path.to_string_lossy().into_owned())])),
            backend_url:Some(base.into()),backend:None,machine_id:Arc::new(||Ok("device".into())),log:Arc::new(|_|{}),now_ms:Arc::new(||10000)}).unwrap());
        Self{root,path,auth}
    }
}
fn write_account(path:&PathBuf,base:&str,user:&str) {fs::write(path,json!({"version":1,"issuer":base,"userId":user,"accessToken":"native-definition-fixture","sessionId":format!("session-{user}"),"deviceId":"device","expiresAtMs":600000}).to_string()).unwrap();}
impl Drop for Account {fn drop(&mut self){self.auth.dispose();let _=fs::remove_dir_all(&self.root);}}
#[derive(Default)]struct State {records:BTreeMap<String,Value>,calls:Vec<String>,revision:u64,lose_response:bool,override_response:Option<Value>,redirect:Option<String>}
struct Service {base:String,state:Arc<Mutex<State>>,stop:Arc<AtomicBool>,worker:Option<thread::JoinHandle<()>>}
fn read(stream:&mut TcpStream)->(String,Value) {
    stream.set_nonblocking(false).unwrap();stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut bytes=Vec::new();loop {let mut buffer=[0;8192];let n=stream.read(&mut buffer).unwrap();assert!(n>0);bytes.extend_from_slice(&buffer[..n]);assert!(bytes.len()<100000);
        if let Some(end)=bytes.windows(4).position(|v|v==b"\r\n\r\n") {
            let header=String::from_utf8_lossy(&bytes[..end]);let length=header.lines().find_map(|l|l.to_ascii_lowercase().strip_prefix("content-length:").map(|v|v.trim().parse::<usize>().unwrap())).unwrap_or(0);
            if bytes.len()>=end+4+length {return(header.into_owned(),if length==0 {Value::Null}else{serde_json::from_slice(&bytes[end+4..end+4+length]).unwrap()});}
        }
    }
}
fn response(stream:&mut TcpStream,status:u16,value:&Value,location:Option<&str>) {
    let body=value.to_string();let location=location.map(|v|format!("Location: {v}\r\n")).unwrap_or_default();
    write!(stream,"HTTP/1.1 {status} Fixture\r\n{location}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
}
fn record(definition:Value,revision:u64)->Value {
    let positions=definition["workflow"]["triggers"].as_array().unwrap().iter().map(|t|if definition["enabled"]==true&&!t["trigger"]["Cron"].is_null(){json!(1000)}else{Value::Null}).collect::<Vec<_>>();
    json!({"id":definition["id"],"agentId":definition["agentId"],"definition":definition,"revision":format!("revision-{revision}"),"deleted":false,"updatedAt":1,"nextRunsMs":positions})
}
impl Service {
    fn new()->Self {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();let base=format!("http://{}",listener.local_addr().unwrap());
        let state=Arc::new(Mutex::new(State::default()));let stop=Arc::new(AtomicBool::new(false));let data=state.clone();let stopping=stop.clone();
        let worker=thread::spawn(move||while !stopping.load(Ordering::Acquire) {
            let (mut stream,_)=match listener.accept(){Ok(v)=>v,Err(e)if e.kind()==std::io::ErrorKind::WouldBlock=>{thread::sleep(Duration::from_millis(2));continue;},Err(e)=>panic!("{e}")};
            let (headers,body)=read(&mut stream);let first=headers.lines().next().unwrap();let mut first=first.split_whitespace();let method=first.next().unwrap();let target=first.next().unwrap();
            let lower=headers.to_ascii_lowercase();assert!(lower.contains("authorization: bearer native-definition-fixture"));assert!(!lower.contains("x-cursor"));
            let url=url::Url::parse(&format!("http://fixture{target}")).unwrap();let id=url.path().strip_prefix("/v1/automations/").unwrap_or("");
            let mut state=data.lock().unwrap();state.calls.push(format!("{method} {target}"));
            if let Some(location)=state.redirect.as_deref(){response(&mut stream,307,&json!({}),Some(location));continue;}
            if let Some(value)=state.override_response.as_ref(){response(&mut stream,200,value,None);continue;}
            let (status,value)=if method=="GET"&&url.path()=="/v1/automations" {
                let query=url.query_pairs().collect::<BTreeMap<_,_>>();let agent=query.get("agentId").unwrap().as_ref();let cursor=query.get("cursor").unwrap().as_ref();
                let rows=state.records.iter().filter(|(id,r)|id.as_str()>cursor&&r["agentId"]==agent).take(100).map(|(_,r)|r.clone()).collect::<Vec<_>>();
                let cursor=if rows.len()==100 {rows.last().unwrap()["id"].clone()}else{Value::Null};
                (200,json!({"protocol":"fabushi.automation.definition.v1","records":rows,"nextCursor":cursor}))
            } else if method=="PUT" {
                let previous=state.records.get(id);let allowed=match previous {None=>body["expectAbsent"]==true,Some(r)=>body["baseRevision"]==r["revision"]};
                if !allowed {(409,json!({}))} else {
                    state.revision+=1;let value=record(body["definition"].clone(),state.revision);state.records.insert(id.into(),value.clone());
                    (200,json!({"protocol":"fabushi.automation.definition.v1","record":value}))
                }
            } else if method=="DELETE" {
                let allowed=state.records.get(id).is_some_and(|r|r["revision"]==body["baseRevision"]);
                if !allowed {(409,json!({}))}else{
                    state.revision+=1;let revision=state.revision;let value=state.records.get_mut(id).unwrap();
                    value["definition"]=Value::Null;value["deleted"]=json!(true);value["nextRunsMs"]=json!([]);value["revision"]=json!(format!("revision-{revision}"));
                    (200,json!({"protocol":"fabushi.automation.definition.v1","record":value.clone()}))
                }
            }else{(404,json!({}))};
            if state.lose_response {state.lose_response=false;continue;}
            response(&mut stream,status,&value,None);
        });Self{base,state,stop,worker:Some(worker)}
    }
}
impl Drop for Service {fn drop(&mut self){self.stop.store(true,Ordering::Release);if let Some(worker)=self.worker.take(){let result=worker.join();if !thread::panicking(){result.unwrap();}}}}
fn workflow()->Workflow {Workflow{triggers:vec![Trigger{trigger:Some(trigger::Trigger::Cron(CronTrigger{cron:"@every 1s".into()}))},Trigger{trigger:Some(trigger::Trigger::Git(GitTrigger{event:Some(git_trigger::Event::PullRequest(GitPullRequestEvent{repos:vec!["owner/repo".into()],pr_action:1})),user_allowlist:vec!["human".into()]}))}],prompts:vec![Prompt{prompt:"native routine".into()}]}}
fn create(id:&str)->CreateAutomationRequest {CreateAutomationRequest{name:"Native".into(),workflow:Some(workflow()),description:Some("native-marker".into()),enabled:Some(true),sand_agent_id:Some("agent-a".into()),sand_automation_id:Some(id.into())}}
fn update(id:&str)->UpdateAutomationRequest {UpdateAutomationRequest{automation_id:id.into(),name:Some("Updated".into()),enabled:Some(false),..Default::default()}}

#[test]fn native_existing_port_preserves_pagination_workflows_cas_and_tombstone_recreation(){
    let server=Service::new();let account=Account::new(&server.base);let client=NativeCloudSyncClient::for_current_account(&server.base,account.auth.clone()).unwrap();
    assert!(client.create_sand_automation(create("new")).is_err());assert!(server.state.lock().unwrap().calls.is_empty());
    for index in 0..101 {let id=format!("clock-{index:03}");server.state.lock().unwrap().records.insert(id.clone(),record(json!({"id":id,"agentId":"agent-a","name":"Native","description":"marker","enabled":true,"workflow":workflow()}),1000+index));}
    assert_eq!(client.list_sand_automations("agent-a").unwrap().workflows.len(),101);
    assert_eq!(server.state.lock().unwrap().calls.len(),2);
    client.create_sand_automation(create("new")).unwrap();
    assert_eq!(server.state.lock().unwrap().records["new"]["definition"]["workflow"],serde_json::to_value(workflow()).unwrap());
    client.update_sand_automation(update("new")).unwrap();assert_eq!(server.state.lock().unwrap().records["new"]["definition"]["enabled"],false);
    client.delete_sand_automation("new").unwrap();assert_eq!(client.list_sand_automations("agent-a").unwrap().workflows.len(),101);
    client.create_sand_automation(create("new")).unwrap();assert_eq!(server.state.lock().unwrap().records["new"]["deleted"],false);
}
#[test]fn conflicts_and_lost_responses_require_fresh_reconciliation_without_write_retry(){
    let server=Service::new();let account=Account::new(&server.base);let client=NativeCloudSyncClient::for_current_account(&server.base,account.auth.clone()).unwrap();
    client.list_sand_automations("agent-a").unwrap();client.create_sand_automation(create("routine")).unwrap();
    server.state.lock().unwrap().records.get_mut("routine").unwrap()["revision"]=json!("foreign-device-version");
    assert!(client.update_sand_automation(update("routine")).is_err());let calls=server.state.lock().unwrap().calls.len();
    assert!(client.update_sand_automation(update("routine")).is_err());assert_eq!(server.state.lock().unwrap().calls.len(),calls);
    client.list_sand_automations("agent-a").unwrap();server.state.lock().unwrap().lose_response=true;
    let before=server.state.lock().unwrap().calls.len();
    assert!(client.update_sand_automation(update("routine")).is_err());let calls=server.state.lock().unwrap().calls.len();assert_eq!(calls,before+1,"unknown write must have one network attempt");
    assert!(client.delete_sand_automation("routine").is_err());assert_eq!(server.state.lock().unwrap().calls.len(),calls);
    let readback=client.list_sand_automations("agent-a").unwrap();assert!(!readback.workflows[0].workflow.as_ref().unwrap().enabled);
}

#[test]fn native_workflow_serde_preserves_every_projected_trigger_variant_and_protobuf_hash_input(){
    use prost::Message;
    let variants=vec![trigger::Trigger::Cron(CronTrigger{cron:"CRON_TZ=Asia/Shanghai @daily".into()}),
        trigger::Trigger::Git(GitTrigger::default()),trigger::Trigger::SlackTrigger(SlackTrigger::default()),
        trigger::Trigger::Linear(LinearTrigger::default()),trigger::Trigger::Pagerduty(PagerDutyTrigger::default()),
        trigger::Trigger::Sentry(SentryTrigger::default()),trigger::Trigger::MicrosoftTeamsTrigger(MicrosoftTeamsTrigger::default()),
        trigger::Trigger::SlackReactionAdded(SlackReactionAddedTrigger::default()),trigger::Trigger::SlackMention(SlackMentionTrigger::default()),
        trigger::Trigger::SlackAnyReactionAdded(SlackAnyReactionAddedTrigger::default())];
    let workflow=Workflow{triggers:variants.into_iter().map(|t|Trigger{trigger:Some(t)}).collect(),prompts:vec![Prompt{prompt:"preserved".into()}]};
    let json=serde_json::to_value(&workflow).unwrap();
    let restored:Workflow=serde_json::from_value(json.clone()).unwrap();assert_eq!(workflow,restored);assert_eq!(workflow.encode_to_vec(),restored.encode_to_vec());
    let mut invalid=json;invalid["triggers"][0]["unknown"]=json!(true);assert!(serde_json::from_value::<Workflow>(invalid).is_err());
}
#[test]fn malformed_pages_origins_account_changes_and_redirects_never_become_empty_success(){
    let server=Service::new();let account=Account::new(&server.base);
    assert!(NativeCloudSyncClient::for_current_account("https://api2.cursor.sh",account.auth.clone()).is_err());
    let client=NativeCloudSyncClient::for_current_account(&server.base,account.auth.clone()).unwrap();
    server.state.lock().unwrap().override_response=Some(json!({"records":[],"nextCursor":null}));assert!(client.list_sand_automations("agent-a").is_err());
    server.state.lock().unwrap().override_response=Some(json!({"protocol":"fabushi.automation.definition.v1","records":[],"nextCursor":"loop"}));assert!(client.list_sand_automations("agent-a").is_err());
    server.state.lock().unwrap().override_response=None;
    let destination=TcpListener::bind("127.0.0.1:0").unwrap();destination.set_nonblocking(true).unwrap();
    server.state.lock().unwrap().redirect=Some(format!("http://{}/capture",destination.local_addr().unwrap()));assert!(client.list_sand_automations("agent-a").is_err());assert!(destination.accept().is_err());
    let calls=server.state.lock().unwrap().calls.len();write_account(&account.path,&server.base,"b");
    assert!(client.list_sand_automations("agent-a").is_err());assert_eq!(server.state.lock().unwrap().calls.len(),calls);
}
