//! Telegram Desktop 1:1 Parity Submodule: `MediaViewRecognitionSelectionView`
//! Subsystem: `media` | Feature ID: `TCG-N10`
//! Disposition: `gpui_ui_1to1`
//! Upstream Sources:
//! - `Telegram/SourceFiles/media/view/media_view_recognition_selection.h`
//! - `Telegram/SourceFiles/media/view/media_view_recognition_selection.cpp:L21-L57, L68-L85`

use serde::{Deserialize, Serialize};

/// Recognized text bounding box and line content
/// Upstream: `media_view_recognition_selection.h:L22`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub text: String,
}

/// Computes natural reading order from scattered bounding boxes
/// Upstream: `media_view_recognition_selection.cpp:L21-L57`
pub fn compute_reading_order(items: &[TextRect]) -> Vec<usize> {
    if items.is_empty() {
        return Vec::new();
    }
    let count = items.len();
    let mut order: Vec<usize> = (0..count).collect();
    let mut rows = vec![0usize; count];

    let center_y = |idx: usize| items[idx].y + items[idx].height / 2;

    // 1. Initial sort by vertical center
    order.sort_by_key(|&idx| center_y(idx));

    // 2. Cluster into reading rows
    let mut current_row = 0;
    for k in 1..count {
        let prev = order[k - 1];
        let cur = order[k];
        let threshold = items[prev].height.max(items[cur].height) / 2;
        if center_y(cur) - center_y(prev) > threshold {
            current_row += 1;
        }
        rows[cur] = current_row;
    }

    // 3. Secondary stable sort by row, then x-coordinate
    order.sort_by(|&a, &b| {
        if rows[a] != rows[b] {
            rows[a].cmp(&rows[b])
        } else {
            items[a].x.cmp(&items[b].x)
        }
    });

    order
}

/// Joins recognized blocks into flowing paragraph text
/// Upstream: `media_view_recognition_selection.cpp:L68-L85`
pub fn recognized_text(items: &[TextRect]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let order = compute_reading_order(items);
    let mut text = String::new();
    let mut last_y: Option<i32> = None;

    for &idx in &order {
        let line = &items[idx];
        if line.text.is_empty() {
            continue;
        }

        if let Some(prev_y) = last_y {
            let threshold = line.height / 2;
            if (line.y - prev_y).abs() > threshold {
                text.push('\n');
            } else {
                text.push(' ');
            }
        }
        text.push_str(&line.text);
        last_y = Some(line.y);
    }

    text
}

/// Primary target symbol for `media_view_recognition_selection.rs`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MediaViewRecognitionSelectionView {
    pub items: Vec<TextRect>,
    pub selected_text: String,
    pub is_selecting: bool,
}

impl MediaViewRecognitionSelectionView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_items(&mut self, items: Vec<TextRect>) {
        self.selected_text = recognized_text(&items);
        self.items = items;
    }

    pub fn full_text(&self) -> String {
        recognized_text(&self.items)
    }

    pub fn clear_selection(&mut self) {
        self.is_selecting = false;
    }
}
