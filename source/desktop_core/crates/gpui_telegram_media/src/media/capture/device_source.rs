//! Unified Media Device Source Interface & Lifecycle Management

use super::frames::{AudioFrame, ScreenFrame, VideoFrame};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MediaDeviceKind {
    AudioInput,
    VideoInput,
    ScreenCapture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DeviceState {
    #[default]
    Uninitialized,
    Streaming,
    Paused,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaDeviceInfo {
    pub id: String,
    pub name: String,
    pub kind: MediaDeviceKind,
    pub is_default: bool,
}

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum MediaCaptureError {
    #[error("Device not found: {0}")]
    DeviceNotFound(String),
    #[error("Device is unavailable or disconnected")]
    DeviceUnavailable,
    #[error("Permission denied to access media device: {0}")]
    PermissionDenied(String),
    #[error("Device initialization failed: {0}")]
    InitFailed(String),
    #[error("Invalid device state transition from {0:?} to {1:?}")]
    InvalidState(DeviceState, DeviceState),
    #[error("Unsupported operation: {0}")]
    UnsupportedOperation(String),
    #[error("Internal capture error: {0}")]
    Internal(String),
}

pub type AudioFrameCallback = Arc<dyn Fn(AudioFrame) + Send + Sync + 'static>;
pub type VideoFrameCallback = Arc<dyn Fn(VideoFrame) + Send + Sync + 'static>;
pub type ScreenFrameCallback = Arc<dyn Fn(ScreenFrame) + Send + Sync + 'static>;

/// Unified abstraction for media capture sources (microphones, cameras, screen capture, and mocks).
pub trait MediaDeviceSource: Send + Sync {
    /// Device kind (AudioInput, VideoInput, ScreenCapture).
    fn kind(&self) -> MediaDeviceKind;

    /// Current device descriptor.
    fn device_info(&self) -> MediaDeviceInfo;

    /// Current lifecycle state.
    fn state(&self) -> DeviceState;

    /// Selects a specific host device by ID.
    fn select_device(&mut self, device_id: &str) -> Result<(), MediaCaptureError>;

    /// Starts streaming frames.
    fn start(&mut self) -> Result<(), MediaCaptureError>;

    /// Pauses or resumes streaming.
    fn pause(&mut self, paused: bool) -> Result<(), MediaCaptureError>;

    /// Stops streaming and releases hardware resources.
    fn stop(&mut self) -> Result<(), MediaCaptureError>;

    /// True if actively streaming frames.
    fn is_streaming(&self) -> bool {
        self.state() == DeviceState::Streaming
    }

    /// True if paused.
    fn is_paused(&self) -> bool {
        self.state() == DeviceState::Paused
    }

    /// Register audio frame callback (default returns UnsupportedOperation).
    fn set_audio_callback(&mut self, _cb: AudioFrameCallback) -> Result<(), MediaCaptureError> {
        Err(MediaCaptureError::UnsupportedOperation(
            "Audio callback not supported".into(),
        ))
    }

    /// Register video frame callback (default returns UnsupportedOperation).
    fn set_video_callback(&mut self, _cb: VideoFrameCallback) -> Result<(), MediaCaptureError> {
        Err(MediaCaptureError::UnsupportedOperation(
            "Video callback not supported".into(),
        ))
    }

    /// Register screen frame callback (default returns UnsupportedOperation).
    fn set_screen_callback(&mut self, _cb: ScreenFrameCallback) -> Result<(), MediaCaptureError> {
        Err(MediaCaptureError::UnsupportedOperation(
            "Screen callback not supported".into(),
        ))
    }
}
