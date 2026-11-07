use super::encoder::{normalize_pcm_peaks, TELEGRAM_WAVEFORM_BARS};

/// Resamples an arbitrary sequence of raw PCM samples (or peaks) into exactly 100 peaks
/// and normalizes them into 0..31 quantized bars.
///
/// Upstream reference: `Telegram/SourceFiles/media/audio/media_audio_capture.cpp` lines 42–79 (`CollectWaveform`).
pub fn resample_and_normalize_waveform(samples: &[u8]) -> Vec<u8> {
    if samples.is_empty() {
        return Vec::new();
    }

    let count = samples.len() as i64;
    let target_bars = TELEGRAM_WAVEFORM_BARS as i64;

    if count < target_bars {
        // Linear stretch if fewer than 100 samples
        let mut peaks = Vec::with_capacity(TELEGRAM_WAVEFORM_BARS);
        for i in 0..TELEGRAM_WAVEFORM_BARS {
            let src_idx = (i * samples.len()) / TELEGRAM_WAVEFORM_BARS;
            peaks.push((samples[src_idx] as u16) * 256);
        }
        return normalize_pcm_peaks(&peaks);
    }

    // Upstream accumulation algorithm
    let mut peaks = Vec::with_capacity(TELEGRAM_WAVEFORM_BARS);
    let mut peak: u16 = 0;
    let mut sum: i64 = 0;

    for &sample in samples {
        let sample_val = (sample as u16) * 256;
        if peak < sample_val {
            peak = sample_val;
        }
        sum += target_bars;
        if sum >= count {
            sum -= count;
            peaks.push(peak);
            peak = 0;
        }
    }

    if peaks.len() < TELEGRAM_WAVEFORM_BARS && !peaks.is_empty() {
        while peaks.len() < TELEGRAM_WAVEFORM_BARS {
            let last = *peaks.last().unwrap();
            peaks.push(last);
        }
    }

    normalize_pcm_peaks(&peaks)
}
