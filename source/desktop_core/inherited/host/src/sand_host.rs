use std::collections::HashSet;

pub const BOX_READY_STAGE_MARKER_PATH: &str = "/tmp/sand-box-ready-stage";
pub const BOX_READY_REPORT_ATTEMPTS: usize = 3;
pub const BOX_READY_REPORT_RETRY_MS: u64 = 30_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandHostHealth {
    pub is_busy: bool,
    pub busy_only_awaiting_approval: bool,
    pub active_agent_id: Option<String>,
    pub last_busy_at_ms: u64,
}

pub fn compute_host_health(
    running_agent_ids: impl IntoIterator<Item = String>,
    awaiting_approval_agent_ids: impl IntoIterator<Item = String>,
    has_other_background_work: bool,
    active_agent_id: Option<String>,
    now_ms: u64,
    last_busy_at_ms: u64,
) -> SandHostHealth {
    let running = running_agent_ids.into_iter().collect::<HashSet<_>>();
    let awaiting = awaiting_approval_agent_ids
        .into_iter()
        .collect::<HashSet<_>>();
    let running_turns_busy = !running.is_empty();
    let is_busy = running_turns_busy || has_other_background_work;
    let busy_only_awaiting_approval = running_turns_busy
        && !has_other_background_work
        && running.iter().all(|agent_id| awaiting.contains(agent_id));
    let next_last_busy = if is_busy && !busy_only_awaiting_approval {
        now_ms
    } else {
        last_busy_at_ms
    };
    SandHostHealth {
        is_busy,
        busy_only_awaiting_approval,
        active_agent_id,
        last_busy_at_ms: next_last_busy,
    }
}

pub fn should_report_box_ready(
    boot_id: Option<&str>,
    boot_started_at_ms: Option<u64>,
    marker: Option<&str>,
) -> bool {
    let Some(boot_id) = boot_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    if boot_started_at_ms.is_none() {
        return false;
    }
    marker != Some(boot_id)
}

pub fn box_ready_duration_ms(now_ms: u64, boot_started_at_ms: u64) -> u64 {
    now_ms.saturating_sub(boot_started_at_ms)
}
