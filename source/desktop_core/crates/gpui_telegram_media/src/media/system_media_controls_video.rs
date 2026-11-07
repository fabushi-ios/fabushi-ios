//! Telegram Desktop 1:1 Parity Submodule: `SystemMediaControlsVideo`
//! Upstream Source: `Telegram/SourceFiles/media/system_media_controls_video.h:L1-L51`
//! Subsystem: `media` | Feature: `TCG-N08`

/// Video playback state for SMTC sync corresponding to `SystemMediaControlsVideoSink::VideoState` (L28-L36).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VideoState {
    pub title: String,
    pub artist: String,
    pub position_ms: i64,
    pub duration_ms: i64,
    pub playing: bool,
    pub next_available: bool,
    pub previous_available: bool,
}

impl VideoState {
    pub fn new(title: impl Into<String>, artist: impl Into<String>, duration_ms: i64) -> Self {
        Self {
            title: title.into(),
            artist: artist.into(),
            position_ms: 0,
            duration_ms,
            playing: false,
            next_available: false,
            previous_available: false,
        }
    }
}

/// User commands received from OS media controls corresponding to `SystemMediaControlsVideoDelegate` (L12-L24).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoMediaCommand {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    Seek(i64),
}

/// Parity target coordinator struct `SystemMediaControlsVideo`.
#[derive(Debug, Clone, Default)]
pub struct SystemMediaControlsVideo {
    pub state: VideoState,
    pub pending_commands: Vec<VideoMediaCommand>,
}

impl SystemMediaControlsVideo {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_state(&mut self, state: VideoState) {
        self.state = state;
    }

    pub fn push_command(&mut self, cmd: VideoMediaCommand) {
        self.pending_commands.push(cmd);
    }

    pub fn pop_command(&mut self) -> Option<VideoMediaCommand> {
        if self.pending_commands.is_empty() {
            None
        } else {
            Some(self.pending_commands.remove(0))
        }
    }

    pub fn is_playing(&self) -> bool {
        self.state.playing
    }
}
