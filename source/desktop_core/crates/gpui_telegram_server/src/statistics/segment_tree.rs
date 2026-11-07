//! Telegram Desktop 1:1 Submodule: `segment_tree`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use crate::statistics::statistics_types::ChartValue;
use serde::{Deserialize, Serialize};

pub const K_MIN_ARRAY_SIZE: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct PendingVal {
    pub value: ChartValue,
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Node {
    pub sum: ChartValue,
    pub max: ChartValue,
    pub min: ChartValue,
    pub pending_val: PendingVal,
    pub from: usize,
    pub to: usize,
}

impl Node {
    pub fn size(&self) -> usize {
        self.to.saturating_sub(self.from) + 1
    }
}

/// Upstream 1:1 class: `SegmentTree`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SegmentTree {
    array: Vec<ChartValue>,
    heap: Vec<Node>,
}

impl SegmentTree {
    pub fn new(array: Vec<ChartValue>) -> Self {
        let n = array.len();
        if n < K_MIN_ARRAY_SIZE {
            return Self {
                array,
                heap: Vec::new(),
            };
        }

        let power = ((n as f64).log2().floor() + 1.0) as u32;
        let heap_size = 2 * (1usize << power) + 1;
        let mut tree = Self {
            array,
            heap: vec![Node::default(); heap_size],
        };
        tree.build(1, 0, n);
        tree
    }

    pub fn is_empty(&self) -> bool {
        self.array.is_empty()
    }

    pub fn len(&self) -> usize {
        self.array.len()
    }

    fn build(&mut self, v: usize, from: usize, size: usize) {
        if v >= self.heap.len() || size == 0 {
            return;
        }
        self.heap[v].from = from;
        self.heap[v].to = from + size - 1;

        if size == 1 {
            let val = self.array.get(from).copied().unwrap_or(0);
            self.heap[v].sum = val;
            self.heap[v].max = val;
            self.heap[v].min = val;
        } else {
            let half = size / 2;
            self.build(2 * v, from, half);
            self.build(2 * v + 1, from + half, size - half);

            if 2 * v + 1 < self.heap.len() {
                self.heap[v].sum = self.heap[2 * v].sum + self.heap[2 * v + 1].sum;
                self.heap[v].max = self.heap[2 * v].max.max(self.heap[2 * v + 1].max);
                self.heap[v].min = self.heap[2 * v].min.min(self.heap[2 * v + 1].min);
            }
        }
    }

    pub fn r_max_q(&mut self, from: usize, to: usize) -> ChartValue {
        if self.array.is_empty() {
            return 0;
        }
        if self.array.len() < K_MIN_ARRAY_SIZE {
            let start = from.min(self.array.len() - 1);
            let end = to.min(self.array.len() - 1);
            return self.array[start..=end].iter().copied().max().unwrap_or(0);
        }
        self.r_max_q_internal(1, from, to)
    }

    fn r_max_q_internal(&mut self, v: usize, from: usize, to: usize) -> ChartValue {
        if v >= self.heap.len() {
            return 0;
        }
        let n = self.heap[v].clone();
        if n.pending_val.available && self.contains(n.from, n.to, from, to) {
            return n.pending_val.value;
        }
        if self.contains(from, to, n.from, n.to) {
            return self.heap[v].max;
        }
        if self.intersects(from, to, n.from, n.to) {
            self.propagate(v);
            let left_max = self.r_max_q_internal(2 * v, from, to);
            let right_max = self.r_max_q_internal(2 * v + 1, from, to);
            return left_max.max(right_max);
        }
        0
    }

    pub fn r_min_q(&mut self, from: usize, to: usize) -> ChartValue {
        if self.array.is_empty() {
            return 0;
        }
        if self.array.len() < K_MIN_ARRAY_SIZE {
            let start = from.min(self.array.len() - 1);
            let end = to.min(self.array.len() - 1);
            return self.array[start..=end].iter().copied().min().unwrap_or(0);
        }
        self.r_min_q_internal(1, from, to)
    }

    fn r_min_q_internal(&mut self, v: usize, from: usize, to: usize) -> ChartValue {
        if v >= self.heap.len() {
            return ChartValue::MAX;
        }
        let n = self.heap[v].clone();
        if n.pending_val.available && self.contains(n.from, n.to, from, to) {
            return n.pending_val.value;
        }
        if self.contains(from, to, n.from, n.to) {
            return self.heap[v].min;
        }
        if self.intersects(from, to, n.from, n.to) {
            self.propagate(v);
            let left_min = self.r_min_q_internal(2 * v, from, to);
            let right_min = self.r_min_q_internal(2 * v + 1, from, to);
            return left_min.min(right_min);
        }
        ChartValue::MAX
    }

    fn propagate(&mut self, v: usize) {
        if v >= self.heap.len() {
            return;
        }
        if self.heap[v].pending_val.available {
            let val = self.heap[v].pending_val.value;
            self.heap[v].pending_val.available = false;
            if 2 * v < self.heap.len() {
                self.change_node(2 * v, val);
            }
            if 2 * v + 1 < self.heap.len() {
                self.change_node(2 * v + 1, val);
            }
        }
    }

    fn change_node(&mut self, v: usize, value: ChartValue) {
        let size = self.heap[v].size() as ChartValue;
        self.heap[v].pending_val = PendingVal { value, available: true };
        self.heap[v].sum = size * value;
        self.heap[v].max = value;
        self.heap[v].min = value;
        let from = self.heap[v].from;
        if from < self.array.len() {
            self.array[from] = value;
        }
    }

    fn contains(&self, from1: usize, to1: usize, from2: usize, to2: usize) -> bool {
        from2 >= from1 && to2 <= to1
    }

    fn intersects(&self, from1: usize, to1: usize, from2: usize, to2: usize) -> bool {
        (from1 <= from2 && to1 >= from2) || (from1 >= from2 && from1 <= to2)
    }
}
