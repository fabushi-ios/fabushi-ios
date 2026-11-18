use crate::media::audio::media_audio::calculate_voice_waveform_100_bars;
use crate::waveform::encoder::encode_waveform_5bit;
use gpui_telegram_core::error::{Result, TelegramError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedAudioPcm {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_ms: i64,
}

/// Encodes signed 16-bit linear PCM samples into a canonical RIFF/WAV byte vector.
pub fn encode_wav_i16(samples: &[i16], sample_rate: u32, channels: u16) -> Result<Vec<u8>> {
    if sample_rate == 0 || channels == 0 {
        return Err(TelegramError::Validation(
            "WAV sample_rate and channels must be non-zero".into(),
        ));
    }
    let data_bytes_len = samples
        .len()
        .checked_mul(2)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| TelegramError::Validation("WAV PCM buffer exceeds 4GiB limit".into()))?;
    let byte_rate = sample_rate
        .saturating_mul(channels as u32)
        .saturating_mul(2);
    let block_align = channels.saturating_mul(2);
    let riff_size = 36u32.saturating_add(data_bytes_len);

    let mut out = Vec::with_capacity(44 + data_bytes_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes_len.to_le_bytes());
    for &s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    Ok(out)
}

/// Decodes a RIFF/WAV stream (8/16/24-bit integer PCM or 32-bit float) into mono-interleaved i16 PCM.
pub fn decode_wav_pcm(wav_bytes: &[u8]) -> Result<DecodedAudioPcm> {
    if wav_bytes.len() < 12 || &wav_bytes[0..4] != b"RIFF" || &wav_bytes[8..12] != b"WAVE" {
        return Err(TelegramError::Validation("Invalid RIFF/WAVE header".into()));
    }
    let mut offset = 12usize;
    let mut audio_format: u16 = 0;
    let mut channels: u16 = 0;
    let mut sample_rate: u32 = 0;
    let mut bits_per_sample: u16 = 0;
    let mut fmt_seen = false;
    let mut data_slice: Option<&[u8]> = None;

    while offset + 8 <= wav_bytes.len() {
        let id = &wav_bytes[offset..offset + 4];
        let chunk_size = u32::from_le_bytes([
            wav_bytes[offset + 4],
            wav_bytes[offset + 5],
            wav_bytes[offset + 6],
            wav_bytes[offset + 7],
        ]) as usize;
        offset += 8;
        let end = offset.saturating_add(chunk_size).min(wav_bytes.len());
        let chunk = &wav_bytes[offset..end];
        if id == b"fmt " && chunk.len() >= 16 {
            audio_format = u16::from_le_bytes([chunk[0], chunk[1]]);
            channels = u16::from_le_bytes([chunk[2], chunk[3]]);
            sample_rate = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
            bits_per_sample = u16::from_le_bytes([chunk[14], chunk[15]]);
            fmt_seen = true;
        } else if id == b"data" {
            data_slice = Some(chunk);
        }
        offset = end + (chunk_size & 1);
    }

    if !fmt_seen || sample_rate == 0 || channels == 0 {
        return Err(TelegramError::Validation("Missing or invalid WAV fmt chunk".into()));
    }
    let data = data_slice.ok_or_else(|| TelegramError::Validation("Missing WAV data chunk".into()))?;
    let mut samples = Vec::new();

    match (audio_format, bits_per_sample) {
        (1, 8) => {
            samples.reserve(data.len());
            for &b in data {
                samples.push(((b as i16) - 128) << 8);
            }
        }
        (1, 16) => {
            samples.reserve(data.len() / 2);
            for frame in data.chunks_exact(2) {
                samples.push(i16::from_le_bytes([frame[0], frame[1]]));
            }
        }
        (1, 24) => {
            samples.reserve(data.len() / 3);
            for frame in data.chunks_exact(3) {
                let raw = ((frame[2] as i32) << 24) | ((frame[1] as i32) << 16) | ((frame[0] as i32) << 8);
                samples.push((raw >> 16) as i16);
            }
        }
        (3, 32) => {
            samples.reserve(data.len() / 4);
            for frame in data.chunks_exact(4) {
                let f = f32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
                let scaled = (f.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                samples.push(scaled);
            }
        }
        _ => {
            return Err(TelegramError::Validation(format!(
                "Unsupported WAV format={} bits={}",
                audio_format, bits_per_sample
            )));
        }
    }

    let total_frames = samples.len() / (channels as usize).max(1);
    let duration_ms = ((total_frames as i128 * 1000) / sample_rate as i128) as i64;
    Ok(DecodedAudioPcm {
        samples,
        sample_rate,
        channels,
        duration_ms,
    })
}

/// Computes normalized RMS amplitude in range 0..=32767 for live VU meters.
pub fn compute_pcm_rms_level(samples: &[i16]) -> u16 {
    if samples.is_empty() {
        return 0;
    }
    let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    let rms = (sum_sq / samples.len() as f64).sqrt();
    rms.clamp(0.0, 32767.0).round() as u16
}

/// Generates Telegram's 100-bar 5-bit waveform and 63-byte packed payload from signed i16 PCM.
pub fn waveform_from_pcm_i16(samples: &[i16]) -> (Vec<u8>, Vec<u8>) {
    let bars = calculate_voice_waveform_100_bars(samples);
    let packed = encode_waveform_5bit(&bars);
    (bars, packed)
}
