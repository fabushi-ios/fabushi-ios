//! Group Call Speak Requests Queue: `CallsGroupSpeakRequestsQueue`
//! Manages hand raising, admin approval queues, and speaker priority.

use serde::{Deserialize, Serialize};

/// An individual speak request item in the admin queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakRequest {
    pub user_id: u64,
    pub user_name: String,
    pub timestamp: i64,
    pub is_approved: bool,
}

/// Queue and coordinator for voice chat hand raising and speak permissions.
#[derive(Debug, Clone, Default)]
pub struct CallsGroupSpeakRequestsQueue {
    pub requests: Vec<SpeakRequest>,
}

impl CallsGroupSpeakRequestsQueue {
    pub fn new() -> Self {
        Self { requests: Vec::new() }
    }

    pub fn raise_hand(&mut self, user_id: u64, user_name: &str) {
        if let Some(existing) = self.requests.iter_mut().find(|r| r.user_id == user_id) {
            existing.is_approved = false;
            existing.timestamp = chrono::Utc::now().timestamp();
        } else {
            self.requests.push(SpeakRequest {
                user_id,
                user_name: user_name.to_string(),
                timestamp: chrono::Utc::now().timestamp(),
                is_approved: false,
            });
        }
    }

    pub fn lower_hand(&mut self, user_id: u64) -> bool {
        if let Some(pos) = self.requests.iter().position(|r| r.user_id == user_id) {
            self.requests.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn approve_request(&mut self, user_id: u64) -> bool {
        if let Some(req) = self.requests.iter_mut().find(|r| r.user_id == user_id) {
            req.is_approved = true;
            true
        } else {
            false
        }
    }

    pub fn pending_requests(&self) -> Vec<SpeakRequest> {
        let mut list: Vec<SpeakRequest> = self.requests.iter().filter(|r| !r.is_approved).cloned().collect();
        list.sort_by_key(|r| r.timestamp);
        list
    }

    pub fn count_pending(&self) -> usize {
        self.requests.iter().filter(|r| !r.is_approved).count()
    }
}
