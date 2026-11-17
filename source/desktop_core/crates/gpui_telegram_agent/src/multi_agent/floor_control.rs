use serde::{Deserialize, Serialize};
use std::collections::{BinaryHeap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum FloorPriority {
    UserInterruption = 0,
    DirectMention = 1,
    TaskAssignee = 2,
    GeneralContribution = 3,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct FloorRequest {
    priority: FloorPriority,
    seq: u64,
    agent_id: String,
    enqueued_at_promotion: u64,
}

impl Ord for FloorRequest {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Lower numerical priority = higher precedence
        other
            .priority
            .cmp(&self.priority)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

impl PartialOrd for FloorRequest {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone)]
pub struct FloorControlQueue {
    lease_duration_ms: u64,
    current_speaker: Option<(String, FloorPriority, u64)>,
    wait_queue: BinaryHeap<FloorRequest>,
    queued_agents: HashSet<String>,
    seq_counter: u64,
    promotion_count: u64,
}

impl Default for FloorControlQueue {
    fn default() -> Self {
        Self::new(10_000)
    }
}

impl FloorControlQueue {
    pub fn new(lease_duration_ms: u64) -> Self {
        Self {
            lease_duration_ms,
            current_speaker: None,
            wait_queue: BinaryHeap::new(),
            queued_agents: HashSet::new(),
            seq_counter: 0,
            promotion_count: 0,
        }
    }

    fn remove_from_queue(&mut self, agent_id: &str) -> Option<FloorRequest> {
        if self.queued_agents.remove(agent_id) {
            let mut removed = None;
            let remaining: Vec<FloorRequest> = self
                .wait_queue
                .drain()
                .filter(|r| {
                    if r.agent_id == agent_id && removed.is_none() {
                        removed = Some(r.clone());
                        false
                    } else {
                        true
                    }
                })
                .collect();
            for req in remaining {
                self.wait_queue.push(req);
            }
            removed
        } else {
            None
        }
    }

    pub fn request_floor(&mut self, agent_id: &str, priority: FloorPriority) -> bool {
        self.request_floor_at(
            agent_id,
            priority,
            chrono::Utc::now().timestamp_millis().max(0) as u64,
        )
    }

    pub fn request_floor_at(
        &mut self,
        agent_id: &str,
        priority: FloorPriority,
        now_ms: u64,
    ) -> bool {
        self.drain_expired_leases(now_ms);

        if let Some((current, _, _)) = &self.current_speaker {
            if current == agent_id {
                return true;
            }
        }

        if priority == FloorPriority::UserInterruption {
            self.remove_from_queue(agent_id);
            if let Some((prev_id, prev_prio, _)) = self.current_speaker.take() {
                if prev_id != agent_id && !self.queued_agents.contains(&prev_id) {
                    self.seq_counter += 1;
                    self.wait_queue.push(FloorRequest {
                        priority: prev_prio,
                        seq: self.seq_counter,
                        agent_id: prev_id.clone(),
                        enqueued_at_promotion: self.promotion_count,
                    });
                    self.queued_agents.insert(prev_id);
                }
            }
            self.current_speaker = Some((
                agent_id.to_string(),
                priority,
                now_ms.saturating_add(self.lease_duration_ms),
            ));
            return true;
        }

        if self.current_speaker.is_none() {
            self.remove_from_queue(agent_id);
            self.current_speaker = Some((
                agent_id.to_string(),
                priority,
                now_ms.saturating_add(self.lease_duration_ms),
            ));
            true
        } else {
            if self.queued_agents.contains(agent_id) {
                if let Some(mut existing) = self.remove_from_queue(agent_id) {
                    if priority < existing.priority {
                        existing.priority = priority;
                    }
                    self.queued_agents.insert(existing.agent_id.clone());
                    self.wait_queue.push(existing);
                }
            } else {
                self.seq_counter += 1;
                self.wait_queue.push(FloorRequest {
                    priority,
                    seq: self.seq_counter,
                    agent_id: agent_id.to_string(),
                    enqueued_at_promotion: self.promotion_count,
                });
                self.queued_agents.insert(agent_id.to_string());
            }
            false
        }
    }

    pub fn release_floor(&mut self, agent_id: &str) {
        if let Some((current, _, _)) = &self.current_speaker {
            if current == agent_id {
                self.current_speaker = None;
                self.promote_next_speaker(chrono::Utc::now().timestamp_millis().max(0) as u64);
            }
        }
    }

    fn promote_next_speaker(&mut self, now_ms: u64) {
        const STARVATION_PROMOTION_THRESHOLD: u64 = 8;
        let starved_agent = self
            .wait_queue
            .iter()
            .filter(|r| {
                self.promotion_count
                    .saturating_sub(r.enqueued_at_promotion)
                    >= STARVATION_PROMOTION_THRESHOLD
            })
            .min_by_key(|r| r.seq)
            .map(|r| r.agent_id.clone());

        if let Some(agent_id) = starved_agent {
            if let Some(req) = self.remove_from_queue(&agent_id) {
                self.promotion_count = self.promotion_count.saturating_add(1);
                self.current_speaker = Some((
                    req.agent_id,
                    req.priority,
                    now_ms.saturating_add(self.lease_duration_ms),
                ));
                return;
            }
        }

        while let Some(req) = self.wait_queue.pop() {
            if self.queued_agents.remove(&req.agent_id) {
                self.promotion_count = self.promotion_count.saturating_add(1);
                self.current_speaker = Some((
                    req.agent_id,
                    req.priority,
                    now_ms.saturating_add(self.lease_duration_ms),
                ));
                break;
            }
        }
    }

    pub fn current_speaker(&self) -> Option<&str> {
        self.current_speaker.as_ref().map(|(id, _, _)| id.as_str())
    }

    pub fn queued_count(&self) -> usize {
        self.queued_agents.len()
    }

    pub fn drain_expired_leases(&mut self, now_ms: u64) {
        if let Some((_, _, lease_end)) = self.current_speaker {
            if now_ms >= lease_end {
                self.current_speaker = None;
                self.promote_next_speaker(now_ms);
            }
        }
    }
}
