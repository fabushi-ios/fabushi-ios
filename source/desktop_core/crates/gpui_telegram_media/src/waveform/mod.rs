pub mod decoder;
pub mod encoder;
pub mod normalizer;

pub use decoder::{decode_waveform_5bit, decode_waveform_to_floats};
pub use encoder::{
    encode_waveform_5bit, encode_waveform_from_floats, normalize_pcm_peaks, MINIMAL_PEAK_FLOOR,
    PEAK_AVERAGE_RATIO, TELEGRAM_WAVEFORM_BARS, TELEGRAM_WAVEFORM_BYTES, WAVEFORM_MAX_VALUE,
};
pub use normalizer::resample_and_normalize_waveform;
