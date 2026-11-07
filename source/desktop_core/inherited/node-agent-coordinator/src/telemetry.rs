use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportStage {
    pub request_id: String,
    pub stage: String,
    pub attempt: u32,
    pub duration_ms: u64,
    pub is_error: bool,
}

#[derive(Debug)]
pub struct TransportStageRecorder {
    max_entries: usize,
    entries: VecDeque<TransportStage>,
}

impl TransportStageRecorder {
    pub fn new(max_entries: usize) -> Self {
        Self { max_entries: max_entries.max(1), entries: VecDeque::new() }
    }

    pub fn record(&mut self, stage: TransportStage) {
        while self.entries.len() >= self.max_entries {
            self.entries.pop_front();
        }
        self.entries.push_back(stage);
    }

    pub fn entries(&self) -> impl Iterator<Item = &TransportStage> {
        self.entries.iter()
    }
}

pub mod transport_stage_recorder;
