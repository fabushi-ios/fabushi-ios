//! Telegram Desktop 1:1 Submodule: `chart_header_widget`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use serde::{Deserialize, Serialize};

pub const K_STATISTICS_CHART_HEADER_HEIGHT: f64 = 36.0;
pub const K_TITLE_FONT_HEIGHT: f64 = 18.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub title: String,
    pub sub_title: String,
    pub height: f64,
}

impl Default for Header {
    fn default() -> Self {
        Self {
            title: String::new(),
            sub_title: String::new(),
            height: K_STATISTICS_CHART_HEADER_HEIGHT,
        }
    }
}

impl Header {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    pub fn set_sub_title(&mut self, sub_title: impl Into<String>) {
        let st = sub_title.into();
        self.height = if st.is_empty() {
            K_TITLE_FONT_HEIGHT
        } else {
            K_STATISTICS_CHART_HEADER_HEIGHT
        };
        self.sub_title = st;
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn sub_title(&self) -> &str {
        &self.sub_title
    }

    pub fn resize_get_height(&self, _new_width: f64) -> f64 {
        self.height
    }
}

/// Target symbol wrapper for compatibility
pub type ChartHeaderWidgetWidgets = Header;
