//! Target Symbol: DataMediaRotation
//! Telegram Desktop 1:1 Submodule: `data/data_media_rotation`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RotationAngle {
    #[default]
    Degrees0,
    Degrees90,
    Degrees180,
    Degrees270,
}

impl RotationAngle {
    pub fn rotate_cw(self) -> Self {
        match self {
            Self::Degrees0 => Self::Degrees90,
            Self::Degrees90 => Self::Degrees180,
            Self::Degrees180 => Self::Degrees270,
            Self::Degrees270 => Self::Degrees0,
        }
    }

    pub fn rotate_ccw(self) -> Self {
        match self {
            Self::Degrees0 => Self::Degrees270,
            Self::Degrees90 => Self::Degrees0,
            Self::Degrees180 => Self::Degrees90,
            Self::Degrees270 => Self::Degrees180,
        }
    }

    pub fn as_degrees(self) -> u16 {
        match self {
            Self::Degrees0 => 0,
            Self::Degrees90 => 90,
            Self::Degrees180 => 180,
            Self::Degrees270 => 270,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DataMediaRotation {
    pub angle: RotationAngle,
    pub flipped_horizontally: bool,
}

impl DataMediaRotation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn rotate_cw(&mut self) {
        self.angle = self.angle.rotate_cw();
    }

    pub fn rotate_ccw(&mut self) {
        self.angle = self.angle.rotate_ccw();
    }

    pub fn toggle_flip(&mut self) {
        self.flipped_horizontally = !self.flipped_horizontally;
    }

    pub fn is_identity(&self) -> bool {
        self.angle == RotationAngle::Degrees0 && !self.flipped_horizontally
    }

    pub fn transform_dimensions(&self, width: u32, height: u32) -> (u32, u32) {
        match self.angle {
            RotationAngle::Degrees0 | RotationAngle::Degrees180 => (width, height),
            RotationAngle::Degrees90 | RotationAngle::Degrees270 => (height, width),
        }
    }
}
