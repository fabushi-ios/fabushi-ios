use super::media_audio::PlayerState;
use super::wav::decode_wav_pcm;
use gpui_telegram_core::error::{Result, TelegramError};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Instant;

/// Real OS audio playback controller backed by macOS `/usr/bin/afplay` with accurate
/// timeline tracking, pause/resume, playback speed (`0.5..=2.5`), volume control, and RAII cleanup.
pub struct AudioPlaybackEngine {
    state: PlayerState,
    track_key: Option<String>,
    duration_ms: i64,
    accumulated_ms: i64,
    started_at: Option<Instant>,
    speed: f64,
    volume: f32,
    child: Option<Child>,
    temp_file: Option<PathBuf>,
}

impl Default for AudioPlaybackEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioPlaybackEngine {
    pub fn new() -> Self {
        Self {
            state: PlayerState::Stopped,
            track_key: None,
            duration_ms: 0,
            accumulated_ms: 0,
            started_at: None,
            speed: 1.0,
            volume: 1.0,
            child: None,
            temp_file: None,
        }
    }

    pub fn play_wav_bytes(&mut self, track_key: impl Into<String>, wav_bytes: &[u8]) -> Result<()> {
        let decoded = decode_wav_pcm(wav_bytes)?;
        let temp_path = std::env::temp_dir().join(format!(
            "gpui_tg_play_{}_{}.wav",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::write(&temp_path, wav_bytes)?;
        let res = self.spawn_playback(track_key.into(), &temp_path, decoded.duration_ms);
        if res.is_err() {
            let _ = std::fs::remove_file(&temp_path);
            return res;
        }
        self.temp_file = Some(temp_path);
        Ok(())
    }

    pub fn play_pcm_i16(
        &mut self,
        track_key: impl Into<String>,
        samples: &[i16],
        sample_rate: u32,
        channels: u16,
    ) -> Result<()> {
        let wav = super::wav::encode_wav_i16(samples, sample_rate, channels)?;
        self.play_wav_bytes(track_key, &wav)
    }

    pub fn play_file(
        &mut self,
        track_key: impl Into<String>,
        path: &Path,
        duration_ms_hint: Option<i64>,
    ) -> Result<()> {
        if !path.is_file() {
            return Err(TelegramError::NotFound(format!(
                "Audio file not found: {}",
                path.display()
            )));
        }
        let duration_ms = match duration_ms_hint {
            Some(ms) => ms,
            None => std::fs::read(path)
                .ok()
                .and_then(|bytes| decode_wav_pcm(&bytes).ok())
                .map(|pcm| pcm.duration_ms)
                .unwrap_or(0),
        };
        self.spawn_playback(track_key.into(), path, duration_ms)
    }

    fn spawn_playback(&mut self, track_key: String, path: &Path, duration_ms: i64) -> Result<()> {
        self.stop();
        #[cfg(target_os = "macos")]
        {
            let afplay = Path::new("/usr/bin/afplay");
            if !afplay.exists() {
                self.state = PlayerState::StoppedAtError;
                return Err(TelegramError::Internal("macOS /usr/bin/afplay unavailable".into()));
            }
            let vol = self.volume.clamp(0.0, 1.0);
            let spd = self.speed.clamp(0.5, 2.5);
            let child = Command::new(afplay)
                .args([
                    "-v",
                    &format!("{vol:.3}"),
                    "-r",
                    &format!("{spd:.2}"),
                ])
                .arg(path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| {
                    self.state = PlayerState::StoppedAtError;
                    TelegramError::Internal(format!("Failed to start audio playback: {e}"))
                })?;
            self.child = Some(child);
            self.track_key = Some(track_key);
            self.duration_ms = duration_ms.max(0);
            self.accumulated_ms = 0;
            self.started_at = Some(Instant::now());
            self.state = PlayerState::Playing;
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (track_key, path, duration_ms);
            self.state = PlayerState::StoppedAtError;
            Err(TelegramError::Internal(
                "OS audio playback backend is only enabled on macOS".into(),
            ))
        }
    }

    pub fn pause(&mut self) -> Result<()> {
        self.poll_state();
        if self.state != PlayerState::Playing {
            return Ok(());
        }
        if let Some(started) = self.started_at.take() {
            let delta = ((started.elapsed().as_millis() as f64) * self.speed).round() as i64;
            self.accumulated_ms = self.accumulated_ms.saturating_add(delta);
        }
        #[cfg(unix)]
        if let Some(child) = &self.child {
            let pid = child.id().to_string();
            let status = Command::new("/bin/kill").args(["-STOP", &pid]).status()?;
            if !status.success() {
                return Err(TelegramError::Internal("Failed to pause audio playback".into()));
            }
        }
        self.state = PlayerState::Paused;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<()> {
        if self.state != PlayerState::Paused {
            return Ok(());
        }
        #[cfg(unix)]
        if let Some(child) = &self.child {
            let pid = child.id().to_string();
            let status = Command::new("/bin/kill").args(["-CONT", &pid]).status()?;
            if !status.success() {
                return Err(TelegramError::Internal("Failed to resume audio playback".into()));
            }
        }
        self.started_at = Some(Instant::now());
        self.state = PlayerState::Playing;
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            #[cfg(unix)]
            {
                let pid = child.id().to_string();
                let _ = Command::new("/bin/kill").args(["-CONT", &pid]).status();
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(path) = self.temp_file.take() {
            let _ = std::fs::remove_file(path);
        }
        self.started_at = None;
        self.accumulated_ms = 0;
        self.track_key = None;
        self.state = PlayerState::Stopped;
    }

    pub fn set_speed(&mut self, speed: f64) {
        self.speed = speed.clamp(0.5, 2.5);
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    pub fn poll_state(&mut self) -> PlayerState {
        if let Some(child) = &mut self.child {
            if let Ok(Some(exit)) = child.try_wait() {
                if let Some(started) = self.started_at.take() {
                    let delta = ((started.elapsed().as_millis() as f64) * self.speed).round() as i64;
                    self.accumulated_ms = self.accumulated_ms.saturating_add(delta);
                }
                self.child = None;
                if let Some(path) = self.temp_file.take() {
                    let _ = std::fs::remove_file(path);
                }
                self.state = if exit.success() {
                    PlayerState::StoppedAtEnd
                } else {
                    PlayerState::StoppedAtError
                };
            }
        }
        self.state
    }

    pub fn position_ms(&mut self) -> i64 {
        self.poll_state();
        let live = self
            .started_at
            .map(|s| ((s.elapsed().as_millis() as f64) * self.speed).round() as i64)
            .unwrap_or(0);
        let pos = self.accumulated_ms.saturating_add(live);
        if self.duration_ms > 0 { pos.min(self.duration_ms) } else { pos }
    }

    pub fn duration_ms(&self) -> i64 {
        self.duration_ms
    }

    pub fn current_track_key(&self) -> Option<&str> {
        self.track_key.as_deref()
    }
}

impl Drop for AudioPlaybackEngine {
    fn drop(&mut self) {
        self.stop();
    }
}
