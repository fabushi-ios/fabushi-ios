//! Calls Dialog Box & History List Controller
//! Upstream: Telegram/SourceFiles/calls/calls_box_controller.h:L20-L88, calls_box_controller.cpp:L55-L150
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRecord {
    pub message_id: i64,
    pub peer_id: u64,
    pub date: i64,
    pub duration_seconds: i32,
    pub is_video: bool,
    pub is_outgoing: bool,
    pub is_missed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallsBoxController {
    pub records: Vec<CallRecord>,
    pub offset_id: i64,
    pub all_loaded: bool,
    pub is_loading: bool,
    pub total_count: usize,
    pub active_group_calls: Vec<u64>,
}

impl CallsBoxController {
    pub const K_FIRST_PAGE_COUNT: usize = 20;
    pub const K_PER_PAGE_COUNT: usize = 100;

    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            offset_id: 0,
            all_loaded: false,
            is_loading: false,
            total_count: 0,
            active_group_calls: Vec::new(),
        }
    }

    pub fn receive_calls_page(&mut self, new_records: Vec<CallRecord>) {
        if new_records.is_empty() {
            self.all_loaded = true;
        } else {
            if let Some(last) = new_records.last() {
                self.offset_id = last.message_id;
            }
            if new_records.len() < Self::K_PER_PAGE_COUNT {
                self.all_loaded = true;
            }
            self.records.extend(new_records);
            self.total_count = self.records.len();
        }
        self.is_loading = false;
    }

    pub fn delete_call_record(&mut self, message_id: i64) -> bool {
        let initial_len = self.records.len();
        self.records.retain(|r| r.message_id != message_id);
        self.total_count = self.records.len();
        self.records.len() < initial_len
    }

    pub fn clear_all_calls(&mut self) {
        self.records.clear();
        self.offset_id = 0;
        self.all_loaded = true;
        self.total_count = 0;
    }

    pub fn add_active_group_call(&mut self, peer_id: u64) {
        if !self.active_group_calls.contains(&peer_id) {
            self.active_group_calls.push(peer_id);
        }
    }

    pub fn remove_active_group_call(&mut self, peer_id: u64) {
        self.active_group_calls.retain(|&id| id != peer_id);
    }
}
