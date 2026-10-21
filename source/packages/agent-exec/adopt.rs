use serde::ser::Serializer;
use serde::{Deserialize,Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
#[derive(Debug,Clone,PartialEq,Eq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct AdoptArgs{pub source_agent_id:String}
impl AdoptArgs{pub fn from_value(v:&Value)->Self{Self{source_agent_id:v.get("source_agent_id").or_else(||v.get("sourceAgentId")).and_then(Value::as_str).unwrap_or_default().trim().to_owned()}}}
#[derive(Debug,Clone,Copy,PartialEq,Eq)]
#[repr(i32)]
pub enum AdoptOutcome{Unspecified=0,AlreadyParented=1,EdgeOnly=2,StoreImportCompleted=3}
impl Serialize for AdoptOutcome{fn serialize<S>(&self,s:S)->Result<S::Ok,S::Error>where S:Serializer{s.serialize_i32(*self as i32)}}
#[derive(Debug,Clone,PartialEq,Eq,Serialize)]
#[serde(tag="case",content="value")]
pub enum AdoptResultValue{#[serde(rename="success")]Success(AdoptOutcome),#[serde(rename="error")]Error(String)}
#[derive(Debug,Clone,PartialEq,Eq,Serialize)]
#[serde(rename_all="camelCase")]
pub struct AdoptResult{pub source_agent_id:String,pub target_agent_id:String,pub project_root_id:String,pub result:Option<AdoptResultValue>}
#[derive(Debug,Clone,PartialEq,Eq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct AgentAdoptionEdge{pub target_agent_id:String,pub project_root_id:String}
pub struct AdoptExecutorResource;
impl AdoptExecutorResource{
pub fn execute(args:AdoptArgs,target:&str,root:&str,edges:&mut BTreeMap<String,AgentAdoptionEdge>)->AdoptResult{
let source=args.source_agent_id.trim().to_owned();let target=target.trim().to_owned();let root=root.trim().to_owned();
let mut out=AdoptResult{source_agent_id:source.clone(),target_agent_id:target.clone(),project_root_id:root.clone(),result:None};
let error=if source.is_empty(){Some("sourceAgentId is required")}else if target.is_empty(){Some("targetAgentId is required")}else if root.is_empty(){Some("projectRootId requires an active workspace root")}else if source==target{Some("cannot adopt an agent into itself")}else{None};
if let Some(e)=error{out.result=Some(AdoptResultValue::Error(e.into()));return out}
match edges.get(&source){Some(e)if e.target_agent_id==target&&e.project_root_id==root=>out.result=Some(AdoptResultValue::Success(AdoptOutcome::AlreadyParented)),Some(_)=>out.result=Some(AdoptResultValue::Error("source agent is already adopted by a different target or project root".into())),None=>{edges.insert(source,AgentAdoptionEdge{target_agent_id:target,project_root_id:root});out.result=Some(AdoptResultValue::Success(AdoptOutcome::EdgeOnly));}}out
}}
#[cfg(test)]mod tests{use super::*;use serde_json::json;
#[test]fn generated_contract_and_outcome_numbers(){let mut e=BTreeMap::new();let r=AdoptExecutorResource::execute(AdoptArgs::from_value(&json!({"sourceAgentId":" source "})), "target","/workspace",&mut e);let v=serde_json::to_value(r).unwrap();assert_eq!(v["sourceAgentId"],"source");assert_eq!(v["result"]["value"].as_i64(),Some(2));assert_eq!(serde_json::to_value(AdoptOutcome::StoreImportCompleted).unwrap().as_i64(),Some(3));}
#[test]fn idempotent_edge_and_conflict(){let mut e=BTreeMap::new();let a=AdoptArgs::from_value(&json!({"source_agent_id":"s"}));assert_eq!(AdoptExecutorResource::execute(a.clone(),"t","/p",&mut e).result,Some(AdoptResultValue::Success(AdoptOutcome::EdgeOnly)));assert_eq!(AdoptExecutorResource::execute(a.clone(),"t","/p",&mut e).result,Some(AdoptResultValue::Success(AdoptOutcome::AlreadyParented)));assert!(matches!(AdoptExecutorResource::execute(a,"other","/p",&mut e).result,Some(AdoptResultValue::Error(_))));}
#[test]fn invalid_scope_and_self_adoption_fail_closed(){let mut e=BTreeMap::new();assert!(matches!(AdoptExecutorResource::execute(AdoptArgs::from_value(&json!({})),"t","/p",&mut e).result,Some(AdoptResultValue::Error(_))));assert!(matches!(AdoptExecutorResource::execute(AdoptArgs::from_value(&json!({"source_agent_id":"t"})),"t","/p",&mut e).result,Some(AdoptResultValue::Error(_))));assert!(matches!(AdoptExecutorResource::execute(AdoptArgs::from_value(&json!({"source_agent_id":"s"})),"t","",&mut e).result,Some(AdoptResultValue::Error(_))));assert!(e.is_empty());}}