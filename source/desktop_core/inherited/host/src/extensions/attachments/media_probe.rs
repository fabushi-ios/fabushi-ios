use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Instant;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackMediaKind {
    Audio,
    Video,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackMode {
    Native,
    Transcode,
}

#[derive(Debug, Clone)]
pub struct PlaybackPolicyEntry {
    pub native_mime_types: &'static [&'static str],
    pub codec_probe_input_formats: &'static [(&'static str, &'static str)],
    pub transcode_input_formats: &'static [(&'static str, &'static str)],
    pub target_content_type: &'static str,
    pub target_extension: &'static str,
}

pub static PLAYBACK_AUDIO_POLICY: PlaybackPolicyEntry = PlaybackPolicyEntry {
    native_mime_types: &[
        "audio/m4a",
        "audio/mp3",
        "audio/mp4",
        "audio/mpeg",
        "audio/wav",
        "audio/wave",
        "audio/x-m4a",
        "audio/x-wav",
    ],
    codec_probe_input_formats: &[
        ("audio/m4a", "mov"),
        ("audio/mpeg", "mp3"),
        ("audio/mp4", "mov"),
        ("audio/wav", "wav"),
        ("audio/wave", "wav"),
        ("audio/x-m4a", "mov"),
        ("audio/x-wav", "wav"),
    ],
    transcode_input_formats: &[
        ("audio/aac", "aac"),
        ("audio/aiff", "aiff"),
        ("audio/amr", "amr"),
        ("audio/amr-wb", "amr"),
        ("audio/flac", "flac"),
        ("audio/ogg", "ogg"),
        ("audio/opus", "ogg"),
        ("audio/vorbis", "ogg"),
        ("audio/webm", "matroska,webm"),
        ("audio/x-aiff", "aiff"),
        ("audio/x-caf", "caf"),
        ("audio/x-ms-asf", "asf"),
        ("audio/x-ms-wma", "asf"),
    ],
    target_content_type: "audio/mp4",
    target_extension: ".m4a",
};

pub static PLAYBACK_VIDEO_POLICY: PlaybackPolicyEntry = PlaybackPolicyEntry {
    native_mime_types: &["video/mp4"],
    codec_probe_input_formats: &[("video/mp4", "mov")],
    transcode_input_formats: &[
        ("video/avi", "avi"),
        ("video/flv", "flv"),
        ("video/matroska", "matroska,webm"),
        ("video/quicktime", "mov"),
        ("video/webm", "matroska,webm"),
        ("video/x-flv", "flv"),
        ("video/x-matroska", "matroska,webm"),
        ("video/x-ms-asf", "asf"),
        ("video/x-ms-wmv", "asf"),
        ("video/x-msvideo", "avi"),
    ],
    target_content_type: "video/mp4",
    target_extension: ".mp4",
};

pub fn get_playback_policy(kind: PlaybackMediaKind) -> &'static PlaybackPolicyEntry {
    match kind {
        PlaybackMediaKind::Audio => &PLAYBACK_AUDIO_POLICY,
        PlaybackMediaKind::Video => &PLAYBACK_VIDEO_POLICY,
    }
}

pub fn normalize_mime(mime: &str) -> String {
    mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

pub fn resolve_playback_mode(
    mime_type: &str,
    policy: &PlaybackPolicyEntry,
) -> Option<PlaybackMode> {
    let mime = normalize_mime(mime_type);
    if mime.is_empty() {
        return None;
    }
    if policy.native_mime_types.iter().any(|&m| m == mime) {
        return Some(PlaybackMode::Native);
    }
    if policy.transcode_input_formats.iter().any(|&(m, _)| m == mime) {
        return Some(PlaybackMode::Transcode);
    }
    None
}

pub fn resolve_playback_input_format(
    policy: &PlaybackPolicyEntry,
    mime_type: &str,
) -> Option<&'static str> {
    let norm = normalize_mime(mime_type);
    if norm.is_empty() {
        return None;
    }
    if let Some(&(_, fmt)) = policy.transcode_input_formats.iter().find(|&&(m, _)| m == norm) {
        return Some(fmt);
    }
    if let Some(&(_, fmt)) = policy.codec_probe_input_formats.iter().find(|&&(m, _)| m == norm) {
        return Some(fmt);
    }
    None
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaProbeResult {
    pub duration_ms: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PlaybackMediaProbeResult {
    pub duration_ms: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub audio_codec: Option<String>,
    pub audio_stream_index: Option<u32>,
    pub video_codec: Option<String>,
    pub video_pixel_format: Option<String>,
    pub video_profile: Option<String>,
    pub video_rotation: Option<i32>,
    pub video_sample_aspect_ratio: Option<f64>,
    pub video_stream_index: Option<u32>,
}

pub fn resolve_native_playback_codec_compatibility(
    kind: PlaybackMediaKind,
    mime_type: &str,
    probe: &PlaybackMediaProbeResult,
) -> Option<bool> {
    let norm_mime = normalize_mime(mime_type);
    if kind == PlaybackMediaKind::Audio {
        let codec = probe.audio_codec.as_deref()?;
        if probe.audio_stream_index.is_none() {
            return None;
        }
        if norm_mime == "audio/wav" || norm_mime == "audio/x-wav" || norm_mime == "audio/wave" {
            return Some(codec == "pcm_s16le" || codec == "pcm_u8");
        }
        return Some(codec == "mp3" || (norm_mime != "audio/mpeg" && codec == "aac"));
    }

    let audio_compatible: Option<bool> = match probe.audio_stream_index {
        None => {
            if probe.audio_codec.is_some() {
                None
            } else {
                Some(true)
            }
        }
        Some(_) => match probe.audio_codec.as_deref() {
            Some(c) => Some(c == "aac" || c == "mp3"),
            None => None,
        },
    };

    let mut video_compatible: Option<bool> = None;
    if let (Some(v_codec), Some(_)) = (&probe.video_codec, probe.video_stream_index) {
        let portable_profile = match probe.video_profile.as_deref() {
            Some("baseline" | "constrained baseline" | "main" | "high") => true,
            _ => false,
        };
        let portable_pix = match probe.video_pixel_format.as_deref() {
            Some("yuv420p" | "yuvj420p") => true,
            _ => false,
        };
        if v_codec == "h264" && probe.video_profile.is_some() && probe.video_pixel_format.is_some() {
            video_compatible = Some(portable_profile && portable_pix);
        } else if v_codec == "h264" {
            video_compatible = None;
        } else {
            video_compatible = Some(false);
        }
    }

    if audio_compatible == Some(false) || video_compatible == Some(false) {
        Some(false)
    } else if audio_compatible == Some(true) && video_compatible == Some(true) {
        Some(true)
    } else {
        None
    }
}

pub fn to_media_probe_result(result: Option<&PlaybackMediaProbeResult>) -> MediaProbeResult {
    let Some(res) = result else {
        return MediaProbeResult::default();
    };

    let swaps_axes = (res.video_rotation.unwrap_or(0).abs() % 180) == 90;
    let sar = res.video_sample_aspect_ratio.unwrap_or(1.0);
    let calculated_width = res
        .width
        .map(|w| ((w as f64) * sar).round() as u32)
        .or(res.width);

    let (final_w, final_h) = match (calculated_width, res.height) {
        (Some(w), Some(h)) => {
            if swaps_axes {
                (Some(h), Some(w))
            } else {
                (Some(w), Some(h))
            }
        }
        _ => (None, None),
    };

    MediaProbeResult {
        duration_ms: res.duration_ms,
        width: final_w,
        height: final_h,
    }
}

fn parse_duration_ms(val: &Value) -> Option<u64> {
    if let Some(n) = val.as_f64() {
        if n > 0.0 && n.is_finite() {
            return Some((n * 1000.0).round() as u64);
        }
    } else if let Some(s) = val.as_str() {
        if let Ok(n) = s.trim().parse::<f64>() {
            if n > 0.0 && n.is_finite() {
                return Some((n * 1000.0).round() as u64);
            }
        }
    }
    None
}

pub fn parse_ffprobe_media_metadata(
    stdout: &str,
    kind: PlaybackMediaKind,
) -> Option<PlaybackMediaProbeResult> {
    let root: Value = serde_json::from_str(stdout).ok()?;
    let streams = root.get("streams")?.as_array()?;

    // Select audio stream: disposition.default == 1 preferred
    let mut audio_streams: Vec<&Value> = streams
        .iter()
        .filter(|s| s.get("codec_type").and_then(|v| v.as_str()) == Some("audio"))
        .collect();
    let audio_stream = audio_streams
        .iter()
        .find(|s| s.get("disposition").and_then(|d| d.get("default")).and_then(|v| v.as_i64()) == Some(1))
        .copied()
        .or_else(|| audio_streams.first().copied());

    // Select video stream: attached_pic != 1, disposition.default == 1 preferred
    let mut video_streams: Vec<&Value> = streams
        .iter()
        .filter(|s| {
            s.get("codec_type").and_then(|v| v.as_str()) == Some("video")
                && s.get("disposition")
                    .and_then(|d| d.get("attached_pic"))
                    .and_then(|v| v.as_i64())
                    != Some(1)
        })
        .collect();
    let video_stream = video_streams
        .iter()
        .find(|s| s.get("disposition").and_then(|d| d.get("default")).and_then(|v| v.as_i64()) == Some(1))
        .copied()
        .or_else(|| video_streams.first().copied());

    let mut stream_durations = Vec::new();
    if kind == PlaybackMediaKind::Video {
        if let Some(vs) = video_stream {
            if let Some(d) = vs.get("duration").and_then(parse_duration_ms) {
                stream_durations.push(d);
            }
        }
    }
    if let Some(as_s) = audio_stream {
        if let Some(d) = as_s.get("duration").and_then(parse_duration_ms) {
            stream_durations.push(d);
        }
    }

    let format_duration = root
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(parse_duration_ms);

    let duration_ms = stream_durations
        .into_iter()
        .max()
        .or(format_duration);

    let width = video_stream
        .and_then(|s| s.get("width"))
        .and_then(|w| w.as_u64())
        .map(|w| w as u32);
    let height = video_stream
        .and_then(|s| s.get("height"))
        .and_then(|h| h.as_u64())
        .map(|h| h as u32);

    let sar = video_stream
        .and_then(|s| s.get("sample_aspect_ratio"))
        .and_then(|v| v.as_str())
        .and_then(|s| {
            let mut parts = s.split(':');
            let num = parts.next()?.parse::<f64>().ok()?;
            let den = parts.next()?.parse::<f64>().ok()?;
            if den > 0.0 && num > 0.0 {
                Some(num / den)
            } else {
                None
            }
        });

    let rotation = video_stream
        .and_then(|s| s.get("side_data_list"))
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter().find_map(|item| {
                item.get("rotation").and_then(|r| r.as_i64()).map(|r| r as i32)
            })
        });

    let audio_codec = audio_stream
        .and_then(|s| s.get("codec_name"))
        .and_then(|c| c.as_str())
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty());

    let video_codec = video_stream
        .and_then(|s| s.get("codec_name"))
        .and_then(|c| c.as_str())
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty());

    let video_pixel_format = video_stream
        .and_then(|s| s.get("pix_fmt"))
        .and_then(|p| p.as_str())
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty());

    let video_profile = video_stream
        .and_then(|s| s.get("profile"))
        .and_then(|p| p.as_str())
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty());

    let audio_stream_index = audio_stream
        .and_then(|s| s.get("index"))
        .and_then(|i| i.as_u64())
        .map(|i| i as u32);

    let video_stream_index = video_stream
        .and_then(|s| s.get("index"))
        .and_then(|i| i.as_u64())
        .map(|i| i as u32);

    Some(PlaybackMediaProbeResult {
        duration_ms,
        width,
        height,
        audio_codec,
        audio_stream_index,
        video_codec,
        video_pixel_format,
        video_profile,
        video_rotation: rotation,
        video_sample_aspect_ratio: sar,
        video_stream_index,
    })
}
