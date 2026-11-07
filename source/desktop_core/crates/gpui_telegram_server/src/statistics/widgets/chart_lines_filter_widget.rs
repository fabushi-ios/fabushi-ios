//! Telegram Desktop 1:1 Submodule: `chart_lines_filter_widget`
//! Subsystem: `statistics`, Feature ID: `TCG-N37`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ButtonData {
    pub text: String,
    pub color_hex: String,
    pub id: i32,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: i32,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlatCheckbox {
    pub text: String,
    pub color_hex: String,
    pub id: i32,
    pub checked: bool,
    pub natural_width: f64,
    pub outer_width: f64,
    pub is_shaking: bool,
}

impl FlatCheckbox {
    pub fn new(text: impl Into<String>, color_hex: impl Into<String>, id: i32) -> Self {
        let t = text.into();
        let approx_width = (t.len() as f64 * 8.0) + 40.0;
        Self {
            text: t,
            color_hex: color_hex.into(),
            id,
            checked: true,
            natural_width: approx_width,
            outer_width: approx_width,
            is_shaking: false,
        }
    }

    pub fn set_checked(&mut self, checked: bool) {
        self.checked = checked;
    }

    pub fn shake(&mut self) {
        self.is_shaking = true;
    }

    pub fn stop_shake(&mut self) {
        self.is_shaking = false;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ChartLinesFilterWidgetWidgets {
    pub buttons: Vec<FlatCheckbox>,
    pub outer_width: f64,
}

impl ChartLinesFilterWidgetWidgets {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fill_buttons(&mut self, buttons_data: &[ButtonData]) {
        self.buttons = buttons_data
            .iter()
            .map(|b| {
                let mut cb = FlatCheckbox::new(&b.text, &b.color_hex, b.id);
                cb.checked = !b.disabled;
                cb
            })
            .collect();
    }

    pub fn resize_to_width(&mut self, outer_width: f64) {
        self.outer_width = outer_width;
        for btn in &mut self.buttons {
            btn.outer_width = btn.natural_width.min(outer_width).max(1.0);
        }
    }

    pub fn toggle_button(&mut self, id: i32) -> Option<Entry> {
        let active_count = self.buttons.iter().filter(|b| b.checked).count();
        if let Some(btn) = self.buttons.iter_mut().find(|b| b.id == id) {
            if btn.checked && active_count <= 1 {
                btn.shake();
                return None;
            }
            btn.stop_shake();
            btn.checked = !btn.checked;
            Some(Entry {
                id: btn.id,
                enabled: btn.checked,
            })
        } else {
            None
        }
    }

    pub fn is_button_enabled(&self, id: i32) -> bool {
        self.buttons.iter().find(|b| b.id == id).map_or(true, |b| b.checked)
    }
}
