//! Target Symbol: DownloadManagerMtproto
//! Telegram Desktop 1:1 Submodule: `DownloadManagerMtproto`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::collections::{HashMap, VecDeque};

pub const DOWNLOAD_PART_SIZE: usize = 128 * 1024;

#[derive(Debug, Clone, Default)]
pub struct DcSessionBalanceData {
    pub requested: usize,
    pub successes: usize,
    pub max_waited_amount: usize,
}

#[derive(Debug, Clone, Default)]
pub struct DcBalanceData {
    pub sessions: Vec<DcSessionBalanceData>,
    pub timeouts: usize,
    pub total_requested: usize,
}

#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub id: u64,
    pub dc_id: i32,
    pub priority: i32,
    pub offset: i64,
    pub full_size: i64,
}

#[derive(Debug, Default)]
pub struct DownloadManagerMtproto {
    dc_balances: HashMap<i32, DcBalanceData>,
    queues: HashMap<i32, VecDeque<DownloadTask>>,
    next_task_id: u64,
}

impl DownloadManagerMtproto {
    pub fn new() -> Self {
        Self {
            dc_balances: HashMap::new(),
            queues: HashMap::new(),
            next_task_id: 1,
        }
    }

    pub fn enqueue(&mut self, dc_id: i32, priority: i32, full_size: i64) -> u64 {
        let id = self.next_task_id;
        self.next_task_id += 1;

        let task = DownloadTask {
            id,
            dc_id,
            priority,
            offset: 0,
            full_size,
        };

        self.queues.entry(dc_id).or_default().push_back(task);
        self.dc_balances.entry(dc_id).or_insert_with(|| DcBalanceData {
            sessions: vec![DcSessionBalanceData::default()],
            timeouts: 0,
            total_requested: 0,
        });

        id
    }

    pub fn remove(&mut self, task_id: u64) {
        for queue in self.queues.values_mut() {
            queue.retain(|t| t.id != task_id);
        }
    }

    pub fn take_next_task(&mut self, dc_id: i32) -> Option<DownloadTask> {
        let queue = self.queues.get_mut(&dc_id)?;
        queue.pop_front()
    }

    pub fn request_succeeded(&mut self, dc_id: i32, session_index: usize) {
        if let Some(balance) = self.dc_balances.get_mut(&dc_id) {
            if session_index < balance.sessions.len() {
                balance.sessions[session_index].successes += 1;
                balance.sessions[session_index].requested = balance.sessions[session_index].requested.saturating_sub(1);
            }
            balance.total_requested = balance.total_requested.saturating_sub(1);
        }
    }

    pub fn session_timed_out(&mut self, dc_id: i32, session_index: usize) {
        if let Some(balance) = self.dc_balances.get_mut(&dc_id) {
            balance.timeouts += 1;
            if session_index < balance.sessions.len() {
                balance.sessions[session_index].requested = balance.sessions[session_index].requested.saturating_sub(1);
            }
            balance.total_requested = balance.total_requested.saturating_sub(1);
        }
    }
}
