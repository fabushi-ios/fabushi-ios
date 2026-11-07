pub mod chunked_upload;
pub mod range_stream;
pub mod thumbnails;
pub mod waveform;

pub use chunked_upload::{
    recommend_part_size, ChunkedUploadSession, BIG_FILE_THRESHOLD, PART_SIZE_128KB, PART_SIZE_1MB,
    PART_SIZE_256KB, PART_SIZE_32KB, PART_SIZE_512KB, PART_SIZE_64KB,
};
pub use range_stream::{ByteRange, RangeError, RangeStreamAdapter};
pub use thumbnails::{
    fit_dimensions_downscale_only, shrink_to_keep_aspect, ThumbnailGenerator, ThumbnailPreset,
};
pub use waveform::{
    decode_waveform_5bit, decode_waveform_to_floats, encode_waveform_5bit,
    encode_waveform_from_floats, normalize_pcm_peaks, resample_and_normalize_waveform,
    MINIMAL_PEAK_FLOOR, PEAK_AVERAGE_RATIO, TELEGRAM_WAVEFORM_BARS, TELEGRAM_WAVEFORM_BYTES,
    WAVEFORM_MAX_VALUE,
};

pub mod editor;
pub use editor::*;
pub mod media;
pub use media::*;
