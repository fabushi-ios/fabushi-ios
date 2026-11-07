use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    Arc, Condvar, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const VIDEO_TOOL_TIMEOUT_MS: u64 = 2 * 60 * 1_000;
pub const MAX_CACHED_RENDITIONS: usize = 8;
pub const FAILED_RESOLUTION_RETRY_MS: u64 = 30_000;
const MAX_ACTIVE_TRANSCODES: usize = 2;

pub type VideoToolRunner =
    Arc<dyn Fn(&str, &[String]) -> Result<String, String> + Send + Sync>;

fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn read_capped(mut reader: impl Read, limit: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(limit.min(8 * 1024));
    let mut buffer = [0u8; 8 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                let remaining = limit.saturating_sub(output.len());
                if remaining > 0 {
                    output.extend_from_slice(&buffer[..count.min(remaining)]);
                }
            }
        }
    }
    output
}

pub fn run_video_tool(command: &str, args: &[String]) -> Result<String, String> {
    let mut child = Command::new(command)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{command} spawn failed: {error}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{command} stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("{command} stderr unavailable"))?;
    let max_bytes = crate::extensions::attachments::media_audio_transcode::MEDIA_FFMPEG_MAX_BUFFER_BYTES;
    let stdout_reader = thread::spawn(move || read_capped(stdout, max_bytes));
    let stderr_reader = thread::spawn(move || read_capped(stderr, max_bytes));

    let timeout_ms = if command == "ffprobe" {
        crate::extensions::attachments::media_audio_transcode::MEDIA_FFPROBE_TIMEOUT_MS
    } else {
        crate::extensions::attachments::media_audio_transcode::MEDIA_FFMPEG_TIMEOUT_MS
    };
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("{command} timed out"));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("{command} wait failed: {error}"));
            }
        }
    };

    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    if !status.success() {
        return Err(format!(
            "{command} failed: {}",
            String::from_utf8_lossy(&stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

struct TranscodeLimiter {
    active: Mutex<usize>,
    available: Condvar,
}

impl TranscodeLimiter {
    fn acquire(&'static self) -> TranscodePermit {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while *active >= MAX_ACTIVE_TRANSCODES {
            active = self
                .available
                .wait(active)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        *active += 1;
        TranscodePermit { limiter: self }
    }
}

struct TranscodePermit {
    limiter: &'static TranscodeLimiter,
}

impl Drop for TranscodePermit {
    fn drop(&mut self) {
        let mut active = self
            .limiter
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *active = active.saturating_sub(1);
        self.limiter.available.notify_one();
    }
}

static TRANSCODE_LIMITER: TranscodeLimiter = TranscodeLimiter {
    active: Mutex::new(0),
    available: Condvar::new(),
};

fn has_playback_rendition(path: &Path) -> bool {
    fs::metadata(path)
        .map(|info| info.is_file() && info.len() > 0)
        .unwrap_or(false)
}

pub fn create_video_playback_source_with(
    source_path: &Path,
    rendition_path: &Path,
    runner: &VideoToolRunner,
) -> Option<PathBuf> {
    let probe_args = vec![
        "-v".into(),
        "error".into(),
        "-select_streams".into(),
        "v:0".into(),
        "-show_entries".into(),
        "stream=codec_name".into(),
        "-of".into(),
        "default=noprint_wrappers=1:nokey=1".into(),
        "-protocol_whitelist".into(),
        "file".into(),
        source_path.to_string_lossy().into_owned(),
    ];
    let probe_raw = runner("ffprobe", &probe_args).ok()?;
    let probe_trimmed = probe_raw.trim();

    let policy = crate::extensions::attachments::media_probe::get_playback_policy(
        crate::extensions::attachments::media_probe::PlaybackMediaKind::Video,
    );
    let mime = crate::media_mime::video_mime_from_path(source_path).unwrap_or("video/mp4");
    let playback_mode = crate::extensions::attachments::media_probe::resolve_playback_mode(mime, policy);

    let requires_transcode = if playback_mode == Some(crate::extensions::attachments::media_probe::PlaybackMode::Transcode) {
        true
    } else if let Some(probe_meta) =
        crate::extensions::attachments::media_probe::parse_ffprobe_media_metadata(
            probe_trimmed,
            crate::extensions::attachments::media_probe::PlaybackMediaKind::Video,
        )
    {
        crate::extensions::attachments::media_probe::resolve_native_playback_codec_compatibility(
            crate::extensions::attachments::media_probe::PlaybackMediaKind::Video,
            mime,
            &probe_meta,
        ) == Some(false)
    } else {
        probe_trimmed.to_ascii_lowercase() == "hevc"
    };

    if !requires_transcode {
        return Some(source_path.to_path_buf());
    }
    if has_playback_rendition(rendition_path) {
        return Some(rendition_path.to_path_buf());
    }

    let _permit = TRANSCODE_LIMITER.acquire();
    if has_playback_rendition(rendition_path) {
        return Some(rendition_path.to_path_buf());
    }
    if let Some(parent) = rendition_path.parent() {
        fs::create_dir_all(parent).ok()?;
    }
    let temp_path = PathBuf::from(format!(
        "{}.{}.tmp.mp4",
        rendition_path.to_string_lossy(),
        Uuid::new_v4()
    ));
    let ffmpeg_args = vec![
        "-v".into(),
        "error".into(),
        "-nostdin".into(),
        "-y".into(),
        "-protocol_whitelist".into(),
        "file".into(),
        "-i".into(),
        source_path.to_string_lossy().into_owned(),
        "-map".into(),
        "0:v:0".into(),
        "-map".into(),
        "0:a:0?".into(),
        "-c:v".into(),
        "libx264".into(),
        "-threads".into(),
        "2".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        "23".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "128k".into(),
        "-movflags".into(),
        "+faststart".into(),
        temp_path.to_string_lossy().into_owned(),
    ];

    let result = (|| {
        runner("ffmpeg", &ffmpeg_args).ok()?;
        let info = fs::metadata(&temp_path).ok()?;
        if !info.is_file() || info.len() == 0 {
            return None;
        }
        fs::rename(&temp_path, rendition_path).ok()?;
        Some(rendition_path.to_path_buf())
    })();
    crate::extensions::attachments::media_file_context::unlink_if_exists(Some(&temp_path));
    result
}

pub fn create_video_playback_source(
    source_path: &Path,
    rendition_path: &Path,
) -> Option<PathBuf> {
    let runner: VideoToolRunner = Arc::new(|command, args| run_video_tool(command, args));
    create_video_playback_source_with(source_path, rendition_path, &runner)
}

fn source_version(source_path: &Path) -> Option<String> {
    let info = fs::metadata(source_path).ok()?;
    if !info.is_file() {
        return None;
    }
    let modified_ms = info
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let mut hash = Sha256::new();
    hash.update(source_path.to_string_lossy().as_bytes());
    hash.update(b"\0");
    hash.update(info.len().to_string().as_bytes());
    hash.update(b"\0");
    hash.update(modified_ms.to_string().as_bytes());
    Some(format!("{:x}", hash.finalize()))
}

enum ResolutionState {
    Pending,
    Ready(Option<PathBuf>),
}

struct Resolution {
    source_path: PathBuf,
    rendition_path: PathBuf,
    state: Mutex<ResolutionState>,
    ready: Condvar,
    active_readers: AtomicUsize,
    evicted: AtomicBool,
    retry_after_ms: AtomicU64,
}

impl Resolution {
    fn new(source_path: PathBuf, rendition_path: PathBuf) -> Self {
        Self {
            source_path,
            rendition_path,
            state: Mutex::new(ResolutionState::Pending),
            ready: Condvar::new(),
            active_readers: AtomicUsize::new(0),
            evicted: AtomicBool::new(false),
            retry_after_ms: AtomicU64::new(0),
        }
    }

    fn cleanup_if_evicted(&self) {
        if !self.evicted.load(Ordering::Acquire)
            || self.active_readers.load(Ordering::Acquire) > 0
        {
            return;
        }
        let ready = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if matches!(*ready, ResolutionState::Ready(_)) {
            crate::extensions::attachments::media_file_context::unlink_if_exists(Some(&self.rendition_path));
        }
    }

    fn resolve(&self, creator: bool, runner: &VideoToolRunner, now_ms: u64) -> PathBuf {
        if creator {
            let result =
                create_video_playback_source_with(&self.source_path, &self.rendition_path, runner);
            if result.is_none() {
                self.retry_after_ms.store(
                    now_ms.saturating_add(FAILED_RESOLUTION_RETRY_MS),
                    Ordering::Release,
                );
            }
            *self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                ResolutionState::Ready(result);
            self.ready.notify_all();
            self.cleanup_if_evicted();
        }

        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            match &*state {
                ResolutionState::Ready(Some(path)) => return path.clone(),
                ResolutionState::Ready(None) => return self.source_path.clone(),
                ResolutionState::Pending => {
                    state = self
                        .ready
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
            }
        }
    }
}

#[derive(Default)]
struct PlaybackCache {
    entries: HashMap<String, Arc<Resolution>>,
    order: VecDeque<String>,
}

impl PlaybackCache {
    fn touch(&mut self, key: &str) {
        self.order.retain(|candidate| candidate != key);
        self.order.push_back(key.to_string());
    }

    fn evict_to_limit(&mut self) {
        while self.entries.len() > MAX_CACHED_RENDITIONS {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(resolution) = self.entries.remove(&oldest) {
                resolution.evicted.store(true, Ordering::Release);
                resolution.cleanup_if_evicted();
            }
        }
    }
}

pub struct VideoPlaybackResolver {
    runner: VideoToolRunner,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    cache_dir: PathBuf,
    cache: Mutex<PlaybackCache>,
}

impl VideoPlaybackResolver {
    pub fn new(
        runner: VideoToolRunner,
        now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
        cache_dir: PathBuf,
    ) -> Self {
        Self {
            runner,
            now_ms,
            cache_dir,
            cache: Mutex::new(PlaybackCache::default()),
        }
    }

    fn resolution_for(&self, source_path: &Path) -> Option<(Arc<Resolution>, bool)> {
        let version = source_version(source_path)?;
        let now_ms = (self.now_ms)();
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(existing) = cache.entries.get(&version).cloned() {
            let retry_after = existing.retry_after_ms.load(Ordering::Acquire);
            if retry_after == 0 || retry_after > now_ms {
                cache.touch(&version);
                return Some((existing, false));
            }
            cache.entries.remove(&version);
            cache.order.retain(|candidate| candidate != &version);
            existing.evicted.store(true, Ordering::Release);
            existing.cleanup_if_evicted();
        }

        let rendition_path = self
            .cache_dir
            .join(format!("{version}.{}.mp4", Uuid::new_v4()));
        let resolution = Arc::new(Resolution::new(
            source_path.to_path_buf(),
            rendition_path,
        ));
        cache.entries.insert(version.clone(), Arc::clone(&resolution));
        cache.touch(&version);
        cache.evict_to_limit();
        Some((resolution, true))
    }

    pub fn with_source<T, E, F>(&self, source_path: &Path, read: F) -> Result<T, E>
    where
        F: FnOnce(&Path) -> Result<T, E>,
    {
        let Some((resolution, creator)) = self.resolution_for(source_path) else {
            return read(source_path);
        };
        resolution.active_readers.fetch_add(1, Ordering::AcqRel);
        let resolved = resolution.resolve(creator, &self.runner, (self.now_ms)());
        let result = read(&resolved);
        resolution.active_readers.fetch_sub(1, Ordering::AcqRel);
        resolution.cleanup_if_evicted();
        result
    }
}

fn default_resolver() -> &'static VideoPlaybackResolver {
    static RESOLVER: OnceLock<VideoPlaybackResolver> = OnceLock::new();
    RESOLVER.get_or_init(|| {
        VideoPlaybackResolver::new(
            Arc::new(|command, args| run_video_tool(command, args)),
            Arc::new(system_now_ms),
            std::env::temp_dir().join(format!(
                "sand-video-playback-{}",
                std::process::id()
            )),
        )
    })
}

pub fn with_video_playback_source<T, E, F>(
    source_path: &Path,
    read: F,
) -> Result<T, E>
where
    F: FnOnce(&Path) -> Result<T, E>,
{
    default_resolver().with_source(source_path, read)
}

pub const PLAYBACK_TRANSCODE_CACHE_VERSION: &str = "v2";
pub const PLAYBACK_TRANSCODE_SUBDIR: &str = "playback-transcode";
pub const PLAYBACK_TRANSCODE_MAX_ALLOC_BYTES: u64 = 256 * 1024 * 1024;
pub const PLAYBACK_TRANSCODE_MAX_DURATION_SECS: u64 = 20 * 60;
pub const PLAYBACK_TRANSCODE_MAX_INPUT_PIXELS: u64 = 4096 * 4096;
pub const PLAYBACK_TRANSCODE_THREADS: u32 = 2;
pub const PLAYBACK_TRANSCODE_FAILURE_COOLDOWN_MS: u64 = 60_000;

pub fn create_playback_transcode_cache_key(
    path: &str,
    size: u64,
    mtime_ms: u64,
    ctime_ms: u64,
    dev: u64,
    ino: u64,
) -> String {
    let payload = serde_json::json!([
        path,
        size,
        mtime_ms,
        ctime_ms,
        dev.to_string(),
        ino.to_string()
    ]);
    let serialized = serde_json::to_string(&payload).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn playback_cache_relative_path(cache_key: &str, extension: &str) -> String {
    format!(
        "{}/{}-{}{}",
        PLAYBACK_TRANSCODE_SUBDIR,
        PLAYBACK_TRANSCODE_CACHE_VERSION,
        cache_key,
        extension
    )
}

pub fn replace_playback_file_extension(file_name: &str, extension: &str) -> String {
    let ext_idx = file_name.rfind('.').and_then(|idx| {
        if idx > 0 {
            Some(idx)
        } else {
            None
        }
    });
    let stem = match ext_idx {
        Some(idx) => &file_name[..idx],
        None => file_name,
    };
    let stem = if stem.is_empty() { "media" } else { stem };
    format!("{}{}", stem, extension)
}

pub fn playback_durations_match(source_duration_ms: u64, output_duration_ms: u64) -> bool {
    let tolerance_ms = ((source_duration_ms as f64 * 0.02).ceil() as u64).clamp(1000, 2000);
    output_duration_ms <= PLAYBACK_TRANSCODE_MAX_DURATION_SECS * 1000
        && (source_duration_ms as i64 - output_duration_ms as i64).unsigned_abs() <= tolerance_ms
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackTranscodeResolution {
    Passthrough,
    Preparing,
    Fallback,
    Transcoded {
        path: PathBuf,
        content_type: String,
        extension: String,
    },
}

pub fn build_ffmpeg_playback_args(
    input_path: &Path,
    output_path: &Path,
    kind: crate::extensions::attachments::media_probe::PlaybackMediaKind,
    input_format: &str,
    primary_stream_index: u32,
    audio_stream_index: Option<u32>,
    max_bytes: u64,
) -> Vec<String> {
    let audio_only = kind == crate::extensions::attachments::media_probe::PlaybackMediaKind::Audio;
    let mut args = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-max_alloc".into(),
        PLAYBACK_TRANSCODE_MAX_ALLOC_BYTES.to_string(),
        "-filter_threads".into(),
        PLAYBACK_TRANSCODE_THREADS.to_string(),
        "-y".into(),
        "-protocol_whitelist".into(),
        "file".into(),
        "-f".into(),
        input_format.to_string(),
        "-max_pixels".into(),
        PLAYBACK_TRANSCODE_MAX_INPUT_PIXELS.to_string(),
        "-threads".into(),
        PLAYBACK_TRANSCODE_THREADS.to_string(),
        "-i".into(),
        input_path.to_string_lossy().into_owned(),
        "-map_metadata".into(),
        "-1".into(),
        "-map_chapters".into(),
        "-1".into(),
        "-map".into(),
        format!("0:{}", primary_stream_index),
    ];
    if !audio_only {
        if let Some(audio_idx) = audio_stream_index {
            args.push("-map".into());
            args.push(format!("0:{}", audio_idx));
        }
    }
    if audio_only {
        args.push("-vn".into());
    }
    args.push("-sn".into());
    args.push("-dn".into());
    args.push("-t".into());
    args.push(PLAYBACK_TRANSCODE_MAX_DURATION_SECS.to_string());
    if !audio_only {
        args.push("-vf".into());
        args.push("scale=w='min(1920,iw)':h='min(1080,ih)':force_original_aspect_ratio=decrease:force_divisible_by=2".into());
        args.push("-c:v".into());
        args.push("libx264".into());
        args.push("-threads".into());
        args.push(PLAYBACK_TRANSCODE_THREADS.to_string());
        args.push("-pix_fmt".into());
        args.push("yuv420p".into());
    }
    args.push("-c:a".into());
    args.push("aac".into());
    args.push("-b:a".into());
    args.push("128k".into());
    args.push("-movflags".into());
    args.push("+faststart".into());
    args.push("-f".into());
    args.push((if audio_only { "ipod" } else { "mp4" }).into());
    args.push("-fs".into());
    args.push((max_bytes + 1).to_string());
    args.push(output_path.to_string_lossy().into_owned());
    args
}

