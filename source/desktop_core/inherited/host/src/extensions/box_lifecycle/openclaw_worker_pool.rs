//! OpenClaw Worker Task Pool, Sizing, Memory Bounds, and Idle GC
//!
//! Mirrors:
//! - `src/infra/worker-task-pool.ts`
//! - `src/infra/worker-pool-sizing.ts`
//! - `src/infra/worker-task-capacity.ts`
//! - `src/infra/worker-cpu.ts`
//! - `src/infra/worker-idle-gc.ts`
//! - `src/infra/worker-transfer-bytes.ts`
//! - `src/infra/worker-native-lifecycle.ts`

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerTaskKind {
    CpuBound,
    IoBound,
    Interactive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerPoolConfig {
    pub min_workers: usize,
    pub max_workers: usize,
    pub idle_timeout_ms: u64,
    pub max_memory_bytes_per_worker: u64,
}

impl Default for WorkerPoolConfig {
    fn default() -> Self {
        Self {
            min_workers: 1,
            max_workers: 4,
            idle_timeout_ms: 30_000,
            max_memory_bytes_per_worker: 512 * 1024 * 1024, // 512 MiB
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerInstance {
    pub worker_id: String,
    pub active_tasks_count: usize,
    pub memory_used_bytes: u64,
    pub last_task_completed_ms: u64,
    pub is_idle: bool,
}

#[derive(Debug, Default)]
pub struct WorkerTaskPool {
    config: WorkerPoolConfig,
    workers: HashMap<String, WorkerInstance>,
}

impl WorkerTaskPool {
    pub fn new(config: WorkerPoolConfig) -> Self {
        Self {
            config,
            workers: HashMap::new(),
        }
    }

    pub fn register_worker(&mut self, worker_id: impl Into<String>, now_ms: u64) {
        let wid = worker_id.into();
        self.workers.insert(
            wid.clone(),
            WorkerInstance {
                worker_id: wid,
                active_tasks_count: 0,
                memory_used_bytes: 0,
                last_task_completed_ms: now_ms,
                is_idle: true,
            },
        );
    }

    pub fn assign_task(&mut self, worker_id: &str) -> Result<(), &'static str> {
        let worker = self.workers.get_mut(worker_id).ok_or("Worker not found")?;
        worker.active_tasks_count += 1;
        worker.is_idle = false;
        Ok(())
    }

    pub fn complete_task(&mut self, worker_id: &str, now_ms: u64) -> Result<(), &'static str> {
        let worker = self.workers.get_mut(worker_id).ok_or("Worker not found")?;
        worker.active_tasks_count = worker.active_tasks_count.saturating_sub(1);
        if worker.active_tasks_count == 0 {
            worker.is_idle = true;
            worker.last_task_completed_ms = now_ms;
        }
        Ok(())
    }

    pub fn sweep_idle_workers(&mut self, now_ms: u64) -> Vec<String> {
        let mut to_gc = Vec::new();
        for worker in self.workers.values() {
            if worker.is_idle && now_ms.saturating_sub(worker.last_task_completed_ms) > self.config.idle_timeout_ms {
                to_gc.push(worker.worker_id.clone());
            }
        }
        to_gc
    }
}
