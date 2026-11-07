pub mod generator;
pub mod geometry;
pub mod stripped;

pub use generator::{ThumbnailGenerator, ThumbnailPreset};
pub use geometry::{fit_dimensions_downscale_only, shrink_to_keep_aspect};
pub use stripped::{
    envelope_stripped_photo, parse_stripped_dimensions, TELEGRAM_STRIPPED_PREFIX,
};
