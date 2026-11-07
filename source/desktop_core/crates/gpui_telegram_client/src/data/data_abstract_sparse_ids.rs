//! Target Symbol: DataAbstractSparseIds
//! Telegram Desktop 1:1 Submodule: `DataAbstractSparseIds`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

/// Generic container for sparse collections of ordered identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbstractSparseIds<T> {
    pub ids: Vec<T>,
    pub full_count: Option<usize>,
    pub skipped_before: Option<usize>,
    pub skipped_after: Option<usize>,
}

impl<T: PartialEq + Clone> AbstractSparseIds<T> {
    pub fn new(
        ids: Vec<T>,
        full_count: Option<usize>,
        skipped_before: Option<usize>,
        skipped_after: Option<usize>,
    ) -> Self {
        Self {
            ids,
            full_count,
            skipped_before,
            skipped_after,
        }
    }

    pub fn index_of(&self, id: &T) -> Option<usize> {
        self.ids.iter().position(|item| item == id)
    }

    pub fn distance(&self, a: &T, b: &T) -> Option<isize> {
        let i = self.index_of(a)?;
        let j = self.index_of(b)?;
        Some(j as isize - i as isize)
    }

    pub fn size(&self) -> usize {
        self.ids.len()
    }

    pub fn reverse(&mut self) {
        self.ids.reverse();
        std::mem::swap(&mut self.skipped_before, &mut self.skipped_after);
    }
}

impl<T> Default for AbstractSparseIds<T> {
    fn default() -> Self {
        Self {
            ids: Vec::new(),
            full_count: None,
            skipped_before: None,
            skipped_after: None,
        }
    }
}

/// Target Symbol for Module 03 Ledger: DataAbstractSparseIds coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataAbstractSparseIds {
    pub sparse_u64: AbstractSparseIds<u64>,
}

impl DataAbstractSparseIds {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_ids(ids: Vec<u64>) -> Self {
        let full_count = Some(ids.len());
        Self {
            sparse_u64: AbstractSparseIds::new(ids, full_count, Some(0), Some(0)),
        }
    }

    pub fn count(&self) -> usize {
        self.sparse_u64.size()
    }
}
