//! Target Symbol: DataWallPaper
//! Telegram Desktop 1:1 Submodule: `data/data_wall_paper`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WallPaperType {
    #[default]
    SolidColor,
    Gradient,
    Pattern,
    CustomImage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataWallPaper {
    pub id: u64,
    pub slug: String,
    pub paper_type: WallPaperType,
    pub background_colors: Vec<u32>,
    pub gradient_rotation: i32,
    pub document_id: Option<u64>,
    pub intensity: i32,
    pub is_blurred: bool,
    pub is_motion: bool,
}

impl Default for DataWallPaper {
    fn default() -> Self {
        Self {
            id: 0,
            slug: "default".to_string(),
            paper_type: WallPaperType::SolidColor,
            background_colors: vec![0x1E2C3A], // Default dark theme background
            gradient_rotation: 0,
            document_id: None,
            intensity: 50,
            is_blurred: false,
            is_motion: false,
        }
    }
}

impl DataWallPaper {
    pub fn solid(id: u64, color: u32) -> Self {
        Self {
            id,
            paper_type: WallPaperType::SolidColor,
            background_colors: vec![color],
            ..Default::default()
        }
    }

    pub fn gradient(id: u64, colors: Vec<u32>, rotation: i32) -> Self {
        Self {
            id,
            paper_type: WallPaperType::Gradient,
            background_colors: colors,
            gradient_rotation: rotation,
            ..Default::default()
        }
    }

    pub fn pattern(id: u64, document_id: u64, colors: Vec<u32>, intensity: i32) -> Self {
        Self {
            id,
            paper_type: WallPaperType::Pattern,
            background_colors: colors,
            document_id: Some(document_id),
            intensity,
            ..Default::default()
        }
    }
}
