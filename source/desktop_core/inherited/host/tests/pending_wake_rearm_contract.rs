use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::transcript::pending_wake_rearm::{
    LostSubagentWake, PendingWakeRearm, PendingWakeReport, PendingWakeRuntimePort,
    PENDING_WAKE_STALE_MAX_AGE_MS,
};
use mahayana_host_runtime::extensions::transcript::sand_pending_wake_store::{
    DurablePendingWakeMarker, PendingWakeKind, QuietWakeOrigin, SandPendingWakeStore,
};
use uuid::Uuid;

#[derive(Default)]
struct FakeRuntime {
    can_execute: Mutex<bool>,
    gone: Mutex<HashSet<String>>,
    group: Mutex<HashSet<String>>,
    armed: Mutex<HashSet<(String, String)>>,
    cloud_watches: Mutex<Vec<(String, String)>>,
    shell_watches: Mutex<Vec<(String, String)>>,
    interrupted: Mutex<Vec<String>>,
    revived: Mutex<Vec<LostSubagentWake>>,
    emitted: Mutex<Vec<String>>,
    reports: Mutex<Vec<PendingWakeReport>>,
}
impl PendingWakeRuntimePort for FakeRuntime {
    fn can_execute(&self) -> bool { *self.can_execute.lock().unwrap() }
    fn is_agent_gone(&self, agent_id: &str) -> bool { self.gone.lock().unwrap().contains(agent_id) }
    fn is_group_session(&self, agent_id: &str) -> Result<bool, String> { Ok(self.group.lock().unwrap().contains(agent_id)) }
    fn cloud_watch_is_armed(&self, agent_id: &str, work_id: &str) -> bool {
        self.armed.lock().unwrap().contains(&(agent_id.into(), work_id.into()))
    }
    fn watch_cloud_agent(&self, agent_id: &str, work_id: &str, _quiet: Option<&QuietWakeOrigin>) -> Result<(), String> {
        self.cloud_watches.lock().unwrap().push((agent_id.into(), work_id.into()));
        self.armed.lock().unwrap().insert((agent_id.into(), work_id.into()));
        Ok(())
    }
    fn watch_background_shell(&self, agent_id: &str, work_id: &str, _title: Option<&str>, _quiet: Option<&QuietWakeOrigin>) -> Result<(), String> {
        self.shell_watches.lock().unwrap().push((agent_id.into(), work_id.into()));
        Ok(())
    }
    fn deliver_recreate_interrupted_shell_notice(&self, marker: &DurablePendingWakeMarker) -> Result<(), String> {
        self.interrupted.lock().unwrap().push(marker.work_id.clone());
        Ok(())
    }
    fn revive_lost_subagent(&self, wake: LostSubagentWake) -> Result<(), String> {
        self.revived.lock().unwrap().push(wake);
        Ok(())
    }
    fn revive_channel_delivery_failure(&self, _marker: &DurablePendingWakeMarker) -> Result<(), String> {
        Ok(())
    }
    fn emit_async_tasks_for_agent(&self, agent_id: &str) { self.emitted.lock().unwrap().push(agent_id.into()); }
    fn report_pending_wake(&self, report: PendingWakeReport) { self.reports.lock().unwrap().push(report); }
}

fn root() -> PathBuf {
    let path=std::env::temp_dir().join(format!("pending-wake-rearm-{}",Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}
fn marker(agent:&str, kind:PendingWakeKind, work:&str, marked:f64) -> DurablePendingWakeMarker {
    DurablePendingWakeMarker {
        agent_id: agent.into(), kind, work_id: work.into(), marked_at_ms: marked,
        quiet_origin: None, title: Some("work".into()), subagent_type: Some("task".into()),
        interrupted_by_recreate: false,
        delivery_operation_id: None,
        address_token: None,
        failure_reason: None,
        session_key: None,
    }
}

#[test]
fn persists_clears_and_disarms_with_reference_telemetry() {
    let dir=root();
    let store=SandPendingWakeStore::new(&dir);
    let runtime=Arc::new(FakeRuntime::default());
    *runtime.can_execute.lock().unwrap()=true;
    let service=PendingWakeRearm::new(Some(store.clone()), runtime.clone());
    assert!(service.persist_pending_wake(marker("a",PendingWakeKind::Shell,"s1",10.0)));
    assert!(store.has_pending("a",PendingWakeKind::Shell,"s1"));
    service.clear_settled_pending_wake("a",PendingWakeKind::Shell,"s1");
    assert!(!store.has_pending("a",PendingWakeKind::Shell,"s1"));
    assert_eq!(runtime.emitted.lock().unwrap().as_slice(), &["a".to_string()]);

    service.persist_pending_wake(marker("a",PendingWakeKind::Shell,"s2",20.0));
    service.disarm_pending_wake("a",PendingWakeKind::Shell,"s2");
    let reports=runtime.reports.lock().unwrap();
    assert!(reports.iter().any(|r| r.outcome=="settled" && r.work_id=="s1" && r.reason.is_none()));
    assert!(reports.iter().any(|r| r.outcome=="settled" && r.work_id=="s2" && r.reason.as_deref()==Some("aborted")));
    drop(reports);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rearm_prunes_stale_preserves_gone_and_restores_each_runtime_kind() {
    let dir=root();
    let store=SandPendingWakeStore::new(&dir);
    let runtime=Arc::new(FakeRuntime::default());
    *runtime.can_execute.lock().unwrap()=true;
    runtime.gone.lock().unwrap().insert("gone".into());

    store.mark_pending(marker("stale",PendingWakeKind::Shell,"old",0.0));
    store.mark_pending(marker("gone",PendingWakeKind::Shell,"held",PENDING_WAKE_STALE_MAX_AGE_MS+10.0));
    store.mark_pending(marker("cloud",PendingWakeKind::CloudAgent,"bc1",PENDING_WAKE_STALE_MAX_AGE_MS+20.0));
    store.mark_pending(marker("shell",PendingWakeKind::Shell,"sh1",PENDING_WAKE_STALE_MAX_AGE_MS+30.0));
    store.mark_pending(marker("parent",PendingWakeKind::Subagent,"sub1",PENDING_WAKE_STALE_MAX_AGE_MS+40.0));

    let now=PENDING_WAKE_STALE_MAX_AGE_MS+100.0;
    let service=PendingWakeRearm::new(Some(store.clone()),runtime.clone()).with_now(Arc::new(move||now));
    service.rearm_pending_wakes();

    assert!(!store.has_pending("stale",PendingWakeKind::Shell,"old"));
    assert!(store.has_pending("gone",PendingWakeKind::Shell,"held"));
    assert_eq!(runtime.cloud_watches.lock().unwrap().as_slice(), &[("cloud".into(),"bc1".into())]);
    assert_eq!(runtime.shell_watches.lock().unwrap().as_slice(), &[("shell".into(),"sh1".into())]);
    assert_eq!(runtime.revived.lock().unwrap().len(),1);
    assert!(store.has_pending("parent",PendingWakeKind::Subagent,"sub1"));

    let reports=runtime.reports.lock().unwrap();
    assert!(reports.iter().any(|r| r.outcome=="pruned" && r.work_id=="old" && r.reason.as_deref()==Some("stale")));
    assert!(reports.iter().any(|r| r.outcome=="rearm_skipped" && r.work_id=="held" && r.reason.as_deref()==Some("agent_gone")));
    assert!(reports.iter().any(|r| r.outcome=="rearmed" && r.work_id=="sub1" && r.reason.as_deref()==Some("interrupted_completion")));
    drop(reports);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn interrupted_recreate_shell_uses_resume_notice_and_group_session_clears_marker() {
    let dir=root();
    let store=SandPendingWakeStore::new(&dir);
    let runtime=Arc::new(FakeRuntime::default());
    *runtime.can_execute.lock().unwrap()=true;

    let mut direct=marker("a",PendingWakeKind::Shell,"resume",100.0);
    direct.interrupted_by_recreate=true;
    store.mark_pending(direct);
    let service=PendingWakeRearm::new(Some(store.clone()),runtime.clone()).with_now(Arc::new(||200.0));
    service.rearm_pending_wakes();
    assert_eq!(runtime.interrupted.lock().unwrap().as_slice(), &["resume".to_string()]);

    let mut grouped=marker("g",PendingWakeKind::Shell,"grouped",100.0);
    grouped.interrupted_by_recreate=true;
    store.mark_pending(grouped);
    runtime.group.lock().unwrap().insert("g".into());
    service.rearm_pending_wakes();
    assert!(!store.has_pending("g",PendingWakeKind::Shell,"grouped"));
    assert!(runtime.reports.lock().unwrap().iter().any(|r| r.work_id=="grouped" && r.reason.as_deref()==Some("group_session")));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn enqueue_refuses_gone_agents_and_appends_for_live_agents() {
    let runtime=Arc::new(FakeRuntime::default());
    runtime.gone.lock().unwrap().insert("gone".into());
    let service=PendingWakeRearm::new(None,runtime);
    let mut q:HashMap<String,Vec<i32>>=HashMap::new();
    assert!(!service.enqueue_pending_wake(&mut q,"gone",vec![1]));
    assert!(service.enqueue_pending_wake(&mut q,"live",vec![1,2]));
    assert!(service.enqueue_pending_wake(&mut q,"live",vec![3]));
    assert_eq!(q["live"],vec![1,2,3]);
}

#[test]
fn recreate_carry_filters_before_persisting_and_reports_frozen_outcomes() {
    let dir = root();
    let store = SandPendingWakeStore::new(&dir);
    let runtime = Arc::new(FakeRuntime::default());
    let service = PendingWakeRearm::new(Some(store.clone()), runtime.clone())
        .with_now(Arc::new(|| 500.0));

    let live_cloud = marker("cloud", PendingWakeKind::CloudAgent, "cloud-1", 100.0);
    let live_shell = marker("shell", PendingWakeKind::Shell, "shell-1", 110.0);
    let local = marker("local", PendingWakeKind::CloudAgent, "local-1", 120.0);
    let gone = marker("gone", PendingWakeKind::CloudAgent, "gone-1", 130.0);
    let group = marker("group", PendingWakeKind::CloudAgent, "group-1", 140.0);
    let subagent = marker("parent", PendingWakeKind::Subagent, "sub-1", 150.0);
    store.mark_pending(local.clone());
    runtime.gone.lock().unwrap().insert("gone".into());
    runtime.group.lock().unwrap().insert("group".into());

    let carried = vec![
        serde_json::to_value(&live_cloud).unwrap(),
        serde_json::to_value(&live_shell).unwrap(),
        serde_json::to_value(&local).unwrap(),
        serde_json::to_value(&gone).unwrap(),
        serde_json::to_value(&group).unwrap(),
        serde_json::to_value(&subagent).unwrap(),
    ];

    assert_eq!(service.restore_recreate_carried_pending_wakes(&carried), 0);
    assert!(!store.has_pending("cloud", PendingWakeKind::CloudAgent, "cloud-1"));
    *runtime.can_execute.lock().unwrap() = true;

    assert_eq!(service.restore_recreate_carried_pending_wakes(&carried), 2);
    assert!(store.has_pending("cloud", PendingWakeKind::CloudAgent, "cloud-1"));
    assert!(store.has_pending("shell", PendingWakeKind::Shell, "shell-1"));
    assert!(!store.has_pending("gone", PendingWakeKind::CloudAgent, "gone-1"));
    assert!(!store.has_pending("group", PendingWakeKind::CloudAgent, "group-1"));
    assert!(!store.has_pending("parent", PendingWakeKind::Subagent, "sub-1"));
    assert_eq!(runtime.cloud_watches.lock().unwrap().as_slice(), &[("cloud".into(), "cloud-1".into())]);
    assert_eq!(runtime.interrupted.lock().unwrap().as_slice(), &["shell-1".to_string()]);

    let reports = runtime.reports.lock().unwrap();
    assert!(reports.iter().any(|r| r.work_id == "local-1" && r.outcome == "rearm_skipped" && r.reason.as_deref() == Some("locally_owned")));
    assert!(reports.iter().any(|r| r.work_id == "gone-1" && r.outcome == "rearm_skipped" && r.reason.as_deref() == Some("agent_gone")));
    assert!(reports.iter().any(|r| r.work_id == "group-1" && r.outcome == "rearm_skipped" && r.reason.as_deref() == Some("group_session")));
    assert!(reports.iter().any(|r| r.work_id == "cloud-1" && r.outcome == "carried"));
    assert!(reports.iter().any(|r| r.work_id == "cloud-1" && r.outcome == "rearmed" && r.reason.as_deref() == Some("recreate_carry")));
    assert!(reports.iter().any(|r| r.work_id == "shell-1" && r.outcome == "carried"));
    assert!(reports.iter().any(|r| r.work_id == "shell-1" && r.outcome == "dropped_with_notice"));
    drop(reports);
    std::fs::remove_dir_all(dir).unwrap();
}
