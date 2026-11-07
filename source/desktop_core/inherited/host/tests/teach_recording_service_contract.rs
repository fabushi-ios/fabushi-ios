use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use mahayana_host_runtime::extensions::teach_recording::extension::TeachRecordingServiceDeps;
use mahayana_host_runtime::extensions::teach_recording::teach_recording_service::{
    SandTeachRecordingService, ShellResult, TeachRecordingRuntimePort, is_fork_window_index,
    learning_prompt_nonce, learning_prompt_rich_text, queue_file_for_session_dir, queue_scope,
    sign_queue_entry,
};
use serde_json::json;

struct FakeRuntime {
    window: Mutex<Option<u32>>,
    ready: Mutex<Result<(), String>>,
    shell_results: Mutex<VecDeque<Result<ShellResult, String>>>,
    commands: Mutex<Vec<(String, String, String)>>,
}

impl FakeRuntime {
    fn forked() -> Self {
        Self {
            window: Mutex::new(Some(2)),
            ready: Mutex::new(Ok(())),
            shell_results: Mutex::new(VecDeque::new()),
            commands: Mutex::new(Vec::new()),
        }
    }
}

impl TeachRecordingRuntimePort for FakeRuntime {
    fn ensure_ready(&self, _agent_id: &str) -> Result<(), String> {
        self.ready.lock().unwrap().clone()
    }

    fn get_agent_window_index(&self, _agent_id: &str) -> Option<u32> {
        *self.window.lock().unwrap()
    }

    fn run_shell(
        &self,
        agent_id: &str,
        command: &str,
        tool_call_id: &str,
    ) -> Result<ShellResult, String> {
        self.commands.lock().unwrap().push((
            agent_id.to_string(),
            tool_call_id.to_string(),
            command.to_string(),
        ));
        self.shell_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(ShellResult::success()))
    }
}

fn deps(
    prompts: Arc<Mutex<Vec<(String, String, String, Option<String>)>>>,
    stopped: Arc<Mutex<Vec<serde_json::Value>>>,
) -> TeachRecordingServiceDeps {
    TeachRecordingServiceDeps {
        is_enabled: Arc::new(|| true),
        cap_delay_ms: 60_000,
        send_learning_prompt: Arc::new(move |agent, content, nonce, rich_text| {
            prompts.lock().unwrap().push((
                agent.to_string(),
                content.to_string(),
                nonce.to_string(),
                rich_text.map(str::to_string),
            ));
            Ok(())
        }),
        list_agent_ids: Arc::new(|| Ok(vec!["agent-1".into()])),
        queue_signature_key: Arc::new(|| Ok([7; 32])),
        ensure_learning_workflow: Arc::new(|| Ok(true)),
        track_recording_started: Arc::new(|_| {}),
        track_recording_stopped: Arc::new(move |event| stopped.lock().unwrap().push(event)),
        report_cap_stop_failed: Arc::new(|_| {}),
        report_start_failed: Arc::new(|_| {}),
    }
}

#[test]
fn frozen_helpers_keep_scope_signature_nonce_and_rich_text_contracts() {
    assert!(is_fork_window_index(Some(2)));
    assert!(!is_fork_window_index(Some(1)));
    assert!(!is_fork_window_index(None));

    assert_eq!(
        queue_scope("agent-1"),
        "6ff3b3bd11c44cac620c43d5b65377bd2ba7e8951c1e835ae40c96733730982b"
    );
    assert_eq!(
        sign_queue_entry(&[7; 32], "agent-1", "teach-20260927T010203Z-00000000-0000-4000-8000-000000000000.json"),
        "824e3de359912a177b295ce84a81aa3a4e0e468d2b4542f82b174ffc763e47c5"
    );

    let recording = mahayana_host_runtime::extensions::teach_recording::teach_recording_service::Recording {
        agent_id: "agent-1".into(),
        started_at_ms: 1,
        session_dir: "/workspace/teach-sessions/teach-20260927T010203Z-00000000-0000-4000-8000-000000000000".into(),
        display_label: ":2".into(),
    };
    assert_eq!(
        queue_file_for_session_dir(&recording.session_dir),
        "teach-20260927T010203Z-00000000-0000-4000-8000-000000000000.json"
    );
    assert!(learning_prompt_nonce(&recording).starts_with("teach-recording:6ff3b3bd"));
    let rich: serde_json::Value = serde_json::from_str(&learning_prompt_rich_text("agent-1")).unwrap();
    assert_eq!(rich["content"][0]["content"][0]["type"], "workflowReference");
    assert_eq!(
        rich["content"][0]["content"][0]["attrs"]["id"],
        "learn-from-demonstration"
    );
}

#[test]
fn start_requires_private_monitor_and_builds_ffmpeg_x11grab_command() {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let stopped = Arc::new(Mutex::new(Vec::new()));
    let runtime = Arc::new(FakeRuntime::forked());
    *runtime.window.lock().unwrap() = Some(1);
    let service = SandTeachRecordingService::new(deps(prompts, stopped), runtime.clone());

    let error = service.start("agent-1", Some("composer")).unwrap_err();
    assert_eq!(error.kind, "no_monitor");
    assert!(runtime.commands.lock().unwrap().is_empty());

    *runtime.window.lock().unwrap() = Some(2);
    let status = service.start("agent-1", Some("composer")).unwrap();
    assert_eq!(status.state, "recording");
    assert_eq!(status.agent_id.as_deref(), Some("agent-1"));
    let commands = runtime.commands.lock().unwrap();
    let start = commands.last().unwrap();
    assert_eq!(start.1, "sand-teach-recording-start");
    assert!(start.2.contains("ffmpeg -y -v error -f x11grab"));
    assert!(start.2.contains("-video_size 1280x800"));
    assert!(start.2.contains("-i :2.0"));
    assert!(start.2.contains("ffmpeg.pid"));
    assert!(start.2.contains("session.json"));
}

#[test]
fn saved_stop_persists_signed_queue_sends_learning_prompt_and_marks_delivery() {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let stopped = Arc::new(Mutex::new(Vec::new()));
    let runtime = Arc::new(FakeRuntime::forked());
    let service =
        SandTeachRecordingService::new(deps(prompts.clone(), stopped.clone()), runtime.clone());

    service.start("agent-1", None).unwrap();
    let status = service.stop("agent-1", true).unwrap();
    assert_eq!(status.state, "idle");

    let commands = runtime.commands.lock().unwrap();
    assert!(commands.iter().any(|(_, id, command)| {
        id == "sand-teach-recording-stop"
            && command.contains("COMPLETED_SESSION_JSON")
            && command.contains("\"signature\"")
    }));
    assert!(commands.iter().any(|(_, id, command)| {
        id == "sand-teach-recording-verify" && command.contains("/queues/")
    }));
    assert!(commands.iter().any(|(_, id, command)| {
        id == "sand-teach-recording-mark-prompt-delivered"
            && command.contains("prompt-delivered")
    }));
    drop(commands);

    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].0, "agent-1");
    assert_eq!(prompts[0].1, "The recording is finished. Learn the task from it.");
    assert!(prompts[0].2.starts_with("teach-recording:"));
    assert!(prompts[0].3.as_ref().unwrap().contains("workflowReference"));

    let stopped = stopped.lock().unwrap();
    assert_eq!(stopped.len(), 1);
    assert_eq!(stopped[0]["outcome"], json!("saved"));
}

#[test]
fn discard_kills_only_owned_ffmpeg_and_removes_pending_queue() {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let stopped = Arc::new(Mutex::new(Vec::new()));
    let runtime = Arc::new(FakeRuntime::forked());
    let service =
        SandTeachRecordingService::new(deps(prompts.clone(), stopped.clone()), runtime.clone());

    service.start("agent-1", None).unwrap();
    service.stop("agent-1", false).unwrap();

    let commands = runtime.commands.lock().unwrap();
    let discard = commands
        .iter()
        .find(|(_, id, _)| id == "sand-teach-recording-discard")
        .expect("discard command");
    assert!(discard.2.contains("/proc/\"$pid\"/cmdline"));
    assert!(discard.2.contains("kill -INT"));
    assert!(discard.2.contains("kill -KILL"));
    assert!(discard.2.contains("rm -f \"$queue_file\""));
    assert!(discard.2.contains("rm -rf /workspace/teach-sessions/teach-"));
    assert!(prompts.lock().unwrap().is_empty());
    assert_eq!(stopped.lock().unwrap()[0]["outcome"], json!("discarded"));
}

#[test]
fn recovery_quarantines_forged_entries_and_redelivers_authentic_undelivered_queue() {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let stopped = Arc::new(Mutex::new(Vec::new()));
    let runtime = Arc::new(FakeRuntime::forked());
    let key = [7; 32];
    let scope = queue_scope("agent-1");
    let file = "teach-20260927T010203Z-00000000-0000-4000-8000-000000000000.json";
    let signature = sign_queue_entry(&key, "agent-1", file);
    let authentic = BASE64_STANDARD.encode(
        serde_json::to_vec(&json!({
            "agentId": "agent-1",
            "sessionDir": "/workspace/teach-sessions/teach-20260927T010203Z-00000000-0000-4000-8000-000000000000",
            "clientNonce": format!("teach-recording:{scope}:{file}"),
            "signature": signature,
        }))
        .unwrap(),
    );
    let forged = BASE64_STANDARD.encode(
        serde_json::to_vec(&json!({
            "agentId": "agent-1",
            "sessionDir": "/workspace/teach-sessions/teach-20260927T010204Z-00000000-0000-4000-8000-000000000001",
            "clientNonce": "wrong",
            "signature": "00".repeat(32),
        }))
        .unwrap(),
    );
    runtime.shell_results.lock().unwrap().push_back(Ok(ShellResult {
        exit_code: 0,
        stdout: format!(
            "{scope}\t{file}\t0\t{authentic}\n{scope}\tteach-20260927T010204Z-00000000-0000-4000-8000-000000000001.json\t0\t{forged}\n"
        ),
        stderr: String::new(),
    }));

    let service = SandTeachRecordingService::new(deps(prompts.clone(), stopped), runtime.clone());
    service.recover_pending().unwrap();

    let commands = runtime.commands.lock().unwrap();
    assert!(commands.iter().any(|(_, id, command)| {
        id == "sand-teach-recording-quarantine" && command.contains("/rejected/")
    }));
    assert!(commands.iter().any(|(_, id, _)| {
        id == "sand-teach-recording-mark-prompt-delivered"
    }));
    assert_eq!(prompts.lock().unwrap().len(), 1);
}
