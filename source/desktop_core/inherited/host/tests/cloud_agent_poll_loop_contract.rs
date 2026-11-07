use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};
use std::time::Duration;

use mahayana_host_runtime::extensions::cloud_agents::cloud_agent_poll_loop::*;
use mahayana_host_runtime::extensions::cloud_agents::cloud_agent_request_composition::{
    SavedEnvironment, SavedEnvironmentRepoConfig,
};
use mahayana_host_runtime::extensions::cloud_agents::model_catalog_fetch::SandModelCatalogEntry;

fn environment(id: &str, name: &str) -> SavedEnvironment {
    SavedEnvironment {
        public_id: id.into(),
        name: name.into(),
        repo_config: Some(SavedEnvironmentRepoConfig { repos: Vec::new() }),
    }
}

struct EnvClient {
    values: Vec<SavedEnvironment>,
}

impl SavedEnvironmentClient for EnvClient {
    fn get_environment(&self, public_id: &str) -> Result<Option<SavedEnvironment>, String> {
        Ok(self.values.iter().find(|value| value.public_id == public_id).cloned())
    }

    fn list_environments(&self, _limit: usize) -> Result<Vec<SavedEnvironment>, String> {
        Ok(self.values.clone())
    }
}

#[test]
fn saved_environment_resolution_and_notes_match_frozen_behavior() {
    let client = EnvClient {
        values: vec![environment("one", "Prod"), environment("two", "Dev")],
    };
    assert_eq!(
        resolve_saved_environment(&client, Some("two"), None).unwrap().name,
        "Dev"
    );
    assert_eq!(
        resolve_saved_environment(&client, None, Some("prod"))
            .unwrap()
            .public_id,
        "one"
    );

    let error = resolve_saved_environment(&client, Some("missing"), None).unwrap_err();
    assert!(error.message.contains("No saved environment with id 'missing'."));
    assert!(error.message.contains("Prod (one)"));

    let empty = available_environments_note(&[]);
    assert!(empty.contains("no saved environments"));
}

#[test]
fn watch_result_preserves_error_limit_diff_and_summary_projection() {
    let detailed = DetailedComposer {
        composer: Some(CloudAgentComposer {
            status: Some(CloudAgentRunStatus::Finished),
            pr_url: Some("https://github.com/acme/repo/pull/7".into()),
            branch_name: Some("agent/work".into()),
            commit_count: Some(2),
            files_changed: Some(3),
            lines_added: Some(10),
            lines_removed: Some(4),
        }),
        prs: Vec::new(),
        summary: Some("Done.".into()),
        permanent_error: None,
    };
    let result = build_watch_result(
        "bc-1",
        CloudAgentRunStatus::Number(2),
        Some(&detailed),
    );
    assert_eq!(result.status, "completed");
    assert!(result.text.contains("Branch: agent/work"));
    assert!(result.text.contains("2 commits, +10/-4 across 3 files"));
    assert!(result.text.contains("Summary from the Cursor agent:\nDone."));

    let failed = DetailedComposer {
        permanent_error: Some(CloudAgentPermanentErrorDetails {
            title: "Limit reached".into(),
            detail: "Upgrade or wait.".into(),
            rate_limit_reason: Some("sand_included_limit".into()),
        }),
        summary: Some("Stopped.".into()),
        ..DetailedComposer::default()
    };
    let result = build_watch_result("bc-2", CloudAgentRunStatus::Expired, Some(&failed));
    assert_eq!(result.status, "error");
    assert!(result.text.contains("expired before finishing"));
    assert!(result.text.contains("Limit reached\nUpgrade or wait."));
}

#[test]
fn rate_limit_jitter_and_model_catalog_ttl_are_deterministic() {
    assert_eq!(cloud_agent_rate_limit_pause_ms(Some(1_000), 0.0), 1_000);
    assert_eq!(cloud_agent_rate_limit_pause_ms(Some(1_000), 1.0), 1_250);
    assert_eq!(
        cloud_agent_rate_limit_pause_ms(None, 0.0),
        CLOUD_AGENT_RATE_LIMIT_FALLBACK_MS
    );

    let now = Arc::new(AtomicU64::new(100));
    let calls = Arc::new(AtomicUsize::new(0));
    let cache = CloudAgentModelCatalogCache::new(
        {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst))
        },
        {
            let calls = Arc::clone(&calls);
            Arc::new(move || {
                let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
                Ok(vec![SandModelCatalogEntry {
                    id: format!("model-{call}"),
                    display_name: None,
                    aliases: Vec::new(),
                    params: Vec::new(),
                    variants: Vec::new(),
                }])
            })
        },
    );
    assert_eq!(cache.list_models().unwrap()[0].id, "model-1");
    assert_eq!(cache.list_models().unwrap()[0].id, "model-1");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    now.store(100 + MODEL_CATALOG_TTL_MS, Ordering::SeqCst);
    assert_eq!(cache.list_models().unwrap()[0].id, "model-2");
}

#[test]
fn team_admin_cache_is_fail_open_and_refreshes_in_background() {
    let now = Arc::new(AtomicU64::new(0));
    let load_count = Arc::new(AtomicUsize::new(0));
    let cache = CloudAgentTeamAdminPolicyCache::new(
        {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst))
        },
        {
            let load_count = Arc::clone(&load_count);
            Arc::new(move || {
                load_count.fetch_add(1, Ordering::SeqCst);
                Ok(true)
            })
        },
    );
    assert!(!cache.is_disabled_by_team_admin());
    cache.wait_for_refresh_for_testing();
    assert!(cache.is_disabled_by_team_admin());
    assert_eq!(load_count.load(Ordering::SeqCst), 1);

    now.store(TEAM_ADMIN_POLICY_TTL_MS, Ordering::SeqCst);
    cache.prefetch_team_admin_policy();
    cache.wait_for_refresh_for_testing();
    assert_eq!(load_count.load(Ordering::SeqCst), 2);
}

#[test]
fn completion_poller_honors_restart_grace_rate_limit_and_terminal_status() {
    let now = Arc::new(AtomicU64::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let responses = Arc::new(Mutex::new(vec![
        Err(CloudAgentPollError {
            message: "rate".into(),
            is_rate_limit: true,
            retry_after_ms: Some(20),
        }),
        Ok(DetailedComposer {
            composer: Some(CloudAgentComposer {
                status: Some(CloudAgentRunStatus::Running),
                ..CloudAgentComposer::default()
            }),
            ..DetailedComposer::default()
        }),
        Ok(DetailedComposer {
            composer: Some(CloudAgentComposer {
                status: Some(CloudAgentRunStatus::Finished),
                pr_url: Some("https://example.com/pr".into()),
                ..CloudAgentComposer::default()
            }),
            ..DetailedComposer::default()
        }),
    ]));

    let poller = CloudAgentCompletionPoller::new(
        {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst))
        },
        {
            let now = Arc::clone(&now);
            Arc::new(move |duration: Duration| {
                now.fetch_add(duration.as_millis() as u64, Ordering::SeqCst);
            })
        },
        {
            let responses = Arc::clone(&responses);
            let calls = Arc::clone(&calls);
            Arc::new(move |_id, timeout| {
                assert_eq!(timeout, CLOUD_AGENT_POLL_RPC_TIMEOUT_MS);
                calls.fetch_add(1, Ordering::SeqCst);
                responses.lock().unwrap().remove(0)
            })
        },
    )
    .with_limits(1_000, 10, 100)
    .with_random(Arc::new(|| 0.0));

    let result = poller.await_completion("bc-1", false);
    assert_eq!(result.status, "completed");
    assert!(result.text.contains("Pull request: https://example.com/pr"));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}
