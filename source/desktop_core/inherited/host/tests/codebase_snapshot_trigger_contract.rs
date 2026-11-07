use std::sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}};
use std::thread;
use std::time::Duration;
use mahayana_host_runtime::extensions::codebase_telemetry::codebase_snapshot_trigger::{
    CodebaseSnapshotTrigger, SnapshotReason, SnapshotReasonType, snapshot_reason_key,
};

#[test]
fn snapshot_trigger_dedupes_and_retries_failed_reason() {
    let calls=Arc::new(Mutex::new(Vec::<String>::new()));
    let fail=Arc::new(AtomicUsize::new(1));
    let calls2=Arc::clone(&calls); let fail2=Arc::clone(&fail);
    let session=Arc::new(move |r:SnapshotReason| {
        calls2.lock().unwrap().push(r.request_id.clone());
        if fail2.fetch_sub(1,Ordering::SeqCst)==1 { Err("boom".into()) } else { Ok(()) }
    });
    let s2=Arc::clone(&session);
    let warnings=Arc::new(Mutex::new(Vec::<String>::new())); let w2=Arc::clone(&warnings);
    let trigger=CodebaseSnapshotTrigger::new(
        Arc::new(move || Some(s2.clone())),
        Arc::new(|_|{}),
        Arc::new(move |m,_| w2.lock().unwrap().push(m.into())),
    );
    let reason=SnapshotReason{reason_type:SnapshotReasonType::AgentRequestStart,request_id:"req-1".into()};
    assert_eq!(snapshot_reason_key(&reason),"[\"AGENT_REQUEST_START\",\"req-1\"]");
    trigger.handle(reason.clone()); thread::sleep(Duration::from_millis(30));
    trigger.handle(reason.clone()); thread::sleep(Duration::from_millis(30));
    trigger.handle(reason); thread::sleep(Duration::from_millis(30));
    assert_eq!(calls.lock().unwrap().as_slice(),&["req-1","req-1"]);
    assert_eq!(warnings.lock().unwrap().len(),1);
}

#[test]
fn snapshot_trigger_drops_without_session() {
    let debug=Arc::new(Mutex::new(Vec::<String>::new())); let d2=Arc::clone(&debug);
    let trigger=CodebaseSnapshotTrigger::new(Arc::new(||None),Arc::new(move |m|d2.lock().unwrap().push(m.into())),Arc::new(|_,_|{}));
    trigger.handle(SnapshotReason{reason_type:SnapshotReasonType::AgentRequestEnd,request_id:"req".into()});
    assert_eq!(debug.lock().unwrap().len(),1);
}
