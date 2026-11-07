//! Native definitions through the existing CloudSyncClient reconciliation port.
use std::collections::{BTreeMap,BTreeSet};
use std::sync::{Arc,Mutex,MutexGuard};
use serde::{Deserialize,Serialize};
use serde_json::{Value,json};
use reqwest::Method;
use crate::extensions::auth::auth_service::FabushiCoreCredentialProvider;
use super::automations_proto::*;
use super::backend_transport::AutomationsBackendError;
use super::connect_unary::SandConnectError;
use super::native_backend_transport::NativeAutomationsBackendTransport;
use super::sand_automation_cloud_sync::CloudSyncClient;

const PROTOCOL:&str="fabushi.automation.definition.v1";
fn valid_id(value:&str)->bool {!value.is_empty()&&value.len()<=200&&value.bytes().all(|b|b.is_ascii_alphanumeric()||b"._:-".contains(&b))}
fn invalid()->SandConnectError {SandConnectError::Decode("Invalid native automation definition or baseline; reconcile before writing.".into())}
fn transport(error:AutomationsBackendError)->SandConnectError {
    match error {
        AutomationsBackendError::Auth(_)=>SandConnectError::Authentication("Native automation account/session changed or unavailable.".into()),
        AutomationsBackendError::Status{path,status}=>SandConnectError::Status{path,status,body:"Native service refused the operation.".into()},
        _=>SandConnectError::Transport("Native automation outcome may be unknown; reconcile before another mutation.".into()),
    }
}
#[derive(Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct NativeDefinition {id:String,agent_id:String,name:String,description:Option<String>,enabled:bool,workflow:Workflow}
impl NativeDefinition {
    fn valid(&self)->bool {
        valid_id(&self.id)&&valid_id(&self.agent_id)&&!self.name.trim().is_empty()&&self.name.len()<=512
        &&self.description.as_ref().is_none_or(|v|v.len()<=4000)
        &&!self.workflow.triggers.is_empty()&&self.workflow.triggers.len()<=32
        &&self.workflow.triggers.iter().all(|t|match t.trigger.as_ref() {
            None=>false,
            Some(trigger::Trigger::Cron(cron))=>{
                let schedule=cron.cron.split_whitespace().collect::<Vec<_>>().join(" ");
                let expression=if let Some(rest)=schedule.strip_prefix("CRON_TZ=").or_else(||schedule.strip_prefix("TZ=")) {
                    match rest.split_once(' ') {Some((zone,expression)) if zone.parse::<chrono_tz::Tz>().is_ok()=>expression,_=>return false}
                }else{schedule.as_str()};
                schedule.len()<=512&&(crate::automations::automation_schedule::parse_every_interval_ms(expression).is_some()
                    ||crate::automations::automation_schedule::compile_cron_matcher(&schedule).is_some())
            },
            _=>true,
        })
        &&!self.workflow.prompts.is_empty()&&self.workflow.prompts.len()<=10
        &&self.workflow.prompts.iter().all(|p|p.prompt.len()<=16384)
        &&serde_json::to_vec(self).is_ok_and(|v|v.len()<=65536)
    }
}
#[derive(Clone,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct Record {id:String,agent_id:String,definition:Option<NativeDefinition>,revision:String,deleted:bool,updated_at:u64,next_runs_ms:Vec<Option<u64>>}
impl Record {
    fn valid(&self,agent:&str)->bool {
        valid_id(&self.id)&&valid_id(&self.revision)&&self.agent_id==agent&&valid_id(agent)
        &&self.updated_at<=9_007_199_254_740_991&&self.next_runs_ms.len()<=32
        &&self.next_runs_ms.iter().all(|v|v.is_none_or(|v|v<=9_007_199_254_740_991))
        &&if self.deleted {self.definition.is_none()&&self.next_runs_ms.is_empty()} else {
            self.definition.as_ref().is_some_and(|d|d.valid()&&d.id==self.id&&d.agent_id==agent&&d.workflow.triggers.len()==self.next_runs_ms.len()
                &&d.workflow.triggers.iter().zip(&self.next_runs_ms).all(|(t,p)|p.is_some()==(d.enabled&&matches!(t.trigger.as_ref(),Some(trigger::Trigger::Cron(_))))))
        }
    }
}
#[derive(Default)]struct Baselines {records:BTreeMap<String,Record>,listed_agents:BTreeSet<String>}
pub struct NativeCloudSyncClient {
    http:NativeAutomationsBackendTransport,
    baselines:Mutex<Baselines>,
    operation:Mutex<()>,
}
impl NativeCloudSyncClient {
    pub fn for_current_account(base:&str,credentials:Arc<dyn FabushiCoreCredentialProvider>)->Result<Self,SandConnectError> {
        Ok(Self{http:NativeAutomationsBackendTransport::for_current_account(base,credentials).map_err(transport)?,baselines:Mutex::new(Baselines::default()),operation:Mutex::new(())})
    }
    fn lock(&self)->Result<MutexGuard<'_,Baselines>,SandConnectError>{self.baselines.lock().map_err(|_|invalid())}
    fn current(&self)->Result<(),SandConnectError>{self.http.ensure_current_account().map_err(transport)}
    fn baseline(&self,id:&str)->Result<Record,SandConnectError>{
        self.current()?;let state=self.lock()?;
        let record=state.records.get(id).ok_or_else(invalid)?;
        if !state.listed_agents.contains(&record.agent_id){return Err(invalid());}
        Ok(record.clone())
    }
    fn publish(&self,agent:&str,id:&str,definition:Option<&NativeDefinition>,body:Value,method:Method)->Result<(),SandConnectError>{
        let result=(||{
            let value=self.http.request_native_json(method,&format!("/v1/automations/{id}"),Some(&body),8*1_048_576).map_err(transport)?;
            if value.get("protocol").and_then(Value::as_str)!=Some(PROTOCOL){return Err(invalid());}
            let record:Record=serde_json::from_value(value.get("record").cloned().ok_or_else(invalid)?).map_err(|_|invalid())?;
            if !record.valid(agent)||record.id!=id||record.definition.as_ref()!=definition||record.deleted!=definition.is_none(){return Err(invalid());}
            self.current()?;self.lock()?.records.insert(id.to_owned(),record);Ok(())
        })();
        if result.is_err(){self.lock()?.listed_agents.remove(agent);}
        result
    }
}
impl CloudSyncClient for NativeCloudSyncClient {
    fn list_sand_automations(&self,agent:&str)->Result<ListAutomationsResponse,SandConnectError>{
        let _operation=self.operation.lock().map_err(|_|invalid())?;self.current()?;
        if !valid_id(agent){return Err(invalid());}
        self.lock()?.listed_agents.remove(agent);
        let mut records=BTreeMap::new();let mut cursor=String::new();let mut complete=false;
        for _ in 0..200 {
            let mut query=url::form_urlencoded::Serializer::new(String::new());
            query.append_pair("agentId",agent).append_pair("cursor",&cursor);
            let query=query.finish();
            let page=self.http.request_native_json(Method::GET,&format!("/v1/automations?{query}"),None,8*1_048_576).map_err(transport)?;
            if page.get("protocol").and_then(Value::as_str)!=Some(PROTOCOL){return Err(invalid());}
            let rows=page.get("records").and_then(Value::as_array).ok_or_else(invalid)?;
            if rows.len()>100{return Err(invalid());}
            let mut last=cursor.clone();
            for raw in rows {
                let record:Record=serde_json::from_value(raw.clone()).map_err(|_|invalid())?;
                if !record.valid(agent)||record.id<=last||records.contains_key(&record.id){return Err(invalid());}
                last=record.id.clone();records.insert(record.id.clone(),record);
            }
            match page.get("nextCursor") {
                Some(Value::Null)=>{complete=true;break;},
                Some(Value::String(next)) if rows.len()==100&&valid_id(next)&&next==&last&&next>&cursor=>cursor=next.clone(),
                _=>return Err(invalid()),
            }
        }
        if !complete{return Err(invalid());}
        self.current()?;
        let workflows=records.values().filter_map(|r|r.definition.as_ref().map(|d|AutomationWithOwner {workflow:Some(Automation{enabled:d.enabled,description:d.description.clone(),automation_id:d.id.clone()})})).collect();
        let mut state=self.lock()?;state.records.retain(|_,r|r.agent_id!=agent);state.records.extend(records);state.listed_agents.insert(agent.to_owned());
        Ok(ListAutomationsResponse{workflows})
    }
    fn create_sand_automation(&self,request:CreateAutomationRequest)->Result<(),SandConnectError>{
        let _operation=self.operation.lock().map_err(|_|invalid())?;self.current()?;
        let definition=NativeDefinition{id:request.sand_automation_id.ok_or_else(invalid)?,agent_id:request.sand_agent_id.ok_or_else(invalid)?,
            name:request.name,description:request.description,enabled:request.enabled.ok_or_else(invalid)?,workflow:request.workflow.ok_or_else(invalid)?};
        if !definition.valid(){return Err(invalid());}
        let old={let state=self.lock()?;if !state.listed_agents.contains(&definition.agent_id){return Err(invalid());}state.records.get(&definition.id).cloned()};
        let mut body=json!({"definition":definition});
        match old {
            None=>body["expectAbsent"]=json!(true),
            Some(record) if record.deleted&&record.agent_id==definition.agent_id=>body["baseRevision"]=json!(record.revision),
            _=>return Err(invalid()),
        }
        self.publish(&definition.agent_id,&definition.id,Some(&definition),body,Method::PUT)
    }
    fn update_sand_automation(&self,request:UpdateAutomationRequest)->Result<(),SandConnectError>{
        let _operation=self.operation.lock().map_err(|_|invalid())?;
        if !valid_id(&request.automation_id){return Err(invalid());}
        let old=self.baseline(&request.automation_id)?;let mut definition=old.definition.ok_or_else(invalid)?;
        if let Some(name)=request.name{definition.name=name;}if let Some(workflow)=request.workflow{definition.workflow=workflow;}
        if let Some(enabled)=request.enabled{definition.enabled=enabled;}if request.description.is_some(){definition.description=request.description;}
        if !definition.valid(){return Err(invalid());}
        self.publish(&definition.agent_id,&definition.id,Some(&definition),json!({"definition":definition,"baseRevision":old.revision}),Method::PUT)
    }
    fn delete_sand_automation(&self,id:&str)->Result<(),SandConnectError>{
        let _operation=self.operation.lock().map_err(|_|invalid())?;
        if !valid_id(id){return Err(invalid());}
        let old=self.baseline(id)?;if old.deleted{return Err(invalid());}
        self.publish(&old.agent_id,id,None,json!({"baseRevision":old.revision}),Method::DELETE)
    }
}
