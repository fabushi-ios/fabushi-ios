//! Target Symbol: StorageSparseIdsList
//! Telegram Desktop 1:1 Submodule: `StorageSparseIdsList`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::collections::BTreeSet;

pub type MsgId = i32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MsgRange {
    pub from: MsgId,
    pub till: MsgId,
}

impl MsgRange {
    pub fn new(from: MsgId, till: MsgId) -> Self {
        Self { from, till }
    }

    pub fn contains(&self, id: MsgId) -> bool {
        id >= self.from && id <= self.till
    }

    pub fn overlaps_or_adjacent(&self, other: &MsgRange) -> bool {
        self.from <= other.till + 1 && other.from <= self.till + 1
    }

    pub fn merge(&self, other: &MsgRange) -> MsgRange {
        MsgRange {
            from: self.from.min(other.from),
            till: self.till.max(other.till),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SparseIdsListQuery {
    pub around_id: MsgId,
    pub limit_before: usize,
    pub limit_after: usize,
}

impl SparseIdsListQuery {
    pub fn new(around_id: MsgId, limit_before: usize, limit_after: usize) -> Self {
        Self {
            around_id,
            limit_before,
            limit_after,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SparseIdsListResult {
    pub count: Option<usize>,
    pub skipped_before: Option<usize>,
    pub skipped_after: Option<usize>,
    pub message_ids: Vec<MsgId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slice {
    pub messages: BTreeSet<MsgId>,
    pub range: MsgRange,
}

impl Slice {
    pub fn new(messages: BTreeSet<MsgId>, range: MsgRange) -> Self {
        Self { messages, range }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SparseIdsList {
    count: Option<usize>,
    slices: Vec<Slice>,
}

impl SparseIdsList {
    pub fn new() -> Self {
        Self {
            count: None,
            slices: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.slices.is_empty() || self.slices.iter().all(|s| s.messages.is_empty())
    }

    pub fn total_count(&self) -> Option<usize> {
        self.count
    }

    pub fn add_new(&mut self, message_id: MsgId) {
        if let Some(c) = self.count.as_mut() {
            *c += 1;
        }
        if let Some(slice) = self.slices.first_mut() {
            if message_id >= slice.range.till {
                slice.messages.insert(message_id);
                slice.range.till = message_id;
                return;
            }
        }
        let mut set = BTreeSet::new();
        set.insert(message_id);
        self.slices.push(Slice::new(set, MsgRange::new(message_id, message_id)));
    }

    pub fn add_existing(&mut self, message_id: MsgId, no_skip_range: MsgRange, increment_count: bool) {
        if increment_count {
            if let Some(c) = self.count.as_mut() {
                *c += 1;
            }
        }
        self.add_slice(vec![message_id], no_skip_range, self.count);
    }

    pub fn add_slice(
        &mut self,
        mut new_ids: Vec<MsgId>,
        mut range: MsgRange,
        count: Option<usize>,
    ) {
        if let Some(c) = count {
            self.count = Some(c);
        }

        let mut set = BTreeSet::new();
        for id in new_ids.drain(..) {
            range.from = range.from.min(id);
            range.till = range.till.max(id);
            set.insert(id);
        }

        // Merge with overlapping existing slices
        let mut i = 0;
        while i < self.slices.len() {
            if self.slices[i].range.overlaps_or_adjacent(&range) {
                let existing = self.slices.remove(i);
                range = range.merge(&existing.range);
                for id in existing.messages {
                    set.insert(id);
                }
            } else {
                i += 1;
            }
        }

        self.slices.push(Slice::new(set, range));
        self.slices.sort_by_key(|s| s.range.from);
    }

    pub fn remove_one(&mut self, message_id: MsgId) {
        let mut removed = false;
        for slice in &mut self.slices {
            if slice.messages.remove(&message_id) {
                removed = true;
            }
        }
        if removed {
            if let Some(c) = self.count.as_mut() {
                *c = c.saturating_sub(1);
            }
        }
    }

    pub fn remove_all(&mut self) {
        self.slices.clear();
        self.count = Some(0);
    }

    pub fn invalidate_bottom(&mut self) {
        if let Some(slice) = self.slices.first_mut() {
            slice.range.from = slice.messages.iter().next().copied().unwrap_or(0);
        }
    }

    pub fn snapshot(&self, query: &SparseIdsListQuery) -> SparseIdsListResult {
        for slice in &self.slices {
            if slice.range.contains(query.around_id)
                || (slice.range.from <= query.around_id && query.around_id <= slice.range.till)
            {
                let before: Vec<MsgId> = slice
                    .messages
                    .range(..query.around_id)
                    .rev()
                    .take(query.limit_before)
                    .copied()
                    .collect();

                let at_and_after: Vec<MsgId> = slice
                    .messages
                    .range(query.around_id..)
                    .take(query.limit_after + 1)
                    .copied()
                    .collect();

                let mut combined: Vec<MsgId> = before.into_iter().rev().collect();
                combined.extend(at_and_after);

                let skipped_before = if let Some(first) = combined.first() {
                    Some(slice.messages.range(..*first).count())
                } else {
                    Some(0)
                };

                let skipped_after = if let Some(last) = combined.last() {
                    Some(slice.messages.range((*last + 1)..).count())
                } else {
                    Some(0)
                };

                return SparseIdsListResult {
                    count: self.count,
                    skipped_before,
                    skipped_after,
                    message_ids: combined,
                };
            }
        }

        SparseIdsListResult {
            count: self.count,
            skipped_before: None,
            skipped_after: None,
            message_ids: Vec::new(),
        }
    }

    pub fn count_after(&self, till_id: MsgId, limit: usize) -> Option<usize> {
        let mut total = 0;
        for slice in &self.slices {
            for &_id in slice.messages.range((till_id + 1)..) {
                total += 1;
                if total >= limit {
                    return Some(limit);
                }
            }
        }
        Some(total)
    }
}

#[derive(Debug, Clone, Default)]
pub struct StorageSparseIdsList {
    pub inner: SparseIdsList,
}

impl StorageSparseIdsList {
    pub fn new() -> Self {
        Self {
            inner: SparseIdsList::new(),
        }
    }

    pub fn add(&mut self, id: MsgId) {
        self.inner.add_new(id);
    }

    pub fn query(&self, around_id: MsgId, limit_before: usize, limit_after: usize) -> SparseIdsListResult {
        self.inner.snapshot(&SparseIdsListQuery::new(around_id, limit_before, limit_after))
    }
}
