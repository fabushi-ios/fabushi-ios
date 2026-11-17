//! Media Capture Device Subsystem & Synthetic Mock Sources

pub mod device_source;
pub mod frames;
pub mod coreaudio_source;
pub mod mock_audio;
pub mod mock_video;
pub mod mock_screen;

pub use device_source::*;
pub use frames::*;
pub use coreaudio_source::*;
pub use mock_audio::*;
pub use mock_video::*;
pub use mock_screen::*;
