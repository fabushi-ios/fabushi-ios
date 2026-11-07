use std::sync::Mutex;

use super::transcript_hub::{TranscriptEntry, transcript_entry_id};

#[derive(Debug, Default)]
pub struct TranscriptStore {
    entries: Mutex<Vec<TranscriptEntry>>,
}

impl TranscriptStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_transcript(&self) -> Vec<TranscriptEntry> {
        self.entries
            .lock()
            .map(|entries| entries.clone())
            .unwrap_or_default()
    }

    pub fn set_transcript(&self, entries: &[TranscriptEntry]) {
        if let Ok(mut current) = self.entries.lock() {
            *current = entries.to_vec();
        }
    }

    pub fn append_entry(&self, entry: TranscriptEntry) {
        if let Ok(mut current) = self.entries.lock() {
            current.push(entry);
        }
    }

    pub fn update_entry<F>(
        &self,
        id: &str,
        update: F,
    ) -> Option<TranscriptEntry>
    where
        F: FnOnce(&TranscriptEntry) -> TranscriptEntry,
    {
        let mut current = self.entries.lock().ok()?;
        let entry = current
            .iter_mut()
            .find(|entry| transcript_entry_id(entry) == Some(id))?;
        let next = update(entry);
        *entry = next.clone();
        Some(next)
    }

    pub fn remove_entry(&self, id: &str) -> bool {
        let Ok(mut current) = self.entries.lock() else {
            return false;
        };
        let before = current.len();
        current.retain(|entry| transcript_entry_id(entry) != Some(id));
        current.len() != before
    }

    pub fn clear_transcript(&self) {
        if let Ok(mut current) = self.entries.lock() {
            current.clear();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries
            .lock()
            .map(|entries| entries.is_empty())
            .unwrap_or(true)
    }
}
