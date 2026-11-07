use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_TRANSCRIPT_QUERY_BUDGET: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub id: u64,
    pub speaker: String,
    pub text: String,
    pub timestamp_ms: u64,
}

#[derive(Clone, Default)]
pub struct TranscriptCaptureStore {
    entries: Arc<Mutex<Vec<TranscriptEntry>>>,
}

impl TranscriptCaptureStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append(&self, speaker: &str, text: &str) -> TranscriptEntry {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let mut entries = self.entries.lock().unwrap();
        let id = entries.len() as u64 + 1;

        let entry = TranscriptEntry {
            id,
            speaker: speaker.to_string(),
            text: text.to_string(),
            timestamp_ms: now_ms,
        };

        entries.push(entry.clone());
        entry
    }

    pub fn query_entries(&self, filter: Option<&str>, limit: usize) -> Vec<TranscriptEntry> {
        let bounded_limit = limit.min(DEFAULT_TRANSCRIPT_QUERY_BUDGET);
        let entries = self.entries.lock().unwrap();

        entries
            .iter()
            .filter(|e| {
                if let Some(f) = filter {
                    e.text.to_ascii_lowercase().contains(&f.to_ascii_lowercase())
                } else {
                    true
                }
            })
            .take(bounded_limit)
            .cloned()
            .collect()
    }

    pub fn generate_summary_text(&self) -> String {
        let entries = self.entries.lock().unwrap();
        let mut summary = String::new();
        for e in entries.iter() {
            summary.push_str(&format!("{}: {}\n", e.speaker, e.text));
        }
        summary
    }
}
