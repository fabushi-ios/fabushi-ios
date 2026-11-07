//! Target Symbol: DataWebPage
//! Telegram Desktop 1:1 Submodule: `data/data_web_page`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WebPageType {
    #[default]
    GenericLink,
    Article,
    ArticleWithInstantView,
    Photo,
    Video,
    Document,
    Profile,
    Group,
    Channel,
    Story,
    StickerSet,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataWebPage {
    pub id: u64,
    pub url: String,
    pub display_url: String,
    pub site_name: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub page_type: WebPageType,
    pub photo_id: Option<u64>,
    pub document_id: Option<u64>,
    pub has_instant_view: bool,
    pub duration_seconds: Option<u32>,
}

impl Default for DataWebPage {
    fn default() -> Self {
        Self {
            id: 0,
            url: String::new(),
            display_url: String::new(),
            site_name: String::new(),
            title: String::new(),
            description: String::new(),
            author: String::new(),
            page_type: WebPageType::GenericLink,
            photo_id: None,
            document_id: None,
            has_instant_view: false,
            duration_seconds: None,
        }
    }
}

impl DataWebPage {
    pub fn new(id: u64, url: impl Into<String>) -> Self {
        let u = url.into();
        Self {
            id,
            url: u.clone(),
            display_url: u,
            ..Default::default()
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn with_site_name(mut self, site: impl Into<String>) -> Self {
        self.site_name = site.into();
        self
    }

    pub fn with_instant_view(mut self, has_iv: bool) -> Self {
        self.has_instant_view = has_iv;
        if has_iv {
            self.page_type = WebPageType::ArticleWithInstantView;
        }
        self
    }
}
