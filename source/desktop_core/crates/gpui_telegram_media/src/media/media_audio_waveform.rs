//! Telegram Desktop 1:1 Parity Submodule: `MediaAudioWaveform`
//! Upstream Sources:
//! - `Telegram/SourceFiles/media/media_audio_waveform.h:L1-L28`
//! - `Telegram/SourceFiles/media/media_audio_waveform.cpp:L1-L259`
//! Subsystem: `media` | Feature: `TCG-N07`

pub const K_MINIMAL_PEAK: u16 = 2500;
pub const K_MAX_PEAK: u16 = u16::MAX;
pub const K_PEAK_AVERAGE_RATIO: f64 = 1.8;
pub const K_FINE_SPANS_PER_SECOND: i32 = 50;
pub const K_MAX_FINE_SPANS: usize = 100_000;

/// Audio waveform container corresponding to `Media::Audio::Waveform` (media_audio_waveform.h:L12-L19).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Waveform {
    pub bars: Vec<u16>,
    pub peak: u16,
}

impl Waveform {
    pub fn new(bars: Vec<u16>, peak: u16) -> Self {
        Self { bars, peak }
    }

    pub fn empty(&self) -> bool {
        self.bars.is_empty()
    }

    pub fn count(&self) -> usize {
        self.bars.len()
    }
}

/// Computes a Telegram audio waveform from 16-bit PCM samples (media_audio_waveform.cpp:L184-L256).
pub fn compute_waveform_from_pcm(samples: &[i16], sample_rate: i32, bar_count: usize) -> Waveform {
    if bar_count == 0 || samples.is_empty() || sample_rate <= 0 {
        return Waveform::default();
    }

    let mut span_size = (sample_rate / K_FINE_SPANS_PER_SECOND).max(1) as usize;
    let mut fine = Vec::with_capacity(bar_count.max(64));
    let mut current_peak: u16 = 0;
    let mut in_span = 0;

    for &sample in samples {
        let abs_val = sample.unsigned_abs();
        if abs_val > current_peak {
            current_peak = abs_val;
        }
        in_span += 1;
        if in_span < span_size {
            continue;
        }

        if fine.len() < K_MAX_FINE_SPANS {
            fine.push(current_peak);
        } else {
            // Downsample by halving pairs (media_audio_waveform.cpp:L218-L222)
            let half = K_MAX_FINE_SPANS / 2;
            for i in 0..half {
                fine[i] = fine[2 * i].max(fine[2 * i + 1]);
            }
            fine.truncate(half);
            span_size *= 2;
            fine.push(current_peak);
        }
        current_peak = 0;
        in_span = 0;
    }

    if in_span > 0 && fine.len() < K_MAX_FINE_SPANS {
        fine.push(current_peak);
    }

    if fine.is_empty() {
        return Waveform::default();
    }

    let total = fine.len();
    let mut bars = vec![0u16; bar_count];
    for i in 0..bar_count {
        let from = i * total / bar_count;
        let till = ((i + 1) * total / bar_count).max(from + 1).min(total);
        let mut peak = 0u16;
        for &val in &fine[from..till] {
            if val > peak {
                peak = val;
            }
        }
        bars[i] = peak;
    }

    // Peak calculation with threshold floor and average ratio (L244-L254)
    let sum: u64 = bars.iter().map(|&b| b as u64).sum();
    let computed_peak = (sum as f64 * K_PEAK_AVERAGE_RATIO / bar_count as f64) as u64;
    let peak = (K_MINIMAL_PEAK as u64)
        .max(computed_peak)
        .min(K_MAX_PEAK as u64) as u16;

    for bar in &mut bars {
        if *bar > peak {
            *bar = peak;
        }
    }

    Waveform { bars, peak }
}

/// Parity target coordinator struct `MediaAudioWaveform`.
#[derive(Debug, Clone, Default)]
pub struct MediaAudioWaveform {
    pub current_waveform: Waveform,
}

impl MediaAudioWaveform {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load_waveform_from_pcm(&mut self, samples: &[i16], sample_rate: i32, bar_count: usize) {
        self.current_waveform = compute_waveform_from_pcm(samples, sample_rate, bar_count);
    }

    pub fn bars(&self) -> &[u16] {
        &self.current_waveform.bars
    }

    pub fn peak(&self) -> u16 {
        self.current_waveform.peak
    }
}
