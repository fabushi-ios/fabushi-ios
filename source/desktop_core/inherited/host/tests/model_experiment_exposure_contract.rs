use std::collections::BTreeMap;
use std::sync::{Arc,Mutex,atomic::{AtomicBool,AtomicUsize,Ordering}};
use mahayana_host_runtime::extensions::telemetry::model_experiment_exposure::*;

struct Exp{
 hydrated:AtomicBool,wait_result:AtomicBool,sdk:AtomicBool,
 state:Mutex<Option<SandModelExperimentState>>,waits:AtomicUsize,logs:AtomicUsize
}
impl ModelExperimentExposureExperiments for Exp{
 fn has_hydrated_statsig_user_id(&self)->bool{self.hydrated.load(Ordering::SeqCst)}
 fn wait_for_hydrated_statsig_user_id(&self,t:u64)->bool{assert_eq!(t,HYDRATION_WAIT_MS);self.waits.fetch_add(1,Ordering::SeqCst);self.wait_result.load(Ordering::SeqCst)}
 fn get_sand_model_experiment_state(&self)->Option<SandModelExperimentState>{self.state.lock().unwrap().clone()}
 fn log_sand_model_experiment_exposure(&self)->bool{self.logs.fetch_add(1,Ordering::SeqCst);self.sdk.load(Ordering::SeqCst)}
}
#[derive(Default)]
struct Analytics{allowed:AtomicBool,events:Mutex<Vec<(String,String)>>}
impl ModelExperimentExposureAnalytics for Analytics{
 fn can_record_events(&self)->bool{self.allowed.load(Ordering::SeqCst)}
 fn track_event(&self,n:&str,a:&str){self.events.lock().unwrap().push((n.into(),a.into()))}
}
#[test]
fn env_override_skips_hydration_and_logs_once(){
 let exp=Arc::new(Exp{hydrated:AtomicBool::new(false),wait_result:AtomicBool::new(false),sdk:AtomicBool::new(false),state:Mutex::new(None),waits:AtomicUsize::new(0),logs:AtomicUsize::new(0)});
 let analytics=Arc::new(Analytics::default()); analytics.allowed.store(true,Ordering::SeqCst);
 let env=BTreeMap::from([(SAND_MODEL_EXPERIMENT_OVERRIDE.into(),"test".into())]);
 let latch=ModelExperimentExposureLatch::new(exp.clone(),analytics.clone(),env);
 latch.note(); latch.note();
 assert!(latch.is_logged());
 assert_eq!(exp.waits.load(Ordering::SeqCst),0);
 assert_eq!(analytics.events.lock().unwrap().as_slice(),&[("sand.model_experiment.exposure".into(),"treatment".into())]);
}
#[test]
fn without_override_waits_for_hydration_and_requires_sdk_exposure(){
 let exp=Arc::new(Exp{hydrated:AtomicBool::new(false),wait_result:AtomicBool::new(true),sdk:AtomicBool::new(false),state:Mutex::new(Some(SandModelExperimentState{active:true,arm:"control".into()})),waits:AtomicUsize::new(0),logs:AtomicUsize::new(0)});
 let analytics=Arc::new(Analytics::default()); analytics.allowed.store(true,Ordering::SeqCst);
 let latch=ModelExperimentExposureLatch::new(exp.clone(),analytics.clone(),BTreeMap::new());
 latch.note();
 assert!(!latch.is_logged());
 assert_eq!(exp.waits.load(Ordering::SeqCst),1);
 assert!(analytics.events.lock().unwrap().is_empty());
}
#[test]
fn override_parser_matches_frozen_aliases(){
 assert_eq!(read_sand_model_experiment_env_override(&BTreeMap::from([(SAND_MODEL_EXPERIMENT_OVERRIDE.into()," control ".into())])).unwrap().arm,"control");
 assert_eq!(read_sand_model_experiment_env_override(&BTreeMap::from([(SAND_MODEL_EXPERIMENT_OVERRIDE.into(),"TEST".into())])).unwrap().arm,"treatment");
 assert!(read_sand_model_experiment_env_override(&BTreeMap::new()).is_none());
}
