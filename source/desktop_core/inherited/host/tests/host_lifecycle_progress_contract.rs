use std::sync::{Arc,Mutex,atomic::{AtomicUsize,Ordering}};
use mahayana_host_runtime::extensions::telemetry::host_lifecycle_progress::*;
struct D(Arc<AtomicUsize>);
impl Disposable for D { fn dispose(&mut self){self.0.fetch_add(1,Ordering::SeqCst);} }

#[test]
fn lifecycle_progress_reports_completion_and_rearms_watchdog(){
 let now=Arc::new(Mutex::new(1000.0)); let n2=Arc::clone(&now);
 let reports=Arc::new(Mutex::new(Vec::new())); let r2=Arc::clone(&reports);
 let disposed=Arc::new(AtomicUsize::new(0)); let d2=Arc::clone(&disposed);
 let callbacks=Arc::new(Mutex::new(Vec::<Box<dyn Fn()+Send+Sync>>::new())); let c2=Arc::clone(&callbacks);
 let mut p=HostLifecycleProgress::new(
  900.0,
  Arc::new(move||*n2.lock().unwrap()),
  Arc::new(move|r|r2.lock().unwrap().push(r)),
  Arc::new(move|cb|{c2.lock().unwrap().push(cb);Box::new(D(Arc::clone(&d2)))})
 );
 assert_eq!(p.current_phase(),Some("plugin_graph"));
 p.complete(HostLifecycleCompletion{phase:"plugin_graph".into(),plugin_count:Some(3),entry_count:None}).unwrap();
 assert_eq!(p.current_phase(),Some("identity"));
 assert_eq!(disposed.load(Ordering::SeqCst),1);
 assert!(matches!(reports.lock().unwrap()[0],HostLifecycleReport::Completed{duration_ms:100,..}));
}

#[test]
fn lifecycle_progress_fails_wrong_phase_and_watchdog_reports_stuck(){
 let now=Arc::new(Mutex::new(100.0)); let n2=Arc::clone(&now);
 let reports=Arc::new(Mutex::new(Vec::new())); let r2=Arc::clone(&reports);
 let callbacks=Arc::new(Mutex::new(Vec::<Box<dyn Fn()+Send+Sync>>::new())); let c2=Arc::clone(&callbacks);
 let mut p=HostLifecycleProgress::new(
  200.0,
  Arc::new(move||*n2.lock().unwrap()),
  Arc::new(move|r|r2.lock().unwrap().push(r)),
  Arc::new(move|cb|{c2.lock().unwrap().push(cb);Box::new(D(Arc::new(AtomicUsize::new(0))))})
 );
 assert!(p.complete(HostLifecycleCompletion{phase:"identity".into(),plugin_count:None,entry_count:None}).is_err());
 callbacks.lock().unwrap()[0]();
 assert!(matches!(reports.lock().unwrap()[0],HostLifecycleReport::Stuck{duration_ms:0,..}));
 p.fail();
 assert!(matches!(reports.lock().unwrap()[1],HostLifecycleReport::Failed{..}));
}


#[test]
fn production_watchdog_matches_frozen_five_minute_idle_policy() {
    assert_eq!(HOST_LIFECYCLE_STUCK_MS, 5 * 60_000);
}
