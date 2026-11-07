use std::collections::BTreeMap;
use std::fs;
use std::sync::{Arc,Mutex,atomic::{AtomicU64,Ordering}};
use std::time::{Duration,SystemTime,UNIX_EPOCH};
use mahayana_host_runtime::extensions::action_audit::action_audit_service::*;
fn dir()->std::path::PathBuf{let n=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();std::env::temp_dir().join(format!("fabushi-audit-{}-{n}",std::process::id()))}
fn shell()->AuditRecord{AuditRecord{occurred_at_ms:1000,agent_id:"a".into(),turn_id:Some("t".into()),box_id:None,action:AuditAction::ShellCommand{command:"ls".into(),shell_kind:"host".into(),target:"/".into(),allowed:None,blocked_reason:None,classification_reasons:vec![]}}}
#[test]
fn local_projection_and_backend_filter_match_frozen_service(){
 let line=local_audit_jsonl_line(&shell(),"e1");assert!(line.contains("\"type\":\"shell_command\""));assert!(line.ends_with('\n'));
 let mut m=shell();m.action=AuditAction::McpToolCall{tool_call_id:"c".into(),server_identifier:"s".into(),server_name:None,tool_name:"x".into(),transport:"http".into(),status:"ok".into(),duration_ms:1.0};assert!(!is_backend_forwardable(&m));
 if let AuditAction::McpToolCall{transport,..}=&mut m.action{*transport="stdio".into()}assert!(is_backend_forwardable(&m));
}
#[test]
fn flush_batches_and_persists_failure_then_recovers(){
 let root=dir();let sent=Arc::new(Mutex::new(Vec::<usize>::new()));let s2=Arc::clone(&sent);let fail=Arc::new(Mutex::new(true));let f2=Arc::clone(&fail);let now=Arc::new(AtomicU64::new(10));let n2=Arc::clone(&now);
 let a=SandActionAuditor::new(root.join("out.json"),Arc::new({let r=root.clone();move|id|r.join(id).join("audit.jsonl")}),Arc::new(||true),Arc::new(move|batch|{s2.lock().unwrap().push(batch.len());if *f2.lock().unwrap(){Err(AuditSendError{message:"x".into(),retry_after_ms:Some(45_000)})}else{Ok(())}}),Duration::from_secs(60)).with_clock_and_id(Arc::new(move||n2.load(Ordering::SeqCst)),Arc::new(||uuid::Uuid::new_v4().to_string()));
 for _ in 0..55{a.record(shell())}a.flush();assert_eq!(a.pending_len(),55);assert!(root.join("out.json").exists());
 *fail.lock().unwrap()=false;now.store(44_999,Ordering::SeqCst);a.flush();assert_eq!(a.pending_len(),55);now.store(45_011,Ordering::SeqCst);a.flush();assert_eq!(a.pending_len(),0);assert_eq!(sent.lock().unwrap().last().copied(),Some(5));a.dispose();let _=fs::remove_dir_all(root);
}
