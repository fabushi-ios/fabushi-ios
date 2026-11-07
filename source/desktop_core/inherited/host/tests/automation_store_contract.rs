use std::fs;use std::sync::{Arc,mpsc};use std::time::{Duration,SystemTime,UNIX_EPOCH};
use mahayana_host_runtime::automations::automation::{AutomationSpec,AUTOMATION_MAX_RUN_HISTORY};
use mahayana_host_runtime::automations::automation_store::{AutomationDefinitionInspectionState,FileAutomationStore,agent_has_automations,inspect_agent_automation_definitions,parse_stored_config};
use mahayana_host_runtime::automations::automation_trigger::{parse_stored_trigger,slack_scope_matches,trigger_schedule};
use mahayana_host_runtime::automations::routine_notices::{GITHUB_LISTENER_SCOPE,routine_notice_ids_to_raise};
use serde_json::json;
fn root(label:&str)->std::path::PathBuf{let n=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();std::env::temp_dir().join(format!("fabushi-automation-{label}-{}-{n}",std::process::id()))}
#[test]fn trigger_parser_normalizes_slack_github_and_group_contract(){let slack=parse_stored_trigger(&json!({"type":"slack","channel":" #eng ","match":{"kind":"reaction","emoji":[":Ship:","ship","BAD SPACE"],"bySelf":true}})).unwrap();assert_eq!(slack["channel"],"#eng");assert_eq!(slack["match"]["emoji"],json!(["ship"]));assert!(slack_scope_matches("#Eng","eng"));assert!(!slack_scope_matches("@Eng","#eng"));let github=parse_stored_trigger(&json!({"type":"github","repo":"Org/Repo","events":["pr-opened","ci-passed"],"userAllowlist":["@Alice","alice"],"ciBranch":" main "})).unwrap();assert_eq!(github["userAllowlist"],json!(["Alice"]));assert_eq!(github["ciBranch"],"main");let group=parse_stored_trigger(&json!({"type":"group","listeners":[{"type":"cron","schedule":"0 9 * * 1-5"},slack]})).unwrap();assert_eq!(trigger_schedule(&group).as_deref(),Some("0 9 * * 1-5"));}
#[test]fn file_store_persists_crud_run_history_and_durable_footprint(){let root=root("crud");let agent=root.join("agent");let dir=agent.join("automations");fs::create_dir_all(&agent).unwrap();let store=FileAutomationStore::new(&dir);assert_eq!(inspect_agent_automation_definitions(&agent).state,AutomationDefinitionInspectionState::DirMissing);let spec=AutomationSpec{name:"Morning Check".into(),prompt:" check inbox ".into(),trigger:json!({"type":"cron","schedule":"0 9 * * 1-5"}),is_enabled:None};let record=store.upsert(&spec,1000.0).unwrap().unwrap();assert_eq!(record.id,"morning-check");assert_eq!(record.prompt,"check inbox");assert!(agent_has_automations(&agent));assert_eq!(inspect_agent_automation_definitions(&agent).valid_definition_count,1);let second=store.upsert(&spec,1100.0).unwrap().unwrap();assert_eq!(second.id,"morning-check-2");assert!(!store.set_enabled(&record.id,false).unwrap().unwrap().is_enabled);let run=store.begin_run(&record.id,"manual",1200.0,Some(" evt "),Some("run-1"),None).unwrap().unwrap();assert_eq!(run.trigger,"manual");let finished=store.finish_run(&record.id,"run-1","error",1300.0,Some(" failed ")).unwrap().unwrap();assert_eq!(finished.runs[0].status,"error");for i in 0..(AUTOMATION_MAX_RUN_HISTORY+5){store.begin_run(&record.id,"schedule",2000.0+i as f64,None,Some(&format!("r-{i}")),None).unwrap();}assert_eq!(store.read_runs(&record.id).len(),AUTOMATION_MAX_RUN_HISTORY);assert!(store.remove(&second.id).unwrap());let _=fs::remove_dir_all(root);}
#[test]fn config_and_notice_contract(){let raw=r#"{"name":" Test ","prompt":" go ","schedule":"@daily","enabled":true,"createdAt":9999,"raisedNotices":[]}"#;let config=parse_stored_config(raw,5000.0).unwrap();assert_eq!(config.created_at,5000.0);assert_eq!(config.name,"Test");let trigger=json!({"type":"github","repo":"org/repo","events":["pr-opened"]});let notices=routine_notice_ids_to_raise(1_700_000_000_000.0,&trigger,&[]);assert_eq!(notices,vec![GITHUB_LISTENER_SCOPE.to_string()]);assert!(routine_notice_ids_to_raise(1_700_000_000_000.0,&trigger,&notices).is_empty());}


#[test]
fn external_and_internal_store_changes_share_the_debounced_watcher(){
 let root=root("watch");let agent=root.join("agent");let dir=agent.join("automations");fs::create_dir_all(&agent).unwrap();
 let store=FileAutomationStore::new(&dir);let(tx,rx)=mpsc::channel();
 store.set_on_change(Some(Arc::new(move||{let _=tx.send(());})));
 let spec=AutomationSpec{name:"Watcher".into(),prompt:"watch".into(),trigger:json!({"type":"cron","schedule":"  0   9  * *  1-5 "}),is_enabled:None};
 let record=store.upsert(&spec,1000.0).unwrap().unwrap();
 rx.recv_timeout(Duration::from_secs(3)).expect("internal atomic write notification");
 assert_eq!(store.read_config(&record.id).unwrap().trigger["schedule"],"0 9 * * 1-5");
 while rx.try_recv().is_ok(){}
 let path=store.config_path(&record.id);let mut value:serde_json::Value=serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
 value["prompt"]=json!("externally edited");fs::write(&path,format!("{}\n",serde_json::to_string_pretty(&value).unwrap())).unwrap();
 rx.recv_timeout(Duration::from_secs(3)).expect("external edit notification");
 assert_eq!(store.get(&record.id).unwrap().prompt,"externally edited");
 store.set_on_change(None);let _=fs::remove_dir_all(root);
}

#[test]
fn definition_run_mutations_suppress_next_run_derivation(){
 let root=root("definition-run");let dir=root.join("automations");let store=FileAutomationStore::new(&dir);
 let spec=AutomationSpec{name:"Definition".into(),prompt:"check".into(),trigger:json!({"type":"cron","schedule":"@hourly"}),is_enabled:None};
 let record=store.upsert(&spec,1_000.0).unwrap().unwrap();
 let definition=store.record_run_definition(&record.id,2_000.0).unwrap().unwrap();
 assert!(definition.next_run_at.is_none());
 let run=store.begin_run(&record.id,"manual",2_100.0,None,Some("r"),None).unwrap().unwrap();
 let definition=store.finish_run_definition(&record.id,&run.id,"ok",2_200.0,None).unwrap().unwrap();
 assert!(definition.next_run_at.is_none());
 assert!(store.get(&record.id).unwrap().next_run_at.is_some());
 let _=fs::remove_dir_all(root);
}
